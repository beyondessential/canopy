//! Tools over incident memory: entries recording what past incidents taught,
//! and the index an agent reads before starting an investigation.
//!
//! Spec: `.workhorse/specs/private-server/mcp.md` (id `MCP`), "Incident
//! memory", and `.workhorse/specs/monitoring/incident-memory.md` (id `MEM`).

use commons_errors::AppError;
use database::{
	audit::{self, Action, Record},
	diesel_async::AsyncConnection,
	memory::{
		self, Amendment, DEFAULT_WINDOW_DAYS, Find, Link, LinkKind, MemoryEntry, MemoryEntryData,
		NewEntry, Outcome,
	},
};
use rmcp::{
	handler::server::{tool::Extension, wrapper::Parameters},
	model::{CallToolResult, ErrorData as McpError},
	tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
	CanopyMcp,
	notes::{refusal, require_notes},
	util::{is_not_found, mcp_err, not_found, ok_json, parse_opt_uuid, parse_uuid},
};

const DEFAULT_FIND_LIMIT: i64 = 50;
const MAX_FIND_LIMIT: i64 = 200;

/// What an entry is about.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LinkArg {
	/// A group.
	Group { id: String },
	/// One incident.
	Incident { id: String },
	/// A check, wherever it is filed: its source and its presented name, as
	/// other tools give them.
	Check { source: String, name: String },
}

impl LinkArg {
	fn parse(&self) -> Result<Link, McpError> {
		Ok(match self {
			Self::Group { id } => Link::Group {
				id: parse_uuid(id, "link.id")?,
			},
			Self::Incident { id } => Link::Incident {
				id: parse_uuid(id, "link.id")?,
			},
			Self::Check { source, name } => Link::Check {
				source: source.clone(),
				name: name.clone(),
			},
		})
	}
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindMemoryArgs {
	/// Free text matched against titles and bodies; each word matches words it
	/// begins.
	pub query: Option<String>,
	/// Only entries with this kind of link: `fleet` (none), `group`, `check` or
	/// `incident`.
	pub link_kind: Option<String>,
	/// Only entries linked to this group.
	pub group_id: Option<String>,
	/// Only entries linked to the check with this source (give `check_name` too).
	pub check_source: Option<String>,
	/// Only entries linked to the check with this name (give `check_source` too).
	pub check_name: Option<String>,
	/// Only entries linked to this incident.
	pub incident_id: Option<String>,
	/// Include retired entries, which are otherwise left out.
	pub include_retired: Option<bool>,
	/// The most entries to return; defaults to 50, at most 200.
	pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MemoryEntryIdArgs {
	/// The entry's id.
	pub id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MemoryIndexArgs {
	/// Narrow to one group's incidents, and to the entries that apply to it.
	pub group_id: Option<String>,
	/// How many days of incidents to look back over; defaults to 90.
	pub window_days: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RecordMemoryEntryArgs {
	/// What the entry is, in a line (at most 512 characters).
	pub title: String,
	/// What a later investigator would otherwise rediscover (at most 16,384
	/// characters).
	pub body: String,
	/// What it is about; omit for an entry that applies to the fleet as a whole.
	pub link: Option<LinkArg>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AmendMemoryEntryArgs {
	/// The entry's id.
	pub id: String,
	/// The revision of the entry you read.
	pub revision: i64,
	pub title: Option<String>,
	pub body: Option<String>,
	/// A link to set.
	pub link: Option<LinkArg>,
	/// Set to clear the entry's link, making it apply to the fleet as a whole.
	pub clear_link: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RetireMemoryEntryArgs {
	/// The entry's id.
	pub id: String,
	/// Why the entry no longer holds (at most 500 characters).
	pub reason: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RestoreMemoryEntryArgs {
	/// The entry's id.
	pub id: String,
	/// The revision of the entry you read.
	pub revision: i64,
	/// The earlier version to restore, by revision, from the entry's history.
	pub to_revision: i64,
}

#[derive(Serialize)]
struct MemoryFound {
	entries: Vec<MemoryEntryData>,
	total: i64,
	truncated: bool,
}

#[derive(Serialize)]
struct MemoryDetail {
	entry: MemoryEntryData,
	history: Vec<memory::MemoryRevision>,
}

fn link_kind(kind: &str) -> Result<LinkKind, McpError> {
	match kind {
		"fleet" => Ok(LinkKind::Fleet),
		"group" => Ok(LinkKind::Group),
		"check" => Ok(LinkKind::Check),
		"incident" => Ok(LinkKind::Incident),
		other => Err(McpError::invalid_params(
			format!("invalid link_kind: {other} (fleet, group, check or incident)"),
			None,
		)),
	}
}

/// A write refused for being wrong, as the result the client reads, or the
/// error that is Canopy's own.
fn refused(error: AppError) -> Result<CallToolResult, McpError> {
	match error {
		AppError::BadRequest(message) => Err(McpError::invalid_params(message, None)),
		error if is_not_found(&error) => {
			Ok(not_found("no such memory entry, group or incident".into()))
		}
		AppError::Conflict(message) => Ok(refusal(message, json!(null))),
		error => Err(mcp_err(error)),
	}
}

#[tool_router(router = memory_router, vis = "pub(crate)")]
impl CanopyMcp {
	#[tool(
		annotations(read_only_hint = true),
		description = "Search incident memory: short titled entries recording what past incidents \
		               taught, so an investigation starts from what is already known. Optional \
		               free-text `query` matched against titles and bodies, `link_kind` (fleet, \
		               group, check or incident), `group_id`, a check (`check_source` and \
		               `check_name`), `incident_id`, and `include_retired`. Returns a bounded \
		               list newest amended first, each entry with its id, title, body, link \
		               (resolved to a name alongside its id), revision, and when it was last \
		               amended, and says so when truncated. Call get_memory_index first to see \
		               what exists and where."
	)]
	async fn find_memory(
		&self,
		Parameters(args): Parameters<FindMemoryArgs>,
	) -> Result<CallToolResult, McpError> {
		let mut conn = self.conn().await?;
		let limit = args
			.limit
			.unwrap_or(DEFAULT_FIND_LIMIT)
			.clamp(1, MAX_FIND_LIMIT);
		let check = match (args.check_source, args.check_name) {
			(Some(source), Some(name)) => Some((source, name)),
			(None, None) => None,
			_ => {
				return Err(McpError::invalid_params(
					"a check is given as both check_source and check_name",
					None,
				));
			}
		};
		let found = memory::find(
			&mut conn,
			&Find {
				text: args.query,
				link_kind: args.link_kind.as_deref().map(link_kind).transpose()?,
				group: parse_opt_uuid(&args.group_id, "group_id")?,
				check,
				incident: parse_opt_uuid(&args.incident_id, "incident_id")?,
				relevant_to_incident: None,
				include_retired: args.include_retired.unwrap_or(false),
				limit,
			},
		)
		.await
		.map_err(mcp_err)?;
		let truncated = found.total > found.entries.len() as i64;
		ok_json(&MemoryFound {
			entries: memory::present(&mut conn, found.entries)
				.await
				.map_err(mcp_err)?,
			total: found.total,
			truncated,
		})
	}

	#[tool(
		annotations(read_only_hint = true),
		description = "One incident memory entry in full, with its history: every version it has \
		               had, every retirement and restoration, each with who made it, how, and \
		               when, newest first."
	)]
	async fn get_memory_entry(
		&self,
		Parameters(args): Parameters<MemoryEntryIdArgs>,
	) -> Result<CallToolResult, McpError> {
		let id = parse_uuid(&args.id, "id")?;
		let mut conn = self.conn().await?;
		let Ok(entry) = memory::get(&mut conn, id).await else {
			return Ok(not_found(format!("no memory entry with id {id}")));
		};
		let history = memory::history(&mut conn, id).await.map_err(mcp_err)?;
		ok_json(&MemoryDetail {
			entry: memory::present_one(&mut conn, entry)
				.await
				.map_err(mcp_err)?,
			history,
		})
	}

	#[tool(
		annotations(read_only_hint = true),
		description = "Start an investigation here: what is already known, where it applies, and \
		               where recurring trouble has no memory yet. Returns (1) `entries`: every \
		               current memory entry in compact form (id, title, link resolved to a name, \
		               when last amended) so you choose which to read in full with \
		               get_memory_entry; (2) `checks`: the checks that have been part of the most \
		               incidents in the window, each with how many incidents, how many were \
		               published, when the latest was, which groups they were on, and \
		               `linked_entries`, how many memory entries are linked to the check; (3) \
		               `groups`: the groups with the most incidents in the window, with incident \
		               and published counts and `linked_entries`; (4) `incidents_with_notes`: \
		               incidents in the window carrying operator or agent notes, each with its \
		               target, status, how many notes, and when the latest was written, so you \
		               can find where a person has already said something worth reading. A check \
		               or group with `linked_entries` of 0 has had trouble and has no memory: a \
		               gap, not an absence from the list. `group_id` narrows lists 2 to 4 to that \
		               group's incidents and list 1 to entries that are fleet-wide or link to the \
		               group, one of its incidents, or a check it has had incidents on. Incident \
		               counts follow find_incidents: count `published` ones, not raw rows. \
		               `window_days` defaults to 90."
	)]
	async fn get_memory_index(
		&self,
		Parameters(args): Parameters<MemoryIndexArgs>,
	) -> Result<CallToolResult, McpError> {
		let mut conn = self.conn().await?;
		let group = parse_opt_uuid(&args.group_id, "group_id")?;
		let index = memory::index(
			&mut conn,
			group,
			args.window_days.unwrap_or(DEFAULT_WINDOW_DAYS),
		)
		.await
		.map_err(mcp_err)?;
		ok_json(&index)
	}

	#[tool(
		annotations(
			read_only_hint = false,
			destructive_hint = false,
			idempotent_hint = false
		),
		description = "Record an incident memory entry: what a later investigator would otherwise \
		               rediscover, such as a recurring cause, how one check's failure was told \
		               apart from another, a standing quirk of a site, or where a fix is kept. \
		               Not a log of what happened during one incident, which is that incident's \
		               own notes (`add_incident_note`). Check `find_memory` first and amend an \
		               entry that already says it rather than adding another. `title` is at most \
		               512 characters and `body` at most 16,384; one over is refused with the \
		               limit named. An optional `link` ties the entry to a group, a check or one \
		               incident; with none it applies to the fleet as a whole. Returns the entry. \
		               Refused, with no effect, unless the caller is an administrator (on the \
		               operator surface) or holds a token with the notes grant."
	)]
	async fn record_memory_entry(
		&self,
		parts: Extension<http::request::Parts>,
		Parameters(args): Parameters<RecordMemoryEntryArgs>,
	) -> Result<CallToolResult, McpError> {
		let caller = require_notes(parts).await?;
		let link = args.link.as_ref().map(LinkArg::parse).transpose()?;
		let actor = caller.actor();
		let mut conn = self.db_write.get().await.map_err(mcp_err)?;
		let entry = conn
			.transaction::<_, AppError, _>(async |conn| {
				let entry = memory::record(
					conn,
					NewEntry {
						title: args.title.clone(),
						body: args.body.clone(),
						link,
					},
					caller.notes_author(),
				)
				.await?;
				audit_entry(conn, &actor, &entry, &caller, EntryAction::Recorded).await?;
				Ok(entry)
			})
			.await;
		match entry {
			Ok(entry) => ok_json(
				&memory::present_one(&mut conn, entry)
					.await
					.map_err(mcp_err)?,
			),
			Err(error) => refused(error),
		}
	}

	#[tool(
		annotations(
			read_only_hint = false,
			destructive_hint = false,
			idempotent_hint = false
		),
		description = "Amend an incident memory entry's title, body or link, stating the \
		               `revision` you read. Fields you leave out are kept; `clear_link` makes \
		               the entry fleet-wide. If the entry changed since the revision you read, \
		               the amendment is refused and the result carries the entry as it stands, so \
		               you can reconcile and try again. A retired entry cannot be amended: \
		               restore it first. Refused, with no effect, unless the caller is an \
		               administrator (on the operator surface) or holds a token with the notes \
		               grant."
	)]
	async fn amend_memory_entry(
		&self,
		parts: Extension<http::request::Parts>,
		Parameters(args): Parameters<AmendMemoryEntryArgs>,
	) -> Result<CallToolResult, McpError> {
		let caller = require_notes(parts).await?;
		let id = parse_uuid(&args.id, "id")?;
		let link = match (&args.link, args.clear_link.unwrap_or(false)) {
			(Some(_), true) => {
				return Err(McpError::invalid_params(
					"give a link or clear_link, not both",
					None,
				));
			}
			(Some(link), false) => Some(Some(link.parse()?)),
			(None, true) => Some(None),
			(None, false) => None,
		};
		let actor = caller.actor();
		let mut conn = self.db_write.get().await.map_err(mcp_err)?;
		let result = conn
			.transaction::<_, AppError, _>(async |conn| {
				let outcome = memory::amend(
					conn,
					id,
					args.revision,
					Amendment {
						title: args.title.clone(),
						body: args.body.clone(),
						link,
					},
					caller.notes_author(),
				)
				.await?;
				if let Outcome::Changed(entry) = &outcome {
					audit_entry(conn, &actor, entry, &caller, EntryAction::Amended).await?;
				}
				Ok(outcome)
			})
			.await;
		answer(&mut conn, result, args.revision).await
	}

	#[tool(
		annotations(
			read_only_hint = false,
			destructive_hint = false,
			idempotent_hint = true
		),
		description = "Retire an incident memory entry that no longer holds, recording why. It is \
		               kept in full with its history and left out of find_memory unless \
		               `include_retired` is set, and can be brought back with \
		               `restore_memory_entry`. Refused, with no effect, unless the caller is an \
		               administrator (on the operator surface) or holds a token with the notes \
		               grant."
	)]
	async fn retire_memory_entry(
		&self,
		parts: Extension<http::request::Parts>,
		Parameters(args): Parameters<RetireMemoryEntryArgs>,
	) -> Result<CallToolResult, McpError> {
		let caller = require_notes(parts).await?;
		let id = parse_uuid(&args.id, "id")?;
		let actor = caller.actor();
		let mut conn = self.db_write.get().await.map_err(mcp_err)?;
		let entry = conn
			.transaction::<_, AppError, _>(async |conn| {
				let entry = memory::retire(conn, id, &args.reason, caller.notes_author()).await?;
				audit_entry(conn, &actor, &entry, &caller, EntryAction::Retired).await?;
				Ok(entry)
			})
			.await;
		match entry {
			Ok(entry) => ok_json(
				&memory::present_one(&mut conn, entry)
					.await
					.map_err(mcp_err)?,
			),
			Err(error) => refused(error),
		}
	}

	#[tool(
		annotations(
			read_only_hint = false,
			destructive_hint = false,
			idempotent_hint = false
		),
		description = "Restore an incident memory entry to an earlier version of itself, given by \
		               `to_revision` from its history (get_memory_entry), stating the `revision` \
		               you read. It gives the entry that version's title, body and link and makes \
		               it current again if it was retired; the restoration is a new version in the \
		               history, not a rewind. If the entry changed since the revision you read, \
		               the restoration is refused and the result carries the entry as it stands. \
		               Refused, with no effect, unless the caller is an administrator (on the \
		               operator surface) or holds a token with the notes grant."
	)]
	async fn restore_memory_entry(
		&self,
		parts: Extension<http::request::Parts>,
		Parameters(args): Parameters<RestoreMemoryEntryArgs>,
	) -> Result<CallToolResult, McpError> {
		let caller = require_notes(parts).await?;
		let id = parse_uuid(&args.id, "id")?;
		let actor = caller.actor();
		let mut conn = self.db_write.get().await.map_err(mcp_err)?;
		let result = conn
			.transaction::<_, AppError, _>(async |conn| {
				let outcome = memory::restore(
					conn,
					id,
					args.revision,
					args.to_revision,
					caller.notes_author(),
				)
				.await?;
				if let Outcome::Changed(entry) = &outcome {
					audit_entry(
						conn,
						&actor,
						entry,
						&caller,
						EntryAction::Restored(args.to_revision),
					)
					.await?;
				}
				Ok(outcome)
			})
			.await;
		answer(&mut conn, result, args.revision).await
	}
}

/// What was done to an entry, for the audit action that records it.
enum EntryAction {
	Recorded,
	Amended,
	Retired,
	Restored(i64),
}

async fn audit_entry(
	conn: &mut database::diesel_async::AsyncPgConnection,
	actor: &audit::Actor,
	entry: &MemoryEntry,
	caller: &crate::caller::Caller,
	what: EntryAction,
) -> Result<(), AppError> {
	let named = Record::memory_entry(entry);
	let link = Record::memory_link(conn, entry).await?;
	let (revision, via) = (
		entry.revision,
		caller.notes_author().via.as_str().to_owned(),
	);
	let action = match what {
		EntryAction::Recorded => Action::MemoryEntryRecorded {
			entry: named,
			link,
			revision,
			via,
		},
		EntryAction::Amended => Action::MemoryEntryAmended {
			entry: named,
			link,
			revision,
			via,
		},
		EntryAction::Retired => Action::MemoryEntryRetired {
			entry: named,
			link,
			revision,
			via,
		},
		EntryAction::Restored(restored_from) => Action::MemoryEntryRestored {
			entry: named,
			link,
			revision,
			restored_from,
			via,
		},
	};
	audit::record(conn, actor, &action).await?;
	Ok(())
}

/// The result of an amendment or restoration: the entry, or why not.
async fn answer(
	conn: &mut database::diesel_async::AsyncPgConnection,
	result: Result<Outcome, AppError>,
	revision_read: i64,
) -> Result<CallToolResult, McpError> {
	match result {
		Ok(Outcome::Changed(entry) | Outcome::Unchanged(entry)) => {
			ok_json(&memory::present_one(conn, entry).await.map_err(mcp_err)?)
		}
		Ok(Outcome::Stale(entry)) => {
			let message = format!(
				"the entry changed since revision {revision_read} was read; it is now at revision {}",
				entry.revision
			);
			let current = memory::present_one(conn, entry).await.map_err(mcp_err)?;
			Ok(refusal(
				message,
				serde_json::to_value(&current).unwrap_or_default(),
			))
		}
		Err(error) => refused(error),
	}
}
