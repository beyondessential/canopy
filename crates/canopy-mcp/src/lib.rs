//! MCP (Model Context Protocol) query interface over the fleet.
//!
//! Spec: `.workhorse/specs/private-server/mcp.md` (id `MCP`).
//!
//! Mounted twice: at `/api/mcp` on the operator surface, behind the
//! tagged-device guard and an "any tailnet user" gate (private-server's
//! `mcp::require_tailnet_user`), and at `/mcp` on the internet-facing
//! surface behind the bearer-token gate (public-server's `mcp` module).
//! Every tool reads, except the three that record, withdraw and edit
//! vulnerability triage decisions, and the ones that write notes and incident
//! memory (see [`AUDITED_TOOLS`]); they are held to the caller's standing (see
//! [`Caller`]). The interface also offers one prompt,
//! `triage_vulnerabilities`, in the `vulnerabilities` module.
//!
//! Tools call the existing `database` read functions directly and shape lean,
//! agent-legible JSON. The one piece of logic that must NOT be reimplemented is
//! backup staleness ("overdue" / "never reported"): `find_backup_problems` and
//! `fleet_summary` (in the `fleet` module) reuse [`database::backup::staleness`]
//! so the verdicts match what the operator UI and the alerting sweep present.
//!
//! Tools are grouped into domain modules (`applications`, `groups`, `versions`,
//! `fleet`, `backups`, `restore`, `incidents`, `upgrade_plans`, `notes`, `memory`), each
//! contributing its own tool router (via rmcp's
//! `#[tool_router(router = ..., vis = "pub(crate)")]`) that [`CanopyMcp::new`]
//! combines into the single stored `ToolRouter`. `util` holds helpers shared
//! across more than one of those modules.

mod alert_triage;
mod applications;
mod audit;
mod backups;
mod caller;
mod fleet;
mod groups;
mod incidents;
mod machines;
mod memory;
mod notes;
mod restore;
mod upgrade_plans;
mod util;
mod versions;
mod vulnerabilities;

pub use caller::{AdminCheck, Caller};

use database::diesel_async::AsyncPgConnection;
use rmcp::{
	ServerHandler,
	handler::server::router::{prompt::PromptRouter, tool::ToolRouter},
	model::{ErrorData as McpError, ServerCapabilities, ServerInfo},
	prompt_handler, tool_handler,
	transport::streamable_http_server::{
		StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
	},
};

/// The tools that are audited, as against silent. Every tool but these reads,
/// and a read is audited only where its spec names the action (see the AUD
/// spec). A tool that changes something is listed here and records through the
/// audit log; the test below holds every other tool to being a read.
pub const AUDITED_TOOLS: &[&str] = &[
	"record_vulnerability_decisions",
	"withdraw_vulnerability_decisions",
	"edit_vulnerability_decision",
	"add_issue_note",
	"add_incident_note",
	"edit_note",
	"delete_note",
	"replace_notes",
	"record_memory_entry",
	"amend_memory_entry",
	"retire_memory_entry",
	"restore_memory_entry",
	"record_alert_triage",
	"attach_alert_triage_report",
];

#[derive(Clone)]
pub struct CanopyMcp {
	db: database::Db,
	/// The primary, for the tools that write. Reads may come from a replica.
	db_write: database::Db,
	tool_router: ToolRouter<CanopyMcp>,
	prompt_router: PromptRouter<CanopyMcp>,
}

impl CanopyMcp {
	pub fn new(db: database::Db, db_write: database::Db) -> Self {
		Self {
			db,
			db_write,
			tool_router: Self::applications_router()
				+ Self::audit_router()
				+ Self::machines_router()
				+ Self::groups_router()
				+ Self::versions_router()
				+ Self::fleet_router()
				+ Self::backups_router()
				+ Self::restore_router()
				+ Self::incidents_router()
				+ Self::upgrade_plans_router()
				+ Self::vulnerabilities_router()
				+ Self::notes_router()
				+ Self::memory_router()
				+ Self::alert_triage_router(),
			prompt_router: Self::vulnerabilities_prompts(),
		}
	}

	async fn conn(&self) -> Result<impl std::ops::DerefMut<Target = AsyncPgConnection>, McpError> {
		self.db.get().await.map_err(util::mcp_err)
	}
}

#[tool_handler(router = self.tool_router.clone())]
#[prompt_handler(router = self.prompt_router.clone())]
impl ServerHandler for CanopyMcp {
	fn get_info(&self) -> ServerInfo {
		let mut info = ServerInfo::default();
		info.instructions = Some(
			"Access to the Canopy fleet: machines, applications, groups, health/status, \
			 Tamanu versions, planned upgrades, backups, incidents/issues, and vulnerabilities. \
			 All data is live; every tool reads except the three that change vulnerability triage \
			 decisions (record_, withdraw_ and edit_) and those that write notes and incident \
			 memory. \
			 Use find_* to locate entities and get_* for detail; fleet_summary and \
			 find_backup_problems for triage.\n\n\
			 Machines and applications are different things and both are queryable. A machine is \
			 the box; an application is the software running on it, and a box can carry more than \
			 one. Ask about a machine (find_machines, get_machine) for platform, disks, memory, \
			 clock, addresses, and backups, which belong to the box. Ask about an application \
			 (find_servers, get_server) for its version, database engine, and product role. Each \
			 result names the other side, so you can move between them without searching. An \
			 issue carries a `scope` saying which of the two it is about. A machine's checks count \
			 toward the machine's health and are read on the machine, not on the applications on it.\n\n\
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
			 A target is one of a group's environments (its members at one rank, the group's own \
			 checks included in its headline environment) or canopy; each incident carries the \
			 `rank` it is on. \
			 find_incidents returns everything open in the window, including heavy sub-grace \
			 flapping that was recorded but never surfaced. When summarizing or ranking, count \
			 `published` incidents (also given as `published_count`), not raw rows: an incident is \
			 published only if it outlived the group's grace window (~3 min default) or escalated. \
			 A count dominated by unpublished short-lived incidents usually means a twitchy \
			 threshold rather than a real outage.\n\n\
			 Vulnerabilities: an item is an OSV advisory within a source, which is a tracked \
			 repository (`tamanu`, `canopy`, and so on: the dependencies on its default branch) \
			 or `linux` (packages installed on machines). Only production counts as exposure. \
			 find_vulnerabilities pages through them, with filters, a search, `fields` to keep \
			 the result small and `group_by` to count instead of list; an item with status \
			 `open` needs triage. get_vulnerability gives one in full. The model's suggestions \
			 on items were made without access to source code: they are inferences to check, \
			 not findings. A decision is whole, or split into entries per workspace package. \
			 record_vulnerability_decisions takes a batch, each item judged on its own, \
			 withdraw_vulnerability_decisions takes decisions back, and \
			 edit_vulnerability_decision corrects a reason or issue link; only an administrator \
			 or a token with the triage grant may call them. To triage a repository you have \
			 checked out, use the `triage_vulnerabilities` prompt, which takes the repository.\n\n\
			 Notes and memory: groups, machines and applications carry free-text notes (read \
			 with their `notes_revision`, replaced with replace_notes and the revision you \
			 read, with get_notes_history for the log of changes), and issues and incidents \
			 carry notes (add_issue_note, add_incident_note; edit_note and delete_note only on \
			 notes agents wrote). Incident memory is separate: short titled entries on what \
			 past incidents taught. Start an investigation with get_memory_index, read what \
			 applies with find_memory and get_memory_entry, and record what a later \
			 investigator would otherwise rediscover with record_memory_entry, amending an \
			 existing entry rather than adding a duplicate. These writes need an administrator \
			 or a token with the notes grant.\n\n\
			 Alert triage: for every Canopy incident notification you are asked to triage, \
			 call triage_alert with the incident id from the notification's link before \
			 deciding anything, and follow its `guidance`. You make the decision; Canopy \
			 gathers the context and keeps the record. find_alert_triage_verdicts gives a \
			 write-up day's verdicts for the morning summary."
				.into(),
		);
		info.capabilities = ServerCapabilities::builder()
			.enable_tools()
			.enable_prompts()
			.build();
		info
	}
}

// ---------------------------------------------------------------------------
// Service wiring.
// ---------------------------------------------------------------------------

/// Build the tower service nested into an axum router (`/api/mcp` on the
/// operator surface, `/mcp` on the internet-facing one). Auth is the mount's
/// business, not this service's.
pub fn service(
	db: database::Db,
	db_write: database::Db,
) -> StreamableHttpService<CanopyMcp, LocalSessionManager> {
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
		move || Ok(CanopyMcp::new(db.clone(), db_write.clone())),
		LocalSessionManager::default().into(),
		config,
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn every_tool_is_audited_or_a_silent_read() {
		let tools = CanopyMcp::new(
			database::init_to("postgres://localhost/unused"),
			database::init_to("postgres://localhost/unused"),
		)
		.tool_router
		.list_all();
		assert!(!tools.is_empty());
		for tool in tools {
			let silent_read = tool
				.annotations
				.as_ref()
				.and_then(|annotations| annotations.read_only_hint)
				== Some(true);
			assert!(
				silent_read || AUDITED_TOOLS.contains(&tool.name.as_ref()),
				"tool {} is neither a read nor declared audited",
				tool.name
			);
		}
	}
}
