//! The revision log of the notes a group, machine or application carries.
//!
//! Spec: `.workhorse/specs/servers/overview.md` (id `FLT`), "Notes".
//!
//! Every change to a record's notes goes through [`set`], which locks the
//! record, compares against what it holds, and writes the new text and its log
//! entry together, so nothing writes the `notes` column directly. A save that
//! leaves the text as it was writes nothing. The log holds the text the notes
//! had after each change; the record's revision is the latest number in it,
//! and is `0` for a record that has never had notes.

use commons_errors::{AppError, Result};
use diesel::{prelude::*, sql_types};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A record that carries notes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotesRecord {
	Group(Uuid),
	Machine(Uuid),
	Application(Uuid),
}

impl NotesRecord {
	fn table(self) -> &'static str {
		match self {
			Self::Group(_) => "server_groups",
			Self::Machine(_) => "machines",
			Self::Application(_) => "applications",
		}
	}

	fn column(self) -> &'static str {
		match self {
			Self::Group(_) => "server_group_id",
			Self::Machine(_) => "machine_id",
			Self::Application(_) => "application_id",
		}
	}

	pub fn id(self) -> Uuid {
		match self {
			Self::Group(id) | Self::Machine(id) | Self::Application(id) => id,
		}
	}
}

/// How a change to notes was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Via {
	/// An operator, in the operator interface.
	Operator,
	/// An Ansible inventory import.
	AnsibleImport,
	/// A fleet query access token, through the fleet query interface.
	Token,
}

impl Via {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Operator => "operator",
			Self::AnsibleImport => "ansible_import",
			Self::Token => "token",
		}
	}
}

/// Who made a change to notes, and how.
#[derive(Clone, Copy, Debug)]
pub struct Author<'a> {
	/// An operator's login, or a token's name.
	pub name: &'a str,
	pub via: Via,
}

impl<'a> Author<'a> {
	pub fn operator(login: &'a str) -> Self {
		Self {
			name: login,
			via: Via::Operator,
		}
	}

	pub fn ansible_import(login: &'a str) -> Self {
		Self {
			name: login,
			via: Via::AnsibleImport,
		}
	}

	pub fn token(name: &'a str) -> Self {
		Self {
			name: name.trim(),
			via: Via::Token,
		}
	}
}

/// What a record's notes hold now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CurrentNotes {
	pub notes: String,
	/// The latest revision, `0` where the record has never had notes.
	pub revision: i64,
}

/// One entry in a record's revision log.
#[derive(Clone, Debug, Serialize, Deserialize, QueryableByName, utoipa::ToSchema)]
pub struct NotesRevision {
	#[diesel(sql_type = sql_types::BigInt)]
	pub revision: i64,
	/// The text the notes held after this change.
	#[diesel(sql_type = sql_types::Text)]
	pub notes: String,
	/// Who made the change. Absent for the revision a record's notes were given
	/// when the log began.
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	pub author: Option<String>,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	pub via: Option<String>,
	#[diesel(sql_type = sql_types::Timestamptz, deserialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
}

/// What [`set`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
	/// The notes changed, and are now at `revision`.
	Changed { revision: i64 },
	/// The text was what the notes already held, so nothing was written.
	Unchanged { revision: i64 },
	/// The notes had changed since the revision the caller read.
	Stale(CurrentNotes),
}

impl Outcome {
	/// The revision the notes are at, if the write was applied or moot, and
	/// the conflict as an error if it was refused.
	pub fn or_conflict(self) -> Result<Self> {
		match self {
			Self::Stale(current) => Err(AppError::Conflict(format!(
				"the notes changed since they were read; they are now at revision {}",
				current.revision
			))),
			applied => Ok(applied),
		}
	}
}

#[derive(QueryableByName)]
struct Held {
	#[diesel(sql_type = sql_types::Text)]
	notes: String,
	#[diesel(sql_type = sql_types::BigInt)]
	revision: i64,
}

/// Set a record's notes, by `author`, or unattributed where the caller has no
/// one to name.
///
/// With `expected_revision` the write is refused as [`Outcome::Stale`] when
/// the notes are past it. Without it the write is unconditional.
pub async fn set(
	conn: &mut AsyncPgConnection,
	record: NotesRecord,
	new_text: &str,
	author: Option<Author<'_>>,
	expected_revision: Option<i64>,
) -> Result<Outcome> {
	let (table, column) = (record.table(), record.column());
	let new_text = new_text.to_owned();
	let author = author.map(|author| (author.name.to_owned(), author.via));
	conn.transaction::<_, AppError, _>(async move |conn| {
		let held: Held = diesel::sql_query(format!(
			"SELECT t.notes, COALESCE((SELECT max(r.revision) FROM notes_revisions r \
			 WHERE r.{column} = t.id), 0) AS revision \
			 FROM {table} t WHERE t.id = $1 FOR UPDATE OF t"
		))
		.bind::<sql_types::Uuid, _>(record.id())
		.get_result(conn)
		.await
		.map_err(AppError::from)?;

		if let Some(expected) = expected_revision
			&& expected != held.revision
		{
			return Ok(Outcome::Stale(CurrentNotes {
				notes: held.notes,
				revision: held.revision,
			}));
		}
		if held.notes == new_text {
			return Ok(Outcome::Unchanged {
				revision: held.revision,
			});
		}

		let revision = held.revision + 1;
		diesel::sql_query(format!("UPDATE {table} SET notes = $2 WHERE id = $1"))
			.bind::<sql_types::Uuid, _>(record.id())
			.bind::<sql_types::Text, _>(&new_text)
			.execute(conn)
			.await
			.map_err(AppError::from)?;
		diesel::sql_query(format!(
			"INSERT INTO notes_revisions ({column}, revision, notes, author, via) \
			 VALUES ($1, $2, $3, $4, $5)"
		))
		.bind::<sql_types::Uuid, _>(record.id())
		.bind::<sql_types::BigInt, _>(revision)
		.bind::<sql_types::Text, _>(&new_text)
		.bind::<sql_types::Nullable<sql_types::Text>, _>(author.as_ref().map(|(name, _)| name))
		.bind::<sql_types::Nullable<sql_types::Text>, _>(
			author.as_ref().map(|(_, via)| via.as_str()),
		)
		.execute(conn)
		.await
		.map_err(AppError::from)?;
		Ok(Outcome::Changed { revision })
	})
	.await
}

/// A record's notes and the revision they are at.
pub async fn current(conn: &mut AsyncPgConnection, record: NotesRecord) -> Result<CurrentNotes> {
	let (table, column) = (record.table(), record.column());
	let held: Held = diesel::sql_query(format!(
		"SELECT t.notes, COALESCE((SELECT max(r.revision) FROM notes_revisions r \
		 WHERE r.{column} = t.id), 0) AS revision \
		 FROM {table} t WHERE t.id = $1"
	))
	.bind::<sql_types::Uuid, _>(record.id())
	.get_result(conn)
	.await
	.map_err(AppError::from)?;
	Ok(CurrentNotes {
		notes: held.notes,
		revision: held.revision,
	})
}

/// A record's revision log, newest first.
pub async fn history(
	conn: &mut AsyncPgConnection,
	record: NotesRecord,
) -> Result<Vec<NotesRevision>> {
	let column = record.column();
	diesel::sql_query(format!(
		"SELECT revision, notes, author, via, created_at FROM notes_revisions \
		 WHERE {column} = $1 ORDER BY revision DESC"
	))
	.bind::<sql_types::Uuid, _>(record.id())
	.load(conn)
	.await
	.map_err(AppError::from)
}
