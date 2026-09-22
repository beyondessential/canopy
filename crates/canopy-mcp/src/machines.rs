//! `find_machines` / `get_machine` tools.
//!
//! A machine is the box; an application is the software on it. A query about
//! what a box is doing answers here, and one about what software is running
//! answers in [`crate::applications`]. Each result names the other side, so a
//! client can move between the two without a search.
// spec: MCP#discovery

use commons_types::{Uuid, status::HealthState};
use database::{
	machines::Machine, reported_detail::MachineReportedDetail, server_groups::ServerGroup,
	statuses::MergedDetail,
};
use jiff::Timestamp;
use rmcp::{
	handler::server::wrapper::Parameters,
	model::{CallToolResult, ErrorData as McpError},
	tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
	CanopyMcp,
	util::{group_names, mcp_err, ok_json, parse_opt_uuid, parse_uuid, unique},
};

const DEFAULT_MACHINE_LIMIT: i64 = 200;

/// A machine's sources folded into one view, newest value per field winning —
/// the same resolution the application grain uses.
// spec: FIG#sourcing
fn merge_machine_reports(reports: &[MachineReportedDetail]) -> MergedDetail {
	MergedDetail::from_reports(reports.iter().map(|r| (r.reported_at, &r.extra)))
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindMachinesArgs {
	/// Substring matched against the machine's name, its reported hostname, or
	/// its id.
	pub query: Option<String>,
	/// Only machines in this group's id.
	pub group_id: Option<String>,
	/// Only machines whose reported platform contains this (e.g. `Debian`).
	pub platform: Option<String>,
	/// Only cloud-hosted machines when true, only on-premises when false.
	pub cloud: Option<bool>,
	/// Include archived machines. Default false.
	pub include_archived: Option<bool>,
	/// Max machines to return (default 200).
	pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MachineIdArgs {
	/// The machine's id (UUID).
	pub machine_id: String,
}

/// What a box currently reports about itself. Resolved across every source
/// reporting on it, so a field the latest push omitted is still here.
// spec: FIG#sourcing
#[derive(Serialize)]
struct MachineFiguresOut {
	platform: Option<String>,
	hostname: Option<String>,
	os_timezone: Option<String>,
	bestool_version: Option<String>,
	cpu_cores: Option<u64>,
	total_memory_bytes: Option<u64>,
	uptime_seconds: Option<u64>,
	filesystems: Option<serde_json::Value>,
	addresses: MachineAddressesOut,
}

#[derive(Serialize)]
struct MachineAddressesOut {
	ipv4: Option<serde_json::Value>,
	ipv6: Option<serde_json::Value>,
	wan_ipv4: Option<serde_json::Value>,
	wan_ipv6: Option<serde_json::Value>,
}

impl MachineFiguresOut {
	fn from_detail(detail: &MergedDetail) -> Self {
		let num = |key: &str| detail.get(key).and_then(|v| v.as_u64());
		let text = |key: &str| detail.get(key).and_then(|v| v.as_str()).map(str::to_owned);
		Self {
			platform: detail.platform(),
			hostname: text("hostname"),
			os_timezone: text("osTimezone"),
			bestool_version: detail.bestool_version(),
			cpu_cores: num("cpuCores"),
			total_memory_bytes: num("totalMemoryBytes"),
			uptime_seconds: num("uptimeSecs"),
			filesystems: detail.get("filesystems").cloned(),
			addresses: MachineAddressesOut {
				ipv4: detail.get("ipv4").cloned(),
				ipv6: detail.get("ipv6").cloned(),
				wan_ipv4: detail.get("wanIpv4").cloned(),
				wan_ipv6: detail.get("wanIpv6").cloned(),
			},
		}
	}
}

/// One machine as a search result.
#[derive(Serialize)]
struct MachineSummary {
	id: Uuid,
	name: Option<String>,
	group_id: Option<Uuid>,
	group_name: Option<String>,
	cloud: Option<bool>,
	is_monitored: bool,
	archived: bool,
	registered_at: Option<Timestamp>,
	platform: Option<String>,
	/// How many applications run on this box. Two or more is the case the
	/// machine grain exists for.
	application_count: usize,
	health: HealthState,
}

#[derive(Serialize)]
struct FindMachinesResult {
	total_matched: usize,
	returned: usize,
	/// True when `limit` cut the result short.
	truncated: bool,
	machines: Vec<MachineSummary>,
}

/// An application on a machine, as the machine's own record names it.
#[derive(Serialize)]
struct MachineApplicationOut {
	id: Uuid,
	name: Option<String>,
	r#type: String,
	health: HealthState,
}

#[derive(Serialize)]
struct MachineDetail {
	id: Uuid,
	name: Option<String>,
	group_id: Option<Uuid>,
	group_name: Option<String>,
	cloud: Option<bool>,
	geolocation: Option<commons_types::geo::GeoPoint>,
	is_monitored: bool,
	archived: bool,
	notes: String,
	registered_at: Option<Timestamp>,
	tags: commons_types::server::TagMap,
	/// What the box reports about itself: platform, hardware, addresses.
	/// Distinct from an application's figures, which are about its software.
	figures: MachineFiguresOut,
	/// The machine's own health, from the checks filed against it. What the
	/// software on it makes of these is each application's own health.
	health: HealthState,
	/// The workloads running on this box.
	applications: Vec<MachineApplicationOut>,
}

#[tool_router(router = machines_router, vis = "pub(crate)")]
impl CanopyMcp {
	#[tool(
		// Every tool reads; none changes anything (see the SAFE spec).
		annotations(read_only_hint = true),
		description = "Find machines (the boxes applications run on) by name/hostname/id substring, \
		               optionally filtered by group, platform, or cloud-hosting. Returns compact \
		               records with each box's platform, health, and how many applications it \
		               carries. Ask about a machine for disks, memory, clock, or addresses; ask \
		               about an application for versions and database engines."
	)]
	async fn find_machines(
		&self,
		Parameters(args): Parameters<FindMachinesArgs>,
	) -> Result<CallToolResult, McpError> {
		let mut conn = self.conn().await?;
		let group = parse_opt_uuid(&args.group_id, "group_id")?;
		let limit = args.limit.unwrap_or(DEFAULT_MACHINE_LIMIT) as usize;

		let mut machines = Machine::list_live(&mut conn).await.map_err(mcp_err)?;
		if !args.include_archived.unwrap_or(false) {
			machines.retain(|m| m.deleted_at.is_none());
		}

		// Figures first: the platform filter and the query both read the
		// reported hostname, which lives in them rather than on the row.
		let reports = MachineReportedDetail::all(&mut conn)
			.await
			.map_err(mcp_err)?;
		let mut by_machine: std::collections::HashMap<Uuid, Vec<MachineReportedDetail>> =
			std::collections::HashMap::new();
		for report in reports {
			by_machine
				.entry(report.machine_id)
				.or_default()
				.push(report);
		}
		let detail_of = |id: Uuid| -> MergedDetail {
			merge_machine_reports(by_machine.get(&id).map(Vec::as_slice).unwrap_or(&[]))
		};

		let q = args.query.as_deref().map(str::to_lowercase);
		let platform = args.platform.as_deref().map(str::to_lowercase);
		machines.retain(|m| {
			let detail = detail_of(m.id);
			let hostname = detail
				.get("hostname")
				.and_then(|v| v.as_str())
				.map(str::to_lowercase);
			group
				.as_ref()
				.is_none_or(|g| m.group_id.as_ref() == Some(g))
				&& args.cloud.is_none_or(|c| m.cloud == Some(c))
				&& platform.as_deref().is_none_or(|p| {
					detail
						.platform()
						.is_some_and(|got| got.to_lowercase().contains(p))
				}) && q.as_deref().is_none_or(|q| {
				m.name
					.as_deref()
					.is_some_and(|n| n.to_lowercase().contains(q))
					|| hostname.as_deref().is_some_and(|h| h.contains(q))
					|| m.id.to_string().contains(q)
			})
		});

		let total_matched = machines.len();
		let truncated = total_matched > limit;
		machines.truncate(limit);
		if truncated {
			tracing::info!(total_matched, limit, "find_machines result truncated");
		}

		let group_ids = unique(machines.iter().filter_map(|m| m.group_id));
		let g_names = group_names(&mut conn, &group_ids).await?;
		let health = database::issues::machine_health_from_check_state(
			&mut conn,
			&machines
				.iter()
				.map(|m| (m.id, m.group_id))
				.collect::<Vec<_>>(),
		)
		.await
		.map_err(mcp_err)?;

		let mut out = Vec::with_capacity(machines.len());
		for machine in &machines {
			let applications = machine.applications(&mut conn).await.map_err(mcp_err)?;
			out.push(MachineSummary {
				id: machine.id,
				name: machine.name.clone(),
				group_id: machine.group_id,
				group_name: machine.group_id.and_then(|g| g_names.get(&g).cloned()),
				cloud: machine.cloud,
				is_monitored: machine.is_monitored,
				archived: machine.deleted_at.is_some(),
				registered_at: machine.registered_at,
				platform: detail_of(machine.id).platform(),
				application_count: applications.len(),
				health: health.get(&machine.id).copied().unwrap_or_default(),
			});
		}

		ok_json(&FindMachinesResult {
			total_matched,
			returned: out.len(),
			truncated,
			machines: out,
		})
	}

	#[tool(
		// Every tool reads; none changes anything (see the SAFE spec).
		annotations(read_only_hint = true),
		description = "Full detail for one machine: its own fields, what it reports about itself \
		               (platform, processor count, memory, filesystems, uptime, addresses), its \
		               health from the checks filed against it, and the applications running on \
		               it. Backup capability and history are the machine's rather than any one \
		               application's, so a box hosting two workloads reports one set."
	)]
	async fn get_machine(
		&self,
		Parameters(args): Parameters<MachineIdArgs>,
	) -> Result<CallToolResult, McpError> {
		let mut conn = self.conn().await?;
		let id = parse_uuid(&args.machine_id, "machine_id")?;

		let machine = Machine::get_by_id(&mut conn, id).await.map_err(mcp_err)?;
		let group = match machine.group_id {
			Some(gid) => ServerGroup::get_by_id(&mut conn, gid).await.ok(),
			None => None,
		};

		let reports = MachineReportedDetail::for_machine(&mut conn, id)
			.await
			.map_err(mcp_err)?;
		let figures = MachineFiguresOut::from_detail(&merge_machine_reports(&reports));

		let health = database::issues::machine_health_from_check_state(
			&mut conn,
			&[(machine.id, machine.group_id)],
		)
		.await
		.map_err(mcp_err)?
		.get(&machine.id)
		.copied()
		.unwrap_or_default();

		let on_box = machine.applications(&mut conn).await.map_err(mcp_err)?;
		let app_health = database::issues::health_from_check_state(
			&mut conn,
			&on_box
				.iter()
				.map(|a| (a.id, a.group_id))
				.collect::<Vec<_>>(),
		)
		.await
		.map_err(mcp_err)?;
		let applications = on_box
			.into_iter()
			.map(|a| MachineApplicationOut {
				health: app_health.get(&a.id).copied().unwrap_or_default(),
				id: a.id,
				name: a.name,
				r#type: a.r#type.to_string(),
			})
			.collect();

		ok_json(&MachineDetail {
			id: machine.id,
			name: machine.name,
			group_id: machine.group_id,
			group_name: group.map(|g| g.name),
			cloud: machine.cloud,
			geolocation: machine.geolocation,
			is_monitored: machine.is_monitored,
			archived: machine.deleted_at.is_some(),
			notes: machine.notes,
			registered_at: machine.registered_at,
			tags: machine.tags,
			figures,
			health,
			applications,
		})
	}
}
