//! Tools that write notes: on issues and incidents, and on a group, machine or
//! application, and the one that reads a record's notes history.
//!
//! Spec: `.workhorse/specs/private-server/mcp.md` (id `MCP`), "Notes".
//!
//! All the writes are held by the notes grant (see [`Caller::may_note`]), and
//! each records the audit action it takes in the transaction that carries the
//! change.

use commons_errors::AppError;
use commons_types::Uuid;
use database::{
	audit::{self, Action, Record},
	diesel_async::{AsyncConnection, AsyncPgConnection},
	issues::Incident,
	notes::{AuthorKind, Edited, IncidentNote, IssueNote},
	notes_log::{self, NotesRecord, Outcome},
};
use jiff::Timestamp;
use rmcp::{
	handler::server::{tool::Extension, wrapper::Parameters},
	model::{CallToolResult, ContentBlock, ErrorData as McpError},
	tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
	CanopyMcp,
	caller::{self, Caller},
	util::{is_not_found, mcp_err, not_found, ok_json, parse_uuid},
};

/// The most notes an issue or incident read returns.
pub(crate) const NOTES_LIMIT: i64 = 100;

/// A note, as a read returns it.
#[derive(Serialize)]
pub(crate) struct NoteOut {
	pub id: Uuid,
	/// The operator's login, or the name of the token an agent wrote it through.
	pub author: String,
	/// Whether an `operator` or an `agent` wrote it. Only a note an agent wrote
	/// can be edited or deleted here.
	pub author_kind: AuthorKind,
	pub body: String,
	/// Changes whenever the note is edited; an edit states the one it read.
	pub revision: i64,
	pub created_at: Timestamp,
}

impl From<IssueNote> for NoteOut {
	fn from(note: IssueNote) -> Self {
		Self {
			id: note.id,
			author: note.author,
			author_kind: note.author_kind,
			body: note.body,
			revision: note.revision,
			created_at: note.created_at,
		}
	}
}

impl From<IncidentNote> for NoteOut {
	fn from(note: IncidentNote) -> Self {
		Self {
			id: note.id,
			author: note.author,
			author_kind: note.author_kind,
			body: note.body,
			revision: note.revision,
			created_at: note.created_at,
		}
	}
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AddIssueNoteArgs {
	/// The issue's id.
	pub issue_id: String,
	/// The note's text.
	pub body: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AddIncidentNoteArgs {
	/// The incident's id.
	pub incident_id: String,
	/// The note's text.
	pub body: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EditNoteArgs {
	/// The note's id, of a note on an issue or an incident.
	pub note_id: String,
	/// The revision of the note you read.
	pub revision: i64,
	/// The note's new text.
	pub body: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteNoteArgs {
	/// The note's id, of a note on an issue or an incident.
	pub note_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReplaceNotesArgs {
	/// The id of the group, machine or application whose notes to replace.
	pub id: String,
	/// The new notes, replacing what is there.
	pub notes: String,
	/// The revision of the notes you read, from `get_group`, `get_machine` or
	/// `get_server`.
	pub revision: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct NotesHistoryArgs {
	/// The id of a group, machine or application.
	pub id: String,
}

#[derive(Serialize)]
struct NotesHistory {
	id: Uuid,
	kind: &'static str,
	revision: i64,
	revisions: Vec<notes_log::NotesRevision>,
}

/// The notes grant, or a refusal naming it.
pub(crate) async fn require_notes(
	parts: Extension<http::request::Parts>,
) -> Result<Caller, McpError> {
	let caller = caller::of(parts)?;
	if !caller.may_note().await.map_err(mcp_err)? {
		return Err(McpError::invalid_request(
			"changing notes or memory needs an administrator, or a token with the notes grant",
			None,
		));
	}
	Ok(caller)
}

/// A refused write, as a result the client reads: what happened, and the
/// current state to reconcile with. The current state is in the text as well
/// as the structured content, since many clients show the model only the text.
pub(crate) fn refusal(message: impl Into<String>, current: serde_json::Value) -> CallToolResult {
	let message = message.into();
	let mut content = vec![ContentBlock::text(message.clone())];
	if !current.is_null() {
		let text = serde_json::to_string_pretty(&current).unwrap_or_default();
		content.push(ContentBlock::text(format!("Current state:\n{text}")));
	}
	let mut result = CallToolResult::error(content);
	result.structured_content = Some(json!({ "error": message, "current": current }));
	result
}

fn body(text: &str) -> Result<&str, McpError> {
	if text.trim().is_empty() {
		return Err(McpError::invalid_params("the text is empty", None));
	}
	Ok(text)
}

/// A note on an issue or incident, whichever the id names.
enum AnyNote {
	Issue(IssueNote),
	Incident(IncidentNote),
}

impl AnyNote {
	async fn find(conn: &mut AsyncPgConnection, id: Uuid) -> Result<Option<Self>, AppError> {
		match IssueNote::get(conn, id).await {
			Ok(note) => return Ok(Some(Self::Issue(note))),
			Err(error) if is_not_found(&error) => {}
			Err(error) => return Err(error),
		}
		match IncidentNote::get(conn, id).await {
			Ok(note) => Ok(Some(Self::Incident(note))),
			Err(error) if is_not_found(&error) => Ok(None),
			Err(error) => Err(error),
		}
	}

	fn author_kind(&self) -> AuthorKind {
		match self {
			Self::Issue(note) => note.author_kind,
			Self::Incident(note) => note.author_kind,
		}
	}

	fn deleted(&self) -> bool {
		match self {
			Self::Issue(note) => note.deleted_at.is_some(),
			Self::Incident(note) => note.deleted_at.is_some(),
		}
	}

	fn author(&self) -> &str {
		match self {
			Self::Issue(note) => &note.author,
			Self::Incident(note) => &note.author,
		}
	}

	fn id(&self) -> Uuid {
		match self {
			Self::Issue(note) => note.id,
			Self::Incident(note) => note.id,
		}
	}

	async fn target(&self, conn: &mut AsyncPgConnection) -> Result<Record, AppError> {
		match self {
			Self::Issue(note) => Record::issue(conn, note.issue_id).await,
			Self::Incident(note) => Record::incident(conn, note.incident_id).await,
		}
	}

	fn revision(&self) -> i64 {
		match self {
			Self::Issue(note) => note.revision,
			Self::Incident(note) => note.revision,
		}
	}

	fn out(self) -> NoteOut {
		match self {
			Self::Issue(note) => note.into(),
			Self::Incident(note) => note.into(),
		}
	}
}

/// Why a note cannot be changed here by `caller`, if it cannot. An agent's
/// note can be changed by any caller who may write notes; an operator's only
/// by that operator, on the operator surface.
fn unchangeable(note: &AnyNote, caller: &Caller) -> Option<&'static str> {
	if note.deleted() {
		return Some("the note was deleted");
	}
	let own = matches!(caller, Caller::Operator { login, .. } if *login == note.author());
	(note.author_kind() == AuthorKind::Operator && !own).then_some(
		"the note was written by an operator, and only a note an agent wrote, or one you wrote \
		 yourself on the operator surface, can be changed here",
	)
}

/// What kind of record an identifier names, and its notes, if any record does.
async fn notes_record(
	conn: &mut AsyncPgConnection,
	id: Uuid,
) -> Result<Option<(NotesRecord, &'static str)>, AppError> {
	for (record, kind) in [
		(NotesRecord::Group(id), "group"),
		(NotesRecord::Machine(id), "machine"),
		(NotesRecord::Application(id), "application"),
	] {
		match notes_log::current(conn, record).await {
			Ok(_) => return Ok(Some((record, kind))),
			Err(error) if is_not_found(&error) => {}
			Err(error) => return Err(error),
		}
	}
	Ok(None)
}

async fn record_of(conn: &mut AsyncPgConnection, record: NotesRecord) -> Result<Record, AppError> {
	match record {
		NotesRecord::Group(id) => Record::group(conn, id).await,
		NotesRecord::Machine(id) => Record::machine(conn, id).await,
		NotesRecord::Application(id) => Record::application(conn, id).await,
	}
}

#[tool_router(router = notes_router, vis = "pub(crate)")]
impl CanopyMcp {
	#[tool(
		annotations(
			read_only_hint = false,
			destructive_hint = false,
			idempotent_hint = false
		),
		description = "Add a note to an issue, attributed to you: an agent through an access \
		               token, an operator on the operator surface. Returns the note. Refused, with \
		               no effect, unless the caller is an administrator (on the operator surface) \
		               or holds a token with the notes grant."
	)]
	async fn add_issue_note(
		&self,
		parts: Extension<http::request::Parts>,
		Parameters(args): Parameters<AddIssueNoteArgs>,
	) -> Result<CallToolResult, McpError> {
		let caller = require_notes(parts).await?;
		let issue_id = parse_uuid(&args.issue_id, "issue_id")?;
		let text = body(&args.body)?;
		let actor = caller.actor();
		let mut conn = self.db_write.get().await.map_err(mcp_err)?;
		let note = conn
			.transaction::<_, AppError, _>(async |conn| {
				let issue = Record::issue(conn, issue_id).await?;
				let note = IssueNote::add(
					conn,
					issue_id,
					caller.name(),
					caller.note_author_kind(),
					text,
				)
				.await?;
				let action = Action::IssueNoteAdded {
					issue,
					note: Record::note(note.id, &note.author),
					author_kind: note.author_kind.as_str().to_owned(),
				};
				audit::record(conn, &actor, &action).await?;
				Ok(note)
			})
			.await;
		match note {
			Ok(note) => ok_json(&NoteOut::from(note)),
			Err(error) if is_not_found(&error) => {
				Ok(not_found(format!("no issue with id {issue_id}")))
			}
			Err(error) => Err(mcp_err(error)),
		}
	}

	#[tool(
		annotations(
			read_only_hint = false,
			destructive_hint = false,
			idempotent_hint = false
		),
		description = "Add a note to an incident, attributed to you. Use it for what is worth \
		               saying about this one incident while it is being worked; what a later \
		               investigation should know beyond it belongs in incident memory \
		               (`record_memory_entry`). Returns the note. Refused, with no effect, unless \
		               the caller is an administrator (on the operator surface) or holds a token \
		               with the notes grant."
	)]
	async fn add_incident_note(
		&self,
		parts: Extension<http::request::Parts>,
		Parameters(args): Parameters<AddIncidentNoteArgs>,
	) -> Result<CallToolResult, McpError> {
		let caller = require_notes(parts).await?;
		let incident_id = parse_uuid(&args.incident_id, "incident_id")?;
		let text = body(&args.body)?;
		let actor = caller.actor();
		let mut conn = self.db_write.get().await.map_err(mcp_err)?;
		let note = conn
			.transaction::<_, AppError, _>(async |conn| {
				Incident::get_by_id(conn, incident_id).await?;
				let note = IncidentNote::add(
					conn,
					incident_id,
					caller.name(),
					caller.note_author_kind(),
					text,
				)
				.await?;
				let incident = Record::incident(conn, incident_id).await?;
				let action = Action::IncidentNoteAdded {
					incident,
					note: Record::note(note.id, &note.author),
					author_kind: note.author_kind.as_str().to_owned(),
				};
				audit::record(conn, &actor, &action).await?;
				Ok(note)
			})
			.await;
		match note {
			Ok(note) => ok_json(&NoteOut::from(note)),
			Err(error) if is_not_found(&error) => {
				Ok(not_found(format!("no incident with id {incident_id}")))
			}
			Err(error) => Err(mcp_err(error)),
		}
	}

	#[tool(
		annotations(
			read_only_hint = false,
			destructive_hint = false,
			idempotent_hint = false
		),
		description = "Edit the text of a note on an issue or an incident, stating the `revision` \
		               you read. The text it replaces is kept. Only a note an agent wrote can be \
		               edited, or on the operator surface one the caller wrote: one another \
		               operator wrote is refused, naming that. If the note changed \
		               since the revision you read, the edit is refused and the result carries \
		               the note as it stands. Refused, with no effect, unless the caller is an \
		               administrator (on the operator surface) or holds a token with the notes \
		               grant."
	)]
	async fn edit_note(
		&self,
		parts: Extension<http::request::Parts>,
		Parameters(args): Parameters<EditNoteArgs>,
	) -> Result<CallToolResult, McpError> {
		let caller = require_notes(parts).await?;
		let note_id = parse_uuid(&args.note_id, "note_id")?;
		let text = body(&args.body)?;
		let actor = caller.actor();
		let mut conn = self.db_write.get().await.map_err(mcp_err)?;
		let result = conn
			.transaction::<_, AppError, _>(async |conn| {
				let Some(held) = AnyNote::find(conn, note_id).await? else {
					return Ok(Err(not_found(format!("no note with id {note_id}"))));
				};
				if let Some(reason) = unchangeable(&held, &caller) {
					return Ok(Err(refusal(
						format!("the note cannot be edited: {reason}"),
						json!({ "note_id": note_id }),
					)));
				}
				let target = held.target(conn).await?;
				let edited = match &held {
					AnyNote::Issue(_) => {
						IssueNote::edit(conn, note_id, args.revision, text, caller.name())
							.await?
							.map(AnyNote::Issue)
					}
					AnyNote::Incident(_) => {
						IncidentNote::edit(conn, note_id, args.revision, text, caller.name())
							.await?
							.map(AnyNote::Incident)
					}
				};
				match edited {
					Edited::Stale(current) => {
						let current = current.out();
						Ok(Err(refusal(
							format!(
								"the note changed since revision {} was read; it is now at revision {}",
								args.revision, current.revision
							),
							serde_json::to_value(&current).unwrap_or_default(),
						)))
					}
					Edited::Unchanged(note) => Ok(Ok(note.out())),
					Edited::Changed(note) => {
						let action = Action::NoteEdited {
							target,
							note: Record::note(note.id(), note.author()),
							revision: note.revision(),
						};
						audit::record(conn, &actor, &action).await?;
						Ok(Ok(note.out()))
					}
				}
			})
			.await
			.map_err(mcp_err)?;
		match result {
			Ok(note) => ok_json(&note),
			Err(result) => Ok(result),
		}
	}

	#[tool(
		annotations(
			read_only_hint = false,
			destructive_hint = true,
			idempotent_hint = true
		),
		description = "Delete a note on an issue or an incident. The note leaves the notes but is \
		               kept, recording who deleted it. Only a note an agent wrote can be \
		               deleted, or on the operator surface one the caller wrote: one another \
		               operator wrote is refused, naming that. Refused, with no \
		               effect, unless the caller is an administrator (on the operator surface) \
		               or holds a token with the notes grant."
	)]
	async fn delete_note(
		&self,
		parts: Extension<http::request::Parts>,
		Parameters(args): Parameters<DeleteNoteArgs>,
	) -> Result<CallToolResult, McpError> {
		let caller = require_notes(parts).await?;
		let note_id = parse_uuid(&args.note_id, "note_id")?;
		let actor = caller.actor();
		let mut conn = self.db_write.get().await.map_err(mcp_err)?;
		let result = conn
			.transaction::<_, AppError, _>(async |conn| {
				let Some(held) = AnyNote::find(conn, note_id).await? else {
					return Ok(Err(not_found(format!("no note with id {note_id}"))));
				};
				if held.deleted() {
					return Ok(Err(not_found(format!("note {note_id} is already deleted"))));
				}
				if let Some(reason) = unchangeable(&held, &caller) {
					return Ok(Err(refusal(
						format!("the note cannot be deleted: {reason}"),
						json!({ "note_id": note_id }),
					)));
				}
				let target = held.target(conn).await?;
				let (author, kind) = (held.author().to_owned(), held.author_kind());
				match &held {
					AnyNote::Issue(_) => IssueNote::delete(conn, note_id, caller.name())
						.await
						.map(drop)?,
					AnyNote::Incident(_) => IncidentNote::delete(conn, note_id, caller.name())
						.await
						.map(drop)?,
				}
				let action = Action::NoteDeleted {
					target,
					note: Record::note(note_id, &author),
					author_kind: kind.as_str().to_owned(),
				};
				audit::record(conn, &actor, &action).await?;
				Ok(Ok(json!({ "deleted": note_id })))
			})
			.await
			.map_err(mcp_err)?;
		match result {
			Ok(value) => ok_json(&value),
			Err(result) => Ok(result),
		}
	}

	#[tool(
		annotations(
			read_only_hint = false,
			destructive_hint = false,
			idempotent_hint = false
		),
		description = "Replace the notes of a group, machine or application with `notes`, stating \
		               the `revision` you read (from `get_group`, `get_machine` or \
		               `get_server`). The notes are one piece of free text kept by operators: \
		               read them first and send the whole text you want them to become, so what is \
		               there is not lost. Every change is kept in the revision log \
		               (`get_notes_history`), recorded as made through this interface. If the \
		               notes changed since the revision you read, the replacement is refused and \
		               the result carries the current notes and revision, so you can reconcile \
		               and try again. Refused, with no effect, unless the caller is an \
		               administrator (on the operator surface) or holds a token with the notes \
		               grant."
	)]
	async fn replace_notes(
		&self,
		parts: Extension<http::request::Parts>,
		Parameters(args): Parameters<ReplaceNotesArgs>,
	) -> Result<CallToolResult, McpError> {
		let caller = require_notes(parts).await?;
		let id = parse_uuid(&args.id, "id")?;
		let actor = caller.actor();
		let mut conn = self.db_write.get().await.map_err(mcp_err)?;
		let result = conn
			.transaction::<_, AppError, _>(async |conn| {
				let Some((record, _)) = notes_record(conn, id).await? else {
					return Ok(Err(not_found(format!(
						"no group, machine or application with id {id}"
					))));
				};
				let outcome = notes_log::set(
					conn,
					record,
					&args.notes,
					Some(caller.notes_author()),
					Some(args.revision),
				)
				.await?;
				match outcome {
					Outcome::Stale(current) => Ok(Err(refusal(
						format!(
							"the notes changed since revision {} was read; they are now at revision {}",
							args.revision, current.revision
						),
						serde_json::to_value(&current).unwrap_or_default(),
					))),
					Outcome::Unchanged { revision } => {
						Ok(Ok(json!({ "notes": args.notes, "revision": revision })))
					}
					Outcome::Changed { revision } => {
						let action = Action::NotesChanged {
							record: record_of(conn, record).await?,
							revision,
							via: caller.notes_author().via.as_str().to_owned(),
						};
						audit::record(conn, &actor, &action).await?;
						Ok(Ok(json!({ "notes": args.notes, "revision": revision })))
					}
				}
			})
			.await
			.map_err(mcp_err)?;
		match result {
			Ok(value) => ok_json(&value),
			Err(result) => Ok(result),
		}
	}

	#[tool(
		annotations(read_only_hint = true),
		description = "The revision log of a group's, machine's or application's notes, newest \
		               first: each revision with its text, who made it, how (the operator \
		               interface, an Ansible import, or an access token), and when."
	)]
	async fn get_notes_history(
		&self,
		Parameters(args): Parameters<NotesHistoryArgs>,
	) -> Result<CallToolResult, McpError> {
		let id = parse_uuid(&args.id, "id")?;
		let mut conn = self.conn().await?;
		let Some((record, kind)) = notes_record(&mut conn, id).await.map_err(mcp_err)? else {
			return Ok(not_found(format!(
				"no group, machine or application with id {id}"
			)));
		};
		let revisions = notes_log::history(&mut conn, record)
			.await
			.map_err(mcp_err)?;
		let revision = revisions.first().map_or(0, |latest| latest.revision);
		ok_json(&NotesHistory {
			id,
			kind,
			revision,
			revisions,
		})
	}
}
