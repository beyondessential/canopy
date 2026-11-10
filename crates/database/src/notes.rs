//! Free-text notes attached to issues and incidents.
//!
//! Spec: `.workhorse/specs/monitoring/checks.md` (id `CHK`), "Notes", and
//! `.workhorse/specs/monitoring/incidents.md` (id `INC`).
//!
//! A note is written by an operator or by an agent, each recorded as such. It
//! can be edited, which keeps the text it replaced, and deleted, which keeps
//! the note and records who deleted it and when. Lists leave deleted notes out
//! unless asked for them. Edits are guarded by the revision the editor read, so
//! two writers cannot silently overwrite each other.
//!
//! Who may edit or delete which note is for the caller to decide, from
//! [`AuthorKind`]: this module only records.

use commons_errors::{AppError, Result};
use diesel::{
	deserialize::{self, FromSql, FromSqlRow},
	expression::AsExpression,
	pg::{Pg, PgValue},
	prelude::*,
	serialize::{self, IsNull, Output, ToSql},
	sql_types::Text,
};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::io::Write;
use uuid::Uuid;

use crate::issues::{Incident, Issue};

/// Whether a person or an agent wrote a note.
#[derive(
	Clone,
	Copy,
	Debug,
	PartialEq,
	Eq,
	Serialize,
	Deserialize,
	AsExpression,
	FromSqlRow,
	utoipa::ToSchema,
)]
#[diesel(sql_type = Text)]
#[serde(rename_all = "snake_case")]
pub enum AuthorKind {
	Operator,
	Agent,
}

impl AuthorKind {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Operator => "operator",
			Self::Agent => "agent",
		}
	}
}

impl ToSql<Text, Pg> for AuthorKind {
	fn to_sql<'b>(&'b self, out: &mut Output<'b, '_, Pg>) -> serialize::Result {
		out.write_all(self.as_str().as_bytes())?;
		Ok(IsNull::No)
	}
}

impl FromSql<Text, Pg> for AuthorKind {
	fn from_sql(bytes: PgValue<'_>) -> deserialize::Result<Self> {
		match std::str::from_utf8(bytes.as_bytes())? {
			"operator" => Ok(Self::Operator),
			"agent" => Ok(Self::Agent),
			other => Err(format!("unknown note author kind {other:?}").into()),
		}
	}
}

/// What an edit did.
#[derive(Clone, Debug)]
pub enum Edited<N> {
	/// The note now holds the new text, at its new revision.
	Changed(N),
	/// The new text is what the note already held.
	Unchanged(N),
	/// The note had changed since the revision the editor read; this is the
	/// note as it stands.
	Stale(N),
}

/// The text a note held before an edit replaced it.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct NoteRevision {
	/// The revision of the note this text was.
	pub revision: i64,
	pub body: String,
	/// Who made the edit that replaced it.
	pub edited_by: String,
	pub edited_at: Timestamp,
}

#[derive(Clone, Debug, Serialize, Deserialize, Queryable, Selectable, Associations)]
#[diesel(belongs_to(Issue))]
#[diesel(table_name = crate::schema::issue_notes)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct IssueNote {
	pub id: Uuid,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	pub issue_id: Uuid,
	pub author: String,
	pub body: String,
	pub author_kind: AuthorKind,
	pub revision: i64,
	#[diesel(
		deserialize_as = jiff_diesel::NullableTimestamp,
		serialize_as = jiff_diesel::NullableTimestamp
	)]
	pub deleted_at: Option<Timestamp>,
	pub deleted_by: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Queryable, Selectable, Associations)]
#[diesel(belongs_to(Incident))]
#[diesel(table_name = crate::schema::incident_notes)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct IncidentNote {
	pub id: Uuid,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	pub incident_id: Uuid,
	pub author: String,
	pub body: String,
	pub author_kind: AuthorKind,
	pub revision: i64,
	#[diesel(
		deserialize_as = jiff_diesel::NullableTimestamp,
		serialize_as = jiff_diesel::NullableTimestamp
	)]
	pub deleted_at: Option<Timestamp>,
	pub deleted_by: Option<String>,
}

/// The operations a kind of note shares. The two kinds have tables of their own
/// with the same shape, so the same code is written once for each.
macro_rules! note_kind {
	(
		$note:ident, $table:ident, $revisions:ident, $parent_col:ident, $parent:ident,
		list = $list:ident, list_doc = $list_doc:literal
	) => {
		impl $note {
			/// Add a note under `author`, written as `author_kind`.
			pub async fn add(
				db: &mut AsyncPgConnection,
				$parent: Uuid,
				author: &str,
				author_kind: AuthorKind,
				body: &str,
			) -> Result<Self> {
				use crate::schema::$table;
				diesel::insert_into($table::table)
					.values((
						$table::$parent_col.eq($parent),
						$table::author.eq(author),
						$table::author_kind.eq(author_kind),
						$table::body.eq(body),
					))
					.returning(Self::as_select())
					.get_result(db)
					.await
					.map_err(AppError::from)
			}

			/// A note, deleted or not.
			pub async fn get(db: &mut AsyncPgConnection, note_id: Uuid) -> Result<Self> {
				use crate::schema::$table::dsl;
				dsl::$table
					.select(Self::as_select())
					.filter(dsl::id.eq(note_id))
					.first(db)
					.await
					.map_err(AppError::from)
			}

			#[doc = $list_doc]
			pub async fn $list(
				db: &mut AsyncPgConnection,
				$parent: Uuid,
				limit: i64,
				include_deleted: bool,
			) -> Result<Vec<Self>> {
				use crate::schema::$table::dsl;
				let mut query = dsl::$table
					.select(Self::as_select())
					.filter(dsl::$parent_col.eq($parent))
					.into_boxed();
				if !include_deleted {
					query = query.filter(dsl::deleted_at.is_null());
				}
				query
					.order(dsl::created_at.desc())
					.limit(limit)
					.load(db)
					.await
					.map_err(AppError::from)
			}

			/// Replace a note's text with `body`, keeping the text it replaces
			/// with `editor`'s name. Refused as [`Edited::Stale`] if the note is past
			/// `expected_revision`; a deleted note cannot be edited.
			pub async fn edit(
				db: &mut AsyncPgConnection,
				note_id: Uuid,
				expected_revision: i64,
				body: &str,
				editor: &str,
			) -> Result<Edited<Self>> {
				use crate::schema::{$revisions, $table};
				let (body, editor) = (body.to_owned(), editor.to_owned());
				db.transaction::<_, AppError, _>(async move |conn| {
					let held: Self = $table::table
						.select(Self::as_select())
						.filter($table::id.eq(note_id))
						.for_update()
						.first(conn)
						.await
						.map_err(AppError::from)?;
					if held.deleted_at.is_some() {
						return Err(AppError::NotFound(format!("no note {note_id}")));
					}
					if held.revision != expected_revision {
						return Ok(Edited::Stale(held));
					}
					if held.body == body {
						return Ok(Edited::Unchanged(held));
					}
					diesel::insert_into($revisions::table)
						.values((
							$revisions::note_id.eq(note_id),
							$revisions::revision.eq(held.revision),
							$revisions::body.eq(&held.body),
							$revisions::edited_by.eq(&editor),
						))
						.execute(conn)
						.await
						.map_err(AppError::from)?;
					let edited = diesel::update($table::table.filter($table::id.eq(note_id)))
						.set((
							$table::body.eq(&body),
							$table::revision.eq(held.revision + 1),
						))
						.returning(Self::as_select())
						.get_result(conn)
						.await
						.map_err(AppError::from)?;
					Ok(Edited::Changed(edited))
				})
				.await
			}

			/// Delete a note, keeping it and recording `by`. Answers the note as
			/// deleted, or none when it was not there or already deleted, so
			/// there is nothing more to record.
			pub async fn delete(
				db: &mut AsyncPgConnection,
				note_id: Uuid,
				by: &str,
			) -> Result<Option<Self>> {
				use crate::schema::$table;
				diesel::update(
					$table::table
						.filter($table::id.eq(note_id))
						.filter($table::deleted_at.is_null()),
				)
				.set((
					$table::deleted_at.eq(jiff_diesel::Timestamp::from(Timestamp::now())),
					$table::deleted_by.eq(by),
				))
				.returning(Self::as_select())
				.get_result(db)
				.await
				.optional()
				.map_err(AppError::from)
			}

			/// The text a note held before each edit, newest edit first.
			pub async fn edit_history(
				db: &mut AsyncPgConnection,
				note_id: Uuid,
			) -> Result<Vec<NoteRevision>> {
				use crate::schema::$revisions::dsl;
				dsl::$revisions
					.select((dsl::revision, dsl::body, dsl::edited_by, dsl::edited_at))
					.filter(dsl::note_id.eq(note_id))
					.order(dsl::revision.desc())
					.load::<(i64, String, String, jiff_diesel::Timestamp)>(db)
					.await
					.map_err(AppError::from)
					.map(|rows| {
						rows.into_iter()
							.map(|(revision, body, edited_by, edited_at)| NoteRevision {
								revision,
								body,
								edited_by,
								edited_at: edited_at.into(),
							})
							.collect()
					})
			}
		}
	};
}

note_kind!(
	IssueNote,
	issue_notes,
	issue_note_revisions,
	issue_id,
	issue_id,
	list = list_for_issue,
	list_doc = "An issue's notes, newest first, leaving deleted ones out unless asked for."
);
note_kind!(
	IncidentNote,
	incident_notes,
	incident_note_revisions,
	incident_id,
	incident_id,
	list = list_for_incident,
	list_doc = "An incident's notes, newest first, leaving deleted ones out unless asked for."
);
