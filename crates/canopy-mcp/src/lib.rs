//! Read-only MCP (Model Context Protocol) query interface over the fleet.
//!
//! Spec: `.workhorse/specs/private-server/mcp.md` (id `MCP`).
//!
//! Mounted twice: at `/api/mcp` on the operator surface, behind the
//! tagged-device guard and an "any tailnet user" gate (private-server's
//! `mcp::require_tailnet_user`), and at `/mcp` on the internet-facing
//! surface behind the bearer-token gate (public-server's `mcp` module).
//! Every tool only reads; nothing here mutates the fleet.
//!
//! Tools call the existing `database` read functions directly and shape lean,
//! agent-legible JSON. The one piece of logic that must NOT be reimplemented is
//! backup staleness ("overdue" / "never reported"): `find_backup_problems` and
//! `fleet_summary` (in the `fleet` module) reuse [`database::backup::staleness`]
//! so the verdicts match what the operator UI and the alerting sweep present.
//!
//! Tools are grouped into domain modules (`applications`, `groups`, `versions`,
//! `fleet`, `backups`, `restore`, `incidents`, `upgrade_plans`), each
//! contributing its own tool router (via rmcp's
//! `#[tool_router(router = ..., vis = "pub(crate)")]`) that [`CanopyMcp::new`]
//! combines into the single stored `ToolRouter`. `util` holds helpers shared
//! across more than one of those modules.

mod applications;
mod backups;
mod fleet;
mod groups;
mod incidents;
mod machines;
mod restore;
mod upgrade_plans;
mod util;
mod versions;

use database::diesel_async::AsyncPgConnection;
use rmcp::{
	ServerHandler,
	handler::server::router::tool::ToolRouter,
	model::{ErrorData as McpError, ServerCapabilities, ServerInfo},
	tool_handler,
	transport::streamable_http_server::{
		StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
	},
};

#[derive(Clone)]
pub struct CanopyMcp {
	db: database::Db,
	tool_router: ToolRouter<CanopyMcp>,
}

impl CanopyMcp {
	pub fn new(db: database::Db) -> Self {
		Self {
			db,
			tool_router: Self::applications_router()
				+ Self::machines_router()
				+ Self::groups_router()
				+ Self::versions_router()
				+ Self::fleet_router()
				+ Self::backups_router()
				+ Self::restore_router()
				+ Self::incidents_router()
				+ Self::upgrade_plans_router(),
		}
	}

	async fn conn(&self) -> Result<impl std::ops::DerefMut<Target = AsyncPgConnection>, McpError> {
		self.db.get().await.map_err(util::mcp_err)
	}
}

#[tool_handler(router = self.tool_router.clone())]
impl ServerHandler for CanopyMcp {
	fn get_info(&self) -> ServerInfo {
		let mut info = ServerInfo::default();
		info.instructions = Some(
			"Read-only access to the Canopy fleet: machines, applications, groups, health/status, \
			 Tamanu versions, planned upgrades, backups, and incidents/issues. All data is live. \
			 Use find_* to locate entities and get_* for detail; fleet_summary and \
			 find_backup_problems for triage.\n\n\
			 Machines and applications are different things and both are queryable. A machine is \
			 the box; an application is the software running on it, and a box can carry more than \
			 one. Ask about a machine (find_machines, get_machine) for platform, disks, memory, \
			 clock, addresses, and backups, which belong to the box. Ask about an application \
			 (find_servers, get_server) for its version, database engine, and product role. Each \
			 result names the other side, so you can move between them without searching. An \
			 issue carries a `scope` saying which of the two it is about, and a machine's checks \
			 also count toward the health of every application on it.\n\n\
			 Upgrade plans: a plan is a group's recorded intention to move to a version. A \
			 planned date is a plan rather than a deadline, so one that has passed (`late`) is \
			 normal operational reality rather than an incident. list_upgrade_plans gives what \
			 every group is going to next; get_upgrade_plan_history gives what a group \
			 planned before, including plans an operator withdrew.\n\n\
			 Products: a server's `product` is the application it runs (tamanu, senaite, or \
			 canopy itself), and its `kind` is its role within that product. The version tools \
			 cover Tamanu's releases; a server whose product has no application version reports \
			 none, which is not the same as a Tamanu server that has yet to report one.\n\n\
			 Incidents: an incident groups the issues active on one target over a span of time. \
			 A target is one of a group's environments (its applications at one rank), the group \
			 itself for its own checks, or canopy; each incident carries the `rank` it is on. \
			 find_incidents returns everything open in the window, including heavy sub-grace \
			 flapping that was recorded but never surfaced. When summarizing or ranking, count \
			 `published` incidents (also given as `published_count`), not raw rows: an incident is \
			 published only if it outlived the group's grace window (~3 min default) or escalated. \
			 A count dominated by unpublished short-lived incidents usually means a twitchy \
			 threshold rather than a real outage."
				.into(),
		);
		info.capabilities = ServerCapabilities::builder().enable_tools().build();
		info
	}
}

// ---------------------------------------------------------------------------
// Service wiring.
// ---------------------------------------------------------------------------

/// Build the tower service nested into an axum router (`/api/mcp` on the
/// operator surface, `/mcp` on the internet-facing one). Auth is the mount's
/// business, not this service's.
pub fn service(db: database::Db) -> StreamableHttpService<CanopyMcp, LocalSessionManager> {
	let mut config = StreamableHttpServerConfig::default();
	// Stateless: each request is self-contained, with no server-side session.
	// The default stateful mode keeps sessions in process memory and 404s
	// ("Session not found") any follow-up request that a load balancer routes to
	// a different replica than the one that handled `initialize` — which is
	// exactly what a multi-replica Canopy instance behind the Tailscale ingress does.
	// This is a read-only request/response API with no server-initiated push, so
	// sessions buy us nothing.
	config.stateful_mode = false;
	// Return plain `application/json` per request instead of an SSE stream. With
	// no streaming there's no long-lived response for a proxy to buffer or drop.
	config.json_response = true;
	// rmcp's `allowed_hosts` defaults to loopback only — a DNS-rebinding defense
	// aimed at browser-facing localhost MCP applications. That threat doesn't apply
	// here: both mounts sit behind an authenticating gate (tailnet identity on
	// the operator surface, bearer tokens on the internet-facing one) and serve
	// no CORS headers, so a browser can't make a credentialed cross-origin POST.
	// Left as-is the loopback default 403s the real instance's hostnames, so disable
	// it. An operator who wants to pin the Host allowlist anyway can set
	// CANOPY_MCP_ALLOWED_HOSTS to a comma-separated list (e.g.
	// `canopy.example.ts.net`); loopback stays allowed for dev.
	match std::env::var("CANOPY_MCP_ALLOWED_HOSTS") {
		Ok(list) if !list.trim().is_empty() => {
			config.allowed_hosts.extend(
				list.split(',')
					.map(str::trim)
					.filter(|s| !s.is_empty())
					.map(ToOwned::to_owned),
			);
		}
		_ => config = config.disable_allowed_hosts(),
	}

	StreamableHttpService::new(
		move || Ok(CanopyMcp::new(db.clone())),
		LocalSessionManager::default().into(),
		config,
	)
}
