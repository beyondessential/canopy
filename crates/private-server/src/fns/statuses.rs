use std::collections::{BTreeSet, HashMap};

use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::TailscaleUser;
use commons_types::{
	namespace::{Namespace, NamespaceRef},
	server::{
		app_type::ApplicationType,
		cards::{FacilityServerStatus, ServerGroupCard},
		rank::ServerRank,
	},
	status::{CheckResult, OperatorPresence, ShortStatus},
	version::VersionStr,
};
use database::{
	applications::Application,
	check_policies::CheckPolicy,
	devices::DeviceConnection,
	issues::Issue,
	machines::Machine,
	reported_detail::{MachineReportedDetail, ReportedDetail},
	server_groups::ServerGroup,
	statuses::{MergedDetail, Status},
	tailscale_users::TailscaleUser as CachedTailscaleUser,
	versions::Version,
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::state::AppState;

/// The lowest and highest software version currently reported by any
/// production server.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize, ToSchema)]
pub struct LiveVersionsBracket {
	/// Oldest version currently reported by a production server.
	pub min: VersionStr,
	/// Newest version currently reported by a production server.
	pub max: VersionStr,
}

/// Fleet-wide summary of software versions currently running in production.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct SummaryData {
	/// Lowest and highest version currently reported by a production
	/// server.
	pub bracket: LiveVersionsBracket,
	/// Distinct major.minor release lines currently observed in
	/// production, each as a `[major, minor]` pair.
	#[schema(value_type = Vec<(u64, u64)>)]
	pub releases: BTreeSet<(u64, u64)>,
	/// Every distinct full version string currently observed in
	/// production, in ascending order.
	#[schema(value_type = Vec<VersionStr>)]
	pub versions: BTreeSet<VersionStr>,
}

/// Basic identifying details for a server. Currently unused by any
/// endpoint in this API.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ServerDetailsData {
	/// Application id.
	pub id: String,
	/// Application display name.
	pub name: String,
	/// The application's role within its type.
	pub kind: String,
	/// Application rank (e.g. production, test, dev).
	pub rank: String,
	/// Application hostname or address.
	pub host: String,
}

/// A simplified snapshot of a server's reported status. Currently unused by
/// any endpoint in this API.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ServerStatusData {
	/// Up/down state, as a short status string.
	pub up: String,
	/// When this status was last updated, as a formatted string.
	pub updated_at: Option<String>,
	/// Reported software version.
	pub version: Option<String>,
	/// Reported operating system platform.
	pub platform: Option<String>,
	/// Reported database engine version.
	pub postgres: Option<String>,
	/// Reported runtime version.
	pub nodejs: Option<String>,
	/// Reported system timezone.
	pub timezone: Option<String>,
}

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(summary))
		.routes(routes!(group_ids))
		.routes(routes!(group_details))
		.routes(routes!(snapshot))
		.routes(routes!(check_detail))
		.routes(routes!(fleet_detail))
}

/// Get a fleet-wide summary of software versions running in production.
///
/// Looks at the most recent status reported by every server ranked as
/// production (within the last 7 days) and returns the range of versions
/// seen, the distinct release lines, and every distinct exact version.
#[utoipa::path(
	post,
	path = "/summary",
	operation_id = "status_summary",
	tag = "statuses",
	responses(
		(status = 200, body = SummaryData),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
pub async fn summary(State(state): State<AppState>) -> Result<Json<SummaryData>> {
	let mut conn = state.db_read.get().await?;

	let versions: BTreeSet<VersionStr> = ReportedDetail::production_versions(&mut conn)
		.await?
		.into_iter()
		.collect();

	let bracket = LiveVersionsBracket {
		min: versions.first().cloned().unwrap_or_default(),
		max: versions.last().cloned().unwrap_or_default(),
	};
	let releases = versions
		.iter()
		.map(|v| (v.0.major, v.0.minor))
		.collect::<BTreeSet<_>>();

	Ok(Json(SummaryData {
		bracket,
		releases,
		versions,
	}))
}

/// List the server group ids the status page shows, ordered by name.
///
/// Alphabetical, because the card carries its own ranks: a rank row per rank,
/// each labelled. Ordering the cards by rank as well would sort the page by
/// something already written on every card, and leave an operator looking for
/// one group scanning for where its rank happens to start. A name is what they
/// know it by.
///
/// A group with no ranked member at all is omitted, as it always has been:
/// nothing in it has a place in the fleet's promotion order yet.
// spec: CHK#presentation
#[utoipa::path(
	post,
	path = "/group_ids",
	tag = "statuses",
	responses(
		(status = 200, description = "Application group IDs, ordered by group name.", body = Vec<Uuid>),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
pub async fn group_ids(State(state): State<AppState>) -> Result<Json<Vec<Uuid>>> {
	let mut conn = state.db_read.get().await?;
	let groups = ServerGroup::list_all(&mut conn).await?;
	if groups.is_empty() {
		return Ok(Json(Vec::new()));
	}
	let all: Vec<Uuid> = groups.iter().map(|g| g.id).collect();
	let top_rank = ServerGroup::highest_member_ranks(&mut conn, &all).await?;

	let mut ranked: Vec<(String, Uuid)> = groups
		.into_iter()
		.filter(|g| top_rank.contains_key(&g.id))
		.map(|g| (g.name, g.id))
		.collect();
	ranked.sort_by(|a, b| a.0.cmp(&b.0));
	Ok(Json(ranked.into_iter().map(|(_, id)| id).collect()))
}

/// Identifies the server group whose status details to fetch.
#[derive(Deserialize, ToSchema)]
pub struct GroupDetailsArgs {
	/// Id of the server group to fetch details for.
	pub server_group_id: Uuid,
}

/// Get a status card for a server group.
///
/// Returns the group's identity, its headline version and how far behind
/// the latest published release that version is, and a per-member summary
/// (up/down state, health, connected operators, rank, kind) for every
/// server in the group. Returns 404 if the group doesn't exist.
#[utoipa::path(
	post,
	path = "/group_details",
	tag = "statuses",
	request_body = GroupDetailsArgs,
	responses(
		(status = 200, body = ServerGroupCard),
		(status = 404, body = ProblemDetailsSchema),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
pub async fn group_details(
	State(state): State<AppState>,
	Json(args): Json<GroupDetailsArgs>,
) -> Result<Json<ServerGroupCard>> {
	let mut conn = state.db_read.get().await?;
	let group = ServerGroup::get_by_id(&mut conn, args.server_group_id).await?;
	let applications = group.list_servers(&mut conn).await?;

	// A group card shouldn't 404 just because no versions are published yet
	// (e.g. a fresh Canopy instance, or every version still draft); treat "no match"
	// as "unknown latest" so `version_distance` falls back to None. Same as
	// `applications::get_detail` and `statuses::snapshot`.
	let latest_version = match Version::get_latest_matching(&mut conn, "*".parse()?).await {
		Ok(v) => Some(v.as_semver()),
		Err(AppError::NoMatchingVersions) => None,
		Err(e) => return Err(e),
	};

	let server_ids: Vec<Uuid> = applications.iter().map(|s| s.id).collect();
	let status_map: HashMap<Uuid, Status> = Status::latest_for_servers(&mut conn, &server_ids)
		.await?
		.into_iter()
		.filter_map(|s| Some((s.server_id?, s)))
		.collect();
	// The archive affordance asks the same question `ServerGroup::soft_delete`
	// enforces: has every member been quiet for longer than the status window.
	// A member unreachable for months has still reported, so member
	// reachability cannot answer it, and the card carries the fact rather than
	// letting the UI infer it from dots that no longer mean that.
	let all_members_quiet = !server_ids.is_empty() && status_map.is_empty();
	// Reachability is graded on this rather than on the status window, so a
	// member quiet for longer than the window reads as unreachable rather than
	// as never heard from.
	// spec: CHK#reachability
	let last_reported =
		database::reported_detail::ReportedDetail::last_reported_ats(&mut conn, &server_ids)
			.await?;
	// Member health rolls up current check state across every source
	// (silenced checks already skipped in the rollup).
	let member_groups: Vec<(Uuid, Option<Uuid>)> =
		applications.iter().map(|s| (s.id, s.group_id)).collect();
	let member_health =
		database::issues::health_from_check_state(&mut conn, &member_groups).await?;

	// The boxes the members run on: their own reachability and health, which a
	// card presents on the enclosure around each machine's dots.
	// spec: FLT
	let machine_ids: Vec<Uuid> = applications.iter().map(|s| s.machine_id).collect();
	let machines: HashMap<Uuid, database::machines::Machine> =
		database::machines::Machine::get_many(&mut conn, &machine_ids)
			.await?
			.into_iter()
			.map(|m| (m.id, m))
			.collect();
	let machine_health = database::issues::machine_health_from_check_state(
		&mut conn,
		&applications
			.iter()
			.map(|s| (s.machine_id, s.group_id))
			.collect::<Vec<_>>(),
	)
	.await?;
	let machine_reports = database::reported_detail::MachineReportedDetail::latest_for_machines(
		&mut conn,
		&machine_ids,
	)
	.await?;
	// One read for the whole card: a window is over a machine or a group, and
	// a box is suspended by either.
	// spec: MNT#presentation
	let suspended =
		database::maintenance_windows::MaintenanceWindow::suspended_targets(&mut conn).await?;

	// The card's headline version is the cached last reported version of the
	// group's canonical member (highest rank, then highest kind), maintained by
	// the `statuses` trigger and `ServerGroup::recompute_version`. The distance
	// is computed against the latest published version.
	let card_version = group.effective_version.clone();
	let version_distance = card_version
		.as_ref()
		.zip(latest_version.as_ref())
		.map(|(current, latest)| database::statuses::version_distance(&current.0, latest));

	let mut members: Vec<FacilityServerStatus> = applications
		.into_iter()
		.map(|s| {
			let st = status_map.get(&s.id);
			let up = s.reachability(
				last_reported
					.get(&s.id)
					.copied()
					.max(st.map(|st| st.created_at)),
			);
			// Active presence only: an application that has stopped reporting
			// may well still have those sessions, but we cannot assert "in the
			// server right now" from a report that is past its own threshold.
			let operators = match up {
				ShortStatus::Up => st.map(|s| s.operators()).unwrap_or_default(),
				ShortStatus::Down | ShortStatus::Gone => Vec::new(),
			};
			FacilityServerStatus {
				id: s.id,
				name: s.display_name(),
				up,
				health: member_health.get(&s.id).copied().unwrap_or_default(),
				is_monitored: s.is_monitored,
				operators,
				rank: s.rank,
				r#type: s.r#type,
				machine_id: s.machine_id,
				machine_name: machines.get(&s.machine_id).and_then(|m| m.name.clone()),
				machine_up: machines.get(&s.machine_id).map_or(ShortStatus::Gone, |m| {
					m.reachability(machine_reports.get(&s.machine_id).copied())
				}),
				machine_health: machine_health
					.get(&s.machine_id)
					.copied()
					.unwrap_or_default(),
				machine_maintained: suspended.suspends(s.machine_id, s.group_id),
				machine_maintenance_settling: suspended.settling(s.machine_id, s.group_id),
			}
		})
		.collect();
	enrich_operators(
		&mut conn,
		members.iter_mut().flat_map(|m| m.operators.iter_mut()),
	)
	.await?;

	Ok(Json(ServerGroupCard {
		id: group.id,
		name: group.name,
		notes: group.notes,
		version: card_version,
		version_distance,
		members,
		all_members_quiet,
	}))
}

/// One server whose latest status reports [`CheckDetailData::check`],
/// for [`check_detail`].
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CheckDetailServerData {
	/// The server's id — the UI links to `/applications/{server_id}`.
	pub server_id: Uuid,
	/// The server's display name; empty string when the server has none.
	pub server_name: String,
	/// The server's group id, if it belongs to one — the UI links to
	/// `/groups/{group_id}`.
	pub group_id: Option<Uuid>,
	/// The server's group name, if it belongs to one.
	pub group_name: Option<String>,
	/// The server's rank, for the standard rank-bucket grouping.
	pub rank: Option<ServerRank>,
	/// The application's type, for the standard within-rank ordering.
	pub r#type: ApplicationType,
	/// The check's observed result on its latest report. The UI shows
	/// warning/failed/broken applications by default and puts passed/skipped
	/// ones behind a "show healthy" toggle.
	pub result: CheckResult,
	/// The check's own fields from its latest report, verbatim, so the
	/// row can expand to the same per-check detail the server page shows.
	pub data: serde_json::Value,
	/// When the check's current degradation streak began. `None` for
	/// applications currently reporting the check healthy.
	pub failing_since: Option<Timestamp>,
	/// When the check state last updated (the check's latest report).
	pub status_created_at: Timestamp,
	/// The state's stability record (observation counters, transition
	/// ring, hour-of-week duty profile, derived flap statistics). `None`
	/// for states that predate stability recording.
	pub stability: Option<database::stability::StabilityData>,
}

/// A group-scoped state of this check — a condition Canopy determines
/// about the group's control plane (backup health and the like) — for
/// the group's section of the check detail list.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CheckDetailGroupData {
	/// The group's id — the UI links to `/groups/{group_id}`.
	pub group_id: Uuid,
	/// The group's display name.
	pub group_name: String,
	/// Rank bucket for display: the highest rank held by any member
	/// server, mirroring the status page's group bucketing.
	pub rank: Option<ServerRank>,
	/// The check's observed result on its latest filing.
	pub result: CheckResult,
	/// The check's own fields from its latest filing, verbatim.
	pub data: serde_json::Value,
	/// When the current degradation streak began; `None` while healthy.
	pub failing_since: Option<Timestamp>,
	/// When the check state last updated.
	pub status_created_at: Timestamp,
	/// The state's stability record; `None` for states that predate
	/// stability recording.
	pub stability: Option<database::stability::StabilityData>,
}

/// The canopy-wide state of this check (self-monitoring), if any.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CheckDetailCanopyData {
	/// The check's observed result on its latest filing.
	pub result: CheckResult,
	/// The check's own fields from its latest filing, verbatim.
	pub data: serde_json::Value,
	/// When the current degradation streak began; `None` while healthy.
	pub failing_since: Option<Timestamp>,
	/// When the check state last updated.
	pub status_created_at: Timestamp,
	/// The state's stability record; `None` for states that predate
	/// stability recording.
	pub stability: Option<database::stability::StabilityData>,
}

/// Request body for [`check_detail`].
#[derive(Debug, Deserialize, ToSchema)]
pub struct CheckDetailArgs {
	/// The source that reports the check. A check's identity is the
	/// (source, check) pair — a same-named check from another source is
	/// a different check.
	pub source: String,
	/// The healthcheck name to look up, exactly as reported by devices in
	/// `health[].check` (an arbitrary, device/plugin-defined string).
	pub check: String,
	/// Which catalog entry is meant, as returned in the entry's `namespace`.
	/// Two application types reporting this name are two checks with separate
	/// pages; omitted is the unqualified namespace a curated source's checks
	/// live in.
	#[serde(default)]
	pub namespace: NamespaceRef,
}

/// Response for [`check_detail`]: the queried check's catalog policy
/// (if it has one yet) and every live server whose latest status reports
/// it, failing or healthy.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CheckDetailData {
	/// The source that was queried, echoed back with `check` so the page
	/// can render its heading without re-decoding the request.
	pub source: String,
	/// The check name that was queried.
	pub check: String,
	/// The configured policy ceiling for this (source, check), or `None`
	/// if the source has never reported it (so it has no catalog row).
	#[schema(value_type = Option<String>)]
	pub ceiling: Option<CheckResult>,
	/// Whether this check's policy escalates its effective failures.
	pub escalates: bool,
	/// Operator-authored documentation for this (source, check)
	/// (markdown), or `None` if nobody has written it yet.
	pub documentation: Option<String>,
	/// Every live server whose latest state from this source reports
	/// this check, at any result, ordered as a TODO list: failed,
	/// warning, broken, passed, skipped (most urgent first), then by
	/// group name then server name. The client filters out the
	/// passed/skipped tail unless the "show healthy" toggle is on.
	pub applications: Vec<CheckDetailServerData>,
	/// Group-scoped states of this check, ordered by group name. The
	/// client files each under its group in the list.
	pub groups: Vec<CheckDetailGroupData>,
	/// The canopy-wide state of this check, if canopy has ever filed one.
	pub canopy: Option<CheckDetailCanopyData>,
}

/// Rank used to order [`CheckDetailServerData::result`] most-urgent
/// first: failed, then warning, then broken, with the healthy tail
/// (passed, then skipped) last. Mirrors the private-web
/// `CHECK_RESULT_ORDER` display order.
fn check_result_rank(result: CheckResult) -> u8 {
	match result {
		CheckResult::Failed => 0,
		CheckResult::Warning => 1,
		CheckResult::Broken => 2,
		CheckResult::Passed => 3,
		CheckResult::Skipped => 4,
	}
}

/// List the applications whose check state reports one (source, check).
///
/// Everything the per-healthcheck page needs: the catalog's configured
/// policy for the (source, check) (if any) plus every live server's
/// current state for it — the real-time picture, with each degraded row
/// carrying `failing_since` (the start of its current degradation
/// streak). This is the data behind the `/healthchecks/:source/:check`
/// "who's affected" page, which doubles as an operator TODO list and as
/// a way to correlate applications sharing the same issue during a
/// fleet-wide incident.
#[utoipa::path(
	post,
	path = "/check_detail",
	operation_id = "status_check_detail",
	tag = "statuses",
	request_body = CheckDetailArgs,
	responses(
		(status = 200, description = "The check's catalog policy and the applications currently reporting it.", body = CheckDetailData),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
pub async fn check_detail(
	State(state): State<AppState>,
	Json(args): Json<CheckDetailArgs>,
) -> Result<Json<CheckDetailData>> {
	let mut conn = state.db_read.get().await?;
	let namespace = Namespace::try_from(&args.namespace)
		.map_err(|e| commons_errors::AppError::BadRequest(e.to_string()))?;
	let states =
		Issue::check_state_for_check(&mut conn, &args.source, &namespace, &args.check).await?;

	// Live applications only: archived applications and canopy's own row never
	// appear on the check detail page.
	let server_ids: Vec<Uuid> = states
		.iter()
		.filter_map(|st| st.application_id)
		.collect::<BTreeSet<_>>()
		.into_iter()
		.collect();
	let live: HashMap<Uuid, database::applications::Application> =
		database::applications::Application::get_by_ids(&mut conn, &server_ids)
			.await?
			.into_iter()
			.filter(|s| s.deleted_at.is_none() && s.id != Uuid::nil())
			.map(|s| (s.id, s))
			.collect();
	// Group names (and, for group-scoped states, rank buckets) cover both
	// the member applications' groups and the groups with their own state.
	let group_ids: Vec<Uuid> = live
		.values()
		.filter_map(|s| s.group_id)
		.chain(states.iter().filter_map(|st| st.server_group_id))
		.collect::<BTreeSet<_>>()
		.into_iter()
		.collect();
	let group_names: HashMap<Uuid, String> = ServerGroup::list_by_ids(&mut conn, &group_ids)
		.await?
		.into_iter()
		.map(|g| (g.id, g.name))
		.collect();
	let scoped_group_ids: Vec<Uuid> = states
		.iter()
		.filter_map(|st| st.server_group_id)
		.collect::<BTreeSet<_>>()
		.into_iter()
		.collect();
	let group_ranks = ServerGroup::highest_member_ranks(&mut conn, &scoped_group_ids).await?;

	let issue_ids: Vec<Uuid> = states.iter().map(|st| st.id).collect();
	let stability =
		database::stability::CheckStability::for_issue_ids(&mut conn, &issue_ids).await?;
	let now = jiff::Timestamp::now();

	// failing_since is the current degradation streak; recovered rows have
	// none. first_seen is the fallback for rows stamped before
	// degraded_since existed.
	let failing_since = |st: &Issue| {
		st.active
			.then_some(st.degraded_since.unwrap_or(st.first_seen))
	};

	let mut applications: Vec<CheckDetailServerData> = Vec::new();
	let mut groups: Vec<CheckDetailGroupData> = Vec::new();
	let mut canopy: Option<CheckDetailCanopyData> = None;
	for st in states {
		let Some(result) = st.observed_result else {
			continue;
		};
		let row_stability = stability
			.get(&st.id)
			.map(|row| database::stability::StabilityData::from_row(row, now));
		match (st.application_id, st.server_group_id) {
			(Some(sid), _) => {
				let Some(server) = live.get(&sid) else {
					continue;
				};
				applications.push(CheckDetailServerData {
					r#type: server.r#type.clone(),
					server_id: server.id,
					server_name: server.display_name(),
					group_id: server.group_id,
					group_name: server.group_id.and_then(|g| group_names.get(&g).cloned()),
					rank: server.rank,
					result,
					data: st.detail.clone().unwrap_or_else(|| serde_json::json!({})),
					failing_since: failing_since(&st),
					status_created_at: st.last_seen,
					stability: row_stability,
				});
			}
			(None, Some(gid)) => {
				groups.push(CheckDetailGroupData {
					group_id: gid,
					group_name: group_names.get(&gid).cloned().unwrap_or_default(),
					rank: group_ranks.get(&gid).copied(),
					result,
					data: st.detail.clone().unwrap_or_else(|| serde_json::json!({})),
					failing_since: failing_since(&st),
					status_created_at: st.last_seen,
					stability: row_stability,
				});
			}
			(None, None) => {
				canopy = Some(CheckDetailCanopyData {
					result,
					data: st.detail.clone().unwrap_or_else(|| serde_json::json!({})),
					failing_since: failing_since(&st),
					status_created_at: st.last_seen,
					stability: row_stability,
				});
			}
		}
	}
	applications.sort_by(|a, b| {
		check_result_rank(a.result)
			.cmp(&check_result_rank(b.result))
			.then_with(|| a.group_name.cmp(&b.group_name))
			.then_with(|| a.server_name.cmp(&b.server_name))
	});
	groups.sort_by(|a, b| a.group_name.cmp(&b.group_name));

	let policy = CheckPolicy::get(&mut conn, &args.source, &namespace, &args.check).await?;

	Ok(Json(CheckDetailData {
		source: args.source,
		check: args.check,
		ceiling: policy.as_ref().map(|p| p.ceiling),
		escalates: policy.as_ref().is_some_and(|p| p.escalates),
		documentation: policy.and_then(|p| p.documentation),
		applications,
		groups,
		canopy,
	}))
}

/// A single status push from a server, as of a point in time, with derived
/// health and version information.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct StatusSnapshotData {
	/// Unique identifier for this status push.
	pub id: Uuid,
	/// When this status push was recorded.
	pub created_at: Timestamp,
	/// Id of the server this status was reported by.
	pub server_id: Uuid,
	/// Id of the device that sent this status push, if known.
	pub device_id: Option<Uuid>,
	/// The application the server runs. Travels with the version so a
	/// consumer can tell a product with no version from one that has yet to
	/// report one.
	// spec: APP#versions
	pub r#type: ApplicationType,
	/// Software version reported in this push.
	pub version: Option<VersionStr>,
	/// How many releases behind the latest published version this push's
	/// version is. Absent when there's no published version to compare
	/// against.
	pub version_distance: Option<u64>,
	/// Minimum embedded browser version required by this software version,
	/// if known.
	pub min_chrome_version: Option<u32>,
	/// Reported operating system platform.
	pub platform: Option<String>,
	/// Reported database engine version.
	pub postgres: Option<String>,
	/// Reported runtime version.
	pub nodejs: Option<String>,
	/// Version of bestool, the agent reporting on the server. Absent when no
	/// source reports one.
	pub bestool: Option<String>,
	/// Reported system timezone.
	pub timezone: Option<String>,
	/// Version the server's reporting schema was built for. Absent until a
	/// server runs a schema that stamps one (spec: RPT#currency).
	pub reporting_schema: Option<String>,
	/// Additional unstructured data reported alongside the snapshot, keyed
	/// by source (`{ [source]: { …fields } }`) so a multi-source snapshot's
	/// raw payloads stay attributed rather than merged. Sources whose
	/// payload is empty are omitted.
	pub extra: serde_json::Value,
	/// Operators identified as connected to the server as of this push,
	/// with display name and profile picture filled in where known. Not
	/// filtered by recency — reflects this specific point-in-time snapshot.
	pub operators: Vec<OperatorPresence>,
	/// The server's consolidated checks as of this snapshot: every source's
	/// checks, graded and classified, with silenced flags and the rolled-up
	/// health state — the same shape the live view uses.
	pub checks: commons_types::status::ConsolidatedChecks,
}

/// Selects a server and a point in time to fetch a status snapshot for.
#[derive(Deserialize, ToSchema)]
pub struct SnapshotArgs {
	/// Id of the server to fetch a status snapshot for.
	pub server_id: Uuid,
	/// Point in time the snapshot should be "as of". Returns the most
	/// recent status reported at or before this time. Omit (or send
	/// `null`) to get the latest status with no time bound.
	#[serde(default)]
	pub at: Option<Timestamp>,
}

/// Get a server's status as of a point in time.
///
/// Returns the most recent status push at or before `at` (or the latest
/// push overall, if `at` is omitted), enriched with version distance,
/// minimum browser version, connected operators, and per-check severities.
/// Returns `null` in the response body if the server has no recorded
/// status.
#[utoipa::path(
	post,
	path = "/snapshot",
	operation_id = "status_snapshot",
	tag = "statuses",
	security(("tailscale-user" = [])),
	request_body = SnapshotArgs,
	responses(
		(status = 200, body = Option<StatusSnapshotData>),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
pub async fn snapshot(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<SnapshotArgs>,
) -> Result<Json<Option<StatusSnapshotData>>> {
	let mut conn = state.db_read.get().await?;
	let status = match args.at {
		Some(at) => Status::at_time(&mut conn, args.server_id, at).await?,
		None => Status::latest_for_server(&mut conn, args.server_id).await?,
	};
	let Some(status) = status else {
		return Ok(Json(None));
	};
	let server = Application::get_by_id(&mut conn, args.server_id).await?;

	// Grading a version means measuring it against a release train canopy
	// holds, so it applies only to a product that has one. A canopy instance
	// reports its own build version and would otherwise be measured against
	// Tamanu's releases, yielding a distance that means nothing.
	//
	// If the Canopy instance has no published versions yet, we just skip
	// the distance computation rather than 404'ing the whole
	// snapshot — the call still wants to surface everything else.
	// spec: APP#versions
	let version_distance = if server.r#type.tracks_versions() {
		match Version::get_latest_matching(&mut conn, "*".parse()?).await {
			Ok(v) => status.distance_from_version(&v.as_semver()),
			Err(_) => None,
		}
	} else {
		None
	};
	// The consolidated checks as of this snapshot: every source's most
	// recent report at-or-before `at`, re-graded through current policy,
	// plus the figures those same reports resolve to. The single `status`
	// above is still used for the push's own metadata (version, operators,
	// etc.).
	let SnapshotState {
		checks,
		by_source_extra,
		figures,
	} = consolidated_checks_at(&mut conn, &server, args.at).await?;

	// Prefer the Node.js version reported in a status payload
	// (`nodeVersion`). Fall back to scraping the User-Agent of the identity
	// reporting on the application's machine: an identity belongs to the box,
	// so what the box's reporter runs is the runtime, while whichever identity
	// filed this push is only that push's provenance. It is the *latest*
	// connection either way — that metadata isn't versioned in lockstep with
	// status pushes, so looking it up "as of" a time would mostly mislead.
	// spec: FIG#figures
	let nodejs = match figures.node_version() {
		Some(v) => Some(v),
		None => match database::machines::Machine::get_by_id(&mut conn, server.machine_id)
			.await?
			.device_id
		{
			Some(dev_id) => {
				DeviceConnection::get_latest_from_device_ids(&mut conn, [dev_id].into_iter())
					.await?
					.into_iter()
					.next()
					.and_then(|d| d.nodejs_version())
			}
			None => None,
		},
	};
	// The embedded-browser floor is a property of a Tamanu release, so it too
	// only means something for a product whose releases canopy holds.
	// spec: APP#versions
	let min_chrome_version = match &status.version {
		Some(v) if server.r#type.tracks_versions() => {
			super::applications::compute_min_chrome_version(&mut conn, v).await
		}
		_ => None,
	};
	let mut operators = status.operators();
	enrich_operators(&mut conn, operators.iter_mut()).await?;

	Ok(Json(Some(StatusSnapshotData {
		id: status.id,
		created_at: status.created_at,
		// The row was selected by this application, so it is the one the push
		// was read for, whether or not the push also carried a machine.
		server_id: args.server_id,
		device_id: status.device_id,
		r#type: server.r#type.clone(),
		// A product with no application version presents none, as against the
		// `unknown` a versioned server shows before it has reported one.
		// spec: APP#versions
		version: status.version.filter(|_| server.r#type.has_versions()),
		version_distance,
		min_chrome_version,
		platform: figures.platform(),
		postgres: figures.postgres_version(),
		nodejs,
		bestool: figures.bestool_version(),
		timezone: figures.timezone(),
		reporting_schema: figures.reporting_schema_version(),
		extra: by_source_extra,
		operators,
		checks,
	})))
}

/// Fill `name`/`profile_pic` on operator entries from the
/// `tailscale_users` cache, in one batch lookup for the lot. Logins that
/// have never authenticated against canopy stay bare — the UI falls back
/// to a letter avatar.
pub(crate) async fn enrich_operators(
	conn: &mut database::diesel_async::AsyncPgConnection,
	operators: impl Iterator<Item = &mut OperatorPresence>,
) -> commons_errors::Result<()> {
	let mut ops: Vec<&mut OperatorPresence> = operators.collect();
	if ops.is_empty() {
		return Ok(());
	}
	let logins: Vec<String> = ops
		.iter()
		.map(|o| o.login.clone())
		.collect::<std::collections::BTreeSet<_>>()
		.into_iter()
		.collect();
	let login_refs: Vec<&str> = logins.iter().map(String::as_str).collect();
	let users = CachedTailscaleUser::by_logins(conn, &login_refs).await?;
	for op in &mut ops {
		if let Some(u) = users.get(&op.login) {
			op.name = Some(u.name.clone());
			op.profile_pic = u.profile_pic.clone();
		}
	}
	Ok(())
}

/// For every warning/failed check on `status`, resolve the policy +
/// rules grading given the snapshot's actual extras and the server's
/// resolved tag map: the effective result ingestion would file. Mirrors
/// the public-server ingestion path (`file_health_events`) so the UI
/// displays what *would* be filed. Broken checks aren't included — they
/// file as warnings and the UI renders them from the result directly.
/// What one pass over a server's per-source statuses yields: everything the
/// snapshot needs from status history, so the pass isn't repeated per figure.
struct SnapshotState {
	checks: commons_types::status::ConsolidatedChecks,
	/// Each source's raw payload, keyed by source.
	by_source_extra: serde_json::Value,
	/// The server-wide figures resolved across those sources.
	figures: MergedDetail,
}

/// Reconstruct a server's consolidated checks as of a point in time (or
/// latest) from status history: each source's most recent report at-or-
/// before `at`, every check re-graded through current policy — the same
/// shape the live path builds from current state. The point-in-time side
/// of the consolidated checks view.
async fn consolidated_checks_at(
	conn: &mut database::diesel_async::AsyncPgConnection,
	server: &Application,
	at: Option<Timestamp>,
) -> commons_errors::Result<SnapshotState> {
	use commons_types::status::{CheckResult, ConsolidatedCheck, ConsolidatedChecks, HealthState};
	use commons_types::subject::CheckSubject;
	use database::check_policies::{CheckPolicy, EvaluationContext, ScopedCheckPolicy};

	let statuses = Status::latest_per_source_at(conn, server.id, at).await?;
	// The box's own reports. A split push files the machine's checks at machine
	// scope rather than on any workload, so they are only here; a reporter
	// still pushing the unified shape files none of these rows and its machine
	// checks are recognised by subject in the application's own. Either shape
	// reconstructs the same list.
	// spec: CHK#a-machines-checks-present-on-its-applications
	let machine = Machine::get_by_id(conn, server.machine_id).await?;
	let machine_statuses = Status::machine_latest_per_source_at(conn, machine.id, at).await?;

	// The figures come from the same set of statuses the checks do, so the
	// snapshot presents each figure as of `at` from whichever source last
	// reported it, rather than from whichever source happened to push last.
	//
	// Both grains' rows, for the reason the checks read both: a split push
	// records the box's detail on the machine's rows, so reading only the
	// application's would drop platform, timezone and the agent's version out
	// of a past moment. A unified push carries them on the application's rows
	// and files no machine rows at all, so nothing is read twice; where a
	// source pushed both shapes across the window, newest-wins per key
	// resolves them as it resolves any two reports.
	// spec: FIG#point-in-time
	let figures = MergedDetail::from_reports(
		statuses
			.iter()
			.chain(machine_statuses.iter())
			.map(|st| (st.created_at, &st.extra)),
	);

	// Each source's raw status-level payload, keyed by source, so the
	// snapshot's raw-payload panel is consolidated rather than one source's
	// blob. Sources with an empty payload are omitted.
	let mut by_source_extra = serde_json::Map::new();
	for status in &statuses {
		if let Some(obj) = status.extra.as_object()
			&& !obj.is_empty()
		{
			by_source_extra.insert(status.source.clone(), status.extra.clone());
		}
	}

	// Tags for rule evaluation, as private-server's other rule-eval sites
	// resolve them. Each grain grades against its own: a rule predicating on a
	// tag reads the box's tags for the box's checks.
	let tags_for = |map: commons_types::server::TagMap| -> std::collections::HashMap<String, serde_json::Value> {
		map.0
			.into_iter()
			.map(|(k, v)| (k, serde_json::Value::String(v)))
			.collect()
	};
	let tags = tags_for(server.tags_merged_with_group(conn).await?);
	let machine_tags = tags_for(machine.tags_merged_with_group(conn).await?);
	// Only present checks backed by a live catalog row, matching the live
	// consolidated view: this drops decommissioned checks and orphaned
	// check-states (a source's catalog rows removed out from under its
	// states) so the point-in-time view reconstructs the same shape.
	let cataloged = CheckPolicy::live_cataloged_pairs(conn).await?;
	// Grading tables loaded once, not once per check: a status can carry
	// dozens of checks, and re-querying the catalog and the scoped chain for
	// each one turned this reconstruction into a few hundred round-trips.
	let grading = CheckPolicy::grading_table(conn).await?;
	// One chain per grain. An application's checks are graded through the
	// application and group chains; the box's through its own and its group's,
	// which need not be the same group.
	let chains = ScopedCheckPolicy::chains_for_scope(
		conn,
		database::check_policies::FilingScope {
			application_id: Some(server.id),
			group_id: server.group_id,
			..Default::default()
		},
	)
	.await?;
	let machine_chains = ScopedCheckPolicy::chains_for_scope(
		conn,
		database::check_policies::FilingScope {
			machine_id: Some(machine.id),
			group_id: machine.group_id,
			..Default::default()
		},
	)
	.await?;

	let mut checks: Vec<ConsolidatedCheck> = Vec::new();
	for (status, from_machine_row) in statuses
		.iter()
		.map(|s| (s, false))
		.chain(machine_statuses.iter().map(|s| (s, true)))
	{
		let Some(arr) = status.health.as_array() else {
			continue;
		};
		let application_silenced = database::silenced_refs::silenced_health_checks_for_server(
			conn,
			Some(server.id),
			server.machine_id,
			server.group_id,
			&status.source,
		)
		.await?;
		let machine_silenced = database::silenced_refs::silenced_health_checks_for_server(
			conn,
			None,
			machine.id,
			machine.group_id,
			&status.source,
		)
		.await?;
		let empty = serde_json::Map::new();
		let status_extra = status.extra.as_object().unwrap_or(&empty).clone();
		for raw in arr {
			let Some(obj) = raw.as_object() else { continue };
			let Some(name) = obj.get("check").and_then(|v| v.as_str()) else {
				continue;
			};
			// What this reading asserts about. A row the box filed is the
			// box's whatever it names; on a unified push the name decides,
			// which is the same rule ingestion applies.
			// spec: STA
			let subject = if from_machine_row {
				CheckSubject::Machine
			} else {
				CheckSubject::of(name)
			};
			// Which catalog entry this reading belongs to follows the
			// reporting application's type, the same as it does on ingest: a
			// machine-subject name lands in the box's entry whatever workload
			// carried it up.
			let namespace = Namespace::for_application(&status.source, name, &server.r#type);
			if !cataloged.contains(&(status.source.clone(), namespace.clone(), name.to_string())) {
				continue;
			}
			let Some(observed) = CheckResult::from_entry(obj) else {
				continue;
			};
			// Re-grade this entry through current policy, mirroring ingestion:
			// the normalised result is injected so rules see a uniform
			// `check.result` even on legacy stored rows.
			let mut check_extra = obj.clone();
			check_extra.remove("check");
			check_extra.remove("healthy");
			check_extra.insert(
				"result".into(),
				serde_json::Value::String(observed.to_string()),
			);
			let ctx = EvaluationContext {
				status_extra: &status_extra,
				check_extra: &check_extra,
				tags: if subject.is_machine() {
					&machine_tags
				} else {
					&tags
				},
			};
			let key = (status.source.clone(), namespace.clone(), name.to_string());
			let fleet = CheckPolicy::grade(grading.get(&key), &status.source, name, observed, &ctx);
			let scoped = if subject.is_machine() {
				&machine_chains
			} else {
				&chains
			};
			let graded = CheckPolicy::chain_scoped(
				fleet,
				scoped.get(&key).map_or(&[][..], Vec::as_slice),
				&ctx,
			);
			// The check's own detail fields, verbatim.
			let mut detail = obj.clone();
			detail.remove("check");
			detail.remove("healthy");
			detail.remove("result");
			let silenced = if subject.is_machine() {
				&machine_silenced
			} else {
				&application_silenced
			};
			checks.push(ConsolidatedCheck {
				silenced: silenced.contains(name),
				qualified_name: namespace.qualified_name(name),
				namespace: (&namespace).into(),
				source: status.source.clone(),
				check: name.to_string(),
				observed: Some(observed),
				effective: graded.effective,
				detail: serde_json::Value::Object(detail),
				subject,
			});
		}
	}
	checks.sort_by(|a, b| {
		a.effective
			.urgency_rank()
			.cmp(&b.effective.urgency_rank())
			.then_with(|| a.source.cmp(&b.source))
			.then_with(|| a.check.cmp(&b.check))
	});
	let health_state =
		HealthState::from_results(checks.iter().filter(|c| !c.silenced).map(|c| c.effective));
	Ok(SnapshotState {
		checks: ConsolidatedChecks {
			health_state,
			checks,
		},
		by_source_extra: serde_json::Value::Object(by_source_extra),
		figures,
	})
}

/// One application's identity and its currently reported detail, as a row of
/// the fleet view.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FleetServerDetailData {
	/// Unique identifier for the server.
	pub server_id: Uuid,
	/// Operator-assigned name for the server, empty when it has none.
	pub server_name: String,
	/// The machine this application runs on. A machine figure is the box's
	/// rather than the workload's, so a row reaches its machine's figures
	/// through this rather than carrying a copy of them.
	// spec: FIG#fleet-spread
	pub machine_id: Uuid,
	/// The group the server belongs to, if any.
	pub group_id: Option<Uuid>,
	/// Display name of that group, if any.
	pub group_name: Option<String>,
	/// Where the server sits in its group's promotion order, if set.
	pub rank: Option<ServerRank>,
	/// The application the server runs. The fleet view reads it to keep the
	/// application-version spread to applications that have one to report.
	// spec: APP#versions
	pub r#type: ApplicationType,
	/// Application version the server reports running, if any.
	pub version: Option<VersionStr>,
	/// Reported database engine version.
	pub postgres: Option<String>,
	/// Reported runtime version.
	pub nodejs: Option<String>,
	/// The application's own configured timezone, as reported. The box's
	/// operating system timezone is a machine figure and is not this.
	pub timezone: Option<String>,
	/// Every field the server's sources currently report, resolved across
	/// them — the raw material behind the derived figures above, and what an
	/// arbitrary-field lookup reads.
	#[schema(additional_properties = true, value_type = Object)]
	pub detail: serde_json::Value,
	/// The application's current healthcheck state, keyed by check name: each
	/// check's reported fields plus its graded `result` and the `observed`
	/// result behind it. This is what a `check.field` lookup reads.
	///
	/// Only the checks filed against the application itself. A check that
	/// asserts something about the box is the machine's row.
	// spec: FIG#fleet-spread
	#[schema(additional_properties = true, value_type = Object)]
	pub checks: serde_json::Value,
}

/// One machine's identity and its currently reported detail, as a row of the
/// fleet view.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FleetMachineDetailData {
	/// Unique identifier for the machine.
	pub machine_id: Uuid,
	/// The name its operator gave it, if any.
	pub machine_name: Option<String>,
	/// The group the machine belongs to, if any.
	pub group_id: Option<Uuid>,
	/// Display name of that group, if any.
	pub group_name: Option<String>,
	/// The operating system the box runs, qualified by its version where one
	/// is reported. A box reporting no operating system falls back to the
	/// family the database engine of an application on it gives away.
	// spec: FIG#machine-figures
	pub platform: Option<String>,
	/// Version of bestool, the agent reporting on the box.
	// spec: FIG#machine-figures
	pub bestool: Option<String>,
	/// Every field the machine's sources currently report, resolved across
	/// them.
	#[schema(additional_properties = true, value_type = Object)]
	pub detail: serde_json::Value,
	/// The machine's current healthcheck state, keyed by check name, in the
	/// same shape as an application's.
	#[schema(additional_properties = true, value_type = Object)]
	pub checks: serde_json::Value,
}

/// Both populations the fleet view spreads its figures over.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FleetDetailData {
	/// Every live machine, canopy's own excluded.
	pub machines: Vec<FleetMachineDetailData>,
	/// Every live application, canopy's own excluded.
	pub applications: Vec<FleetServerDetailData>,
}

/// Get every live machine's and application's currently reported detail.
///
/// One row per machine and one per application, each carrying its derived
/// figures, the full resolved payload its sources report, and its current
/// healthcheck state. This is the data behind the fleet view, which groups it
/// to show how each figure — or any field a source reports, whether against
/// the target itself or on one of its checks — is spread across the fleet,
/// and can cross two fields against each other.
///
/// The two populations are returned separately because a figure spreads at
/// the grain it belongs to: a box running two applications is one machine in
/// a platform spread and two applications in a version spread. An application
/// names the box it runs on so a crossing can put an application figure on a
/// machine axis.
///
/// Reads each source's current report rather than status history, so it
/// covers targets that have been quiet for any length of time. Archived
/// machines and applications, and canopy's own rows, are excluded; a live one
/// that has never reported appears with everything absent.
// spec: FIG#fleet-spread
#[utoipa::path(
	post,
	path = "/fleet_detail",
	operation_id = "status_fleet_detail",
	tag = "statuses",
	security(("tailscale-user" = [])),
	responses(
		(status = 200, description = "Every live machine's and application's currently reported detail.", body = FleetDetailData),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
pub async fn fleet_detail(
	State(state): State<AppState>,
	_user: TailscaleUser,
) -> Result<Json<FleetDetailData>> {
	let mut conn = state.db_read.get().await?;

	// get_all already excludes archived applications and canopy's own row.
	let applications = Application::get_all(&mut conn, 0, None).await?;
	// list_live excludes archived machines but not canopy's own, which is not
	// a box and is not part of the fleet.
	let machines: Vec<Machine> = Machine::list_live(&mut conn)
		.await?
		.into_iter()
		.filter(|m| m.id != Uuid::nil())
		.collect();

	let group_ids: Vec<Uuid> = applications
		.iter()
		.filter_map(|s| s.group_id)
		.chain(machines.iter().filter_map(|m| m.group_id))
		.collect::<BTreeSet<_>>()
		.into_iter()
		.collect();
	let group_names: HashMap<Uuid, String> = ServerGroup::list_by_ids(&mut conn, &group_ids)
		.await?
		.into_iter()
		.map(|g| (g.id, g.name))
		.collect();

	// One read for the whole fleet at each grain: the current-detail tables
	// are a row per (target, source), so this is a few hundred rows however
	// much status history sits behind them. The application read is the
	// application's own detail, without its machine's folded in, so a box's
	// fields don't present as each workload's and inflate a machine figure.
	// spec: FIG#fleet-spread
	let mut merged = ReportedDetail::merge_by_server(ReportedDetail::all_own(&mut conn).await?);
	let mut machine_merged =
		MachineReportedDetail::merge_by_machine(MachineReportedDetail::all(&mut conn).await?);

	// Same again for check state, which is a row per (target, source, check)
	// and carries the fields each check reports.
	let scopes: Vec<(Uuid, Option<Uuid>)> =
		applications.iter().map(|s| (s.id, s.group_id)).collect();
	let mut checks = database::issues::check_detail_by_server(&mut conn, &scopes).await?;
	let machine_scopes: Vec<(Uuid, Option<Uuid>)> =
		machines.iter().map(|m| (m.id, m.group_id)).collect();
	let mut machine_checks =
		database::issues::check_detail_by_machine(&mut conn, &machine_scopes).await?;

	// A box that reports no operating system falls back to the family the
	// database engine of an application on it gives away, so the fallback
	// needs the applications gathered by machine before the rows are built.
	// spec: FIG#machine-figures
	let mut pg_banner_by_machine: HashMap<Uuid, String> = HashMap::new();
	for application in &applications {
		if let Some(banner) = merged
			.get(&application.id)
			.and_then(|(figures, _)| figures.pg_version_banner())
		{
			pg_banner_by_machine
				.entry(application.machine_id)
				.or_insert(banner);
		}
	}

	let machines = machines
		.into_iter()
		.map(|machine| {
			let figures = machine_merged.remove(&machine.id).unwrap_or_default();
			let checks = machine_checks.remove(&machine.id).unwrap_or_default();
			FleetMachineDetailData {
				machine_id: machine.id,
				machine_name: machine.name,
				group_id: machine.group_id,
				group_name: machine.group_id.and_then(|g| group_names.get(&g).cloned()),
				platform: figures.os_platform().or_else(|| {
					pg_banner_by_machine
						.get(&machine.id)
						.map(database::statuses::platform_family_of)
				}),
				bestool: figures.bestool_version(),
				detail: figures.into_json(),
				checks: serde_json::Value::Object(checks),
			}
		})
		.collect();

	let rows = applications
		.into_iter()
		.map(|server| {
			let (figures, version) = merged.remove(&server.id).unwrap_or_default();
			let checks = checks.remove(&server.id).unwrap_or_default();
			FleetServerDetailData {
				server_id: server.id,
				server_name: server.display_name(),
				machine_id: server.machine_id,
				group_id: server.group_id,
				group_name: server.group_id.and_then(|g| group_names.get(&g).cloned()),
				rank: server.rank,
				r#type: server.r#type.clone(),
				// A product with no application version reports none, so the
				// row carries nothing for the fleet view to count.
				// spec: APP#versions
				version: version.filter(|_| server.r#type.has_versions()),
				postgres: figures.postgres_version(),
				nodejs: figures.node_version(),
				timezone: figures.timezone(),
				detail: figures.into_json(),
				checks: serde_json::Value::Object(checks),
			}
		})
		.collect();

	Ok(Json(FleetDetailData {
		machines,
		applications: rows,
	}))
}
