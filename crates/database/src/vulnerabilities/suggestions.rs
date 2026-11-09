//! The model's suggestions, and the queue of items waiting for one.
//!
//! Spec: `.workhorse/specs/security/vulnerability-triage.md` (id `VTR`),
//! "Model triage".

use std::collections::HashMap;

use commons_errors::Result;
use commons_types::server::rank::ServerRank;
use diesel::sql_types;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::{
	Source, Status,
	decisions::{
		Decision, Entry, reaching_workspace_packages, shortened_reason, status_of, validate_entries,
	},
	items::EXPOSURE_CTE,
};

/// What a suggestion rests on. The model has no access to source code, so none
/// of these is a finding about how the code uses a package.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = VulnerabilitySuggestionBasis)]
pub enum Basis {
	/// How the package is reached through dependencies, and their scopes.
	DependencyPathOnly,
	/// What the advisory says.
	AdvisoryTextOnly,
	/// Both.
	DependencyPathAndAdvisoryText,
	/// Too little to say more than that a person with the source should look.
	InsufficientInformation,
}

impl Basis {
	pub const ALL: [Self; 4] = [
		Self::DependencyPathOnly,
		Self::AdvisoryTextOnly,
		Self::DependencyPathAndAdvisoryText,
		Self::InsufficientInformation,
	];

	pub fn as_str(self) -> &'static str {
		match self {
			Self::DependencyPathOnly => "dependency_path_only",
			Self::AdvisoryTextOnly => "advisory_text_only",
			Self::DependencyPathAndAdvisoryText => "dependency_path_and_advisory_text",
			Self::InsufficientInformation => "insufficient_information",
		}
	}

	pub fn parse(s: &str) -> Option<Self> {
		Self::ALL.into_iter().find(|basis| basis.as_str() == s)
	}

	/// The plain line a suggestion presents its basis as.
	pub fn describe(self) -> &'static str {
		match self {
			Self::DependencyPathOnly => "Based on dependency paths only",
			Self::AdvisoryTextOnly => "Based on advisory text only",
			Self::DependencyPathAndAdvisoryText => "Based on dependency paths and advisory text",
			Self::InsufficientInformation => "Insufficient information",
		}
	}
}

/// The model's latest suggestion for an item. Not a decision: the item stays
/// as it was. Made without access to source code.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = VulnerabilitySuggestion)]
pub struct Suggestion {
	/// What the model suggests, in the shape of a decision.
	pub entries: Vec<Entry>,
	/// The status the suggestion would give the item.
	pub status: Status,
	/// What it rests on.
	pub basis: Basis,
	/// The basis as a plain line of text.
	pub basis_text: String,
	/// Always set: the model has no access to source code, so this is an
	/// inference from the advisory and the dependency tree, to be checked.
	pub note: String,
	/// The model that made it.
	pub model: String,
	/// When it was made.
	pub created_at: Timestamp,
}

const NO_SOURCE_NOTE: &str =
	"Made without access to source code; an inference to check, not a finding";

#[derive(diesel::QueryableByName)]
struct SuggestionRow {
	#[diesel(sql_type = sql_types::Text)]
	vulnerability_id: String,
	#[diesel(sql_type = sql_types::Text)]
	source: String,
	#[diesel(sql_type = sql_types::Jsonb)]
	entries: serde_json::Value,
	#[diesel(sql_type = sql_types::Text)]
	basis: String,
	#[diesel(sql_type = sql_types::Text)]
	model: String,
	#[diesel(sql_type = sql_types::Timestamptz, deserialize_as = jiff_diesel::Timestamp)]
	created_at: Timestamp,
}

impl SuggestionRow {
	fn split(self) -> Option<((String, Source), Suggestion)> {
		let entries: Vec<Entry> = serde_json::from_value(self.entries).ok()?;
		let basis = Basis::parse(&self.basis)?;
		Some((
			(self.vulnerability_id, Source::parse(&self.source)?),
			Suggestion {
				status: status_of(&entries),
				entries,
				basis,
				basis_text: basis.describe().to_owned(),
				note: NO_SOURCE_NOTE.to_owned(),
				model: self.model,
				created_at: self.created_at,
			},
		))
	}
}

const SUGGESTION_COLUMNS: &str = "vulnerability_id, source, entries, basis, model, created_at";

impl Suggestion {
	/// Keep a suggestion as the item's, replacing the one before. Its entries
	/// are held to the rules of a decision, against the workspace packages that
	/// reach the advisory now.
	pub async fn record(
		conn: &mut AsyncPgConnection,
		vulnerability_id: &str,
		source: &Source,
		entries: Vec<Entry>,
		basis: Basis,
		model: &str,
	) -> Result<()> {
		let reaching = reaching_workspace_packages(conn, vulnerability_id, source).await?;
		validate_entries(&entries, source, &reaching)?;
		diesel::sql_query(
			"INSERT INTO vulnerability_suggestions (vulnerability_id, source, entries, basis, model) \
			 VALUES ($1, $2, $3, $4, $5) \
			 ON CONFLICT (vulnerability_id, source) DO UPDATE SET entries = EXCLUDED.entries, \
			 basis = EXCLUDED.basis, model = EXCLUDED.model, created_at = now()",
		)
		.bind::<sql_types::Text, _>(vulnerability_id)
		.bind::<sql_types::Text, _>(source.as_str())
		.bind::<sql_types::Jsonb, _>(serde_json::to_value(&entries).unwrap_or_default())
		.bind::<sql_types::Text, _>(basis.as_str())
		.bind::<sql_types::Text, _>(model)
		.execute(conn)
		.await?;
		Ok(())
	}

	pub async fn get(
		conn: &mut AsyncPgConnection,
		vulnerability_id: &str,
		source: &Source,
	) -> Result<Option<Self>> {
		use diesel::OptionalExtension;
		let row: Option<SuggestionRow> = diesel::sql_query(format!(
			"SELECT {SUGGESTION_COLUMNS} FROM vulnerability_suggestions \
			 WHERE vulnerability_id = $1 AND source = $2"
		))
		.bind::<sql_types::Text, _>(vulnerability_id)
		.bind::<sql_types::Text, _>(source.as_str())
		.get_result(conn)
		.await
		.optional()?;
		Ok(row
			.and_then(SuggestionRow::split)
			.map(|(_, suggestion)| suggestion))
	}

	/// The suggestions held for `items`.
	pub async fn for_items(
		conn: &mut AsyncPgConnection,
		items: &[(String, String)],
	) -> Result<HashMap<(String, Source), Self>> {
		let ids: Vec<&str> = items.iter().map(|(id, _)| id.as_str()).collect();
		let sources: Vec<&str> = items.iter().map(|(_, source)| source.as_str()).collect();
		let rows: Vec<SuggestionRow> = diesel::sql_query(format!(
			"SELECT {SUGGESTION_COLUMNS} FROM vulnerability_suggestions \
			 WHERE (vulnerability_id, source) IN (SELECT * FROM unnest($1::text[], $2::text[]))"
		))
		.bind::<sql_types::Array<sql_types::Text>, _>(&ids)
		.bind::<sql_types::Array<sql_types::Text>, _>(&sources)
		.load(conn)
		.await?;
		Ok(rows.into_iter().filter_map(SuggestionRow::split).collect())
	}
}

/// Claim up to `limit` items for the model to look at: needing triage, closed
/// for no source, with no suggestion, and not looked at within `retry_after`.
/// Claiming marks the attempt, so a reply that never comes is asked for again
/// only after `retry_after`, and two workers do not take the same item.
pub async fn claim_for_model(
	conn: &mut AsyncPgConnection,
	limit: i64,
	retry_after: jiff::SignedDuration,
) -> Result<Vec<(String, Source)>> {
	#[derive(diesel::QueryableByName)]
	struct Row {
		#[diesel(sql_type = sql_types::Text)]
		vulnerability_id: String,
		#[diesel(sql_type = sql_types::Text)]
		source: String,
	}
	let rows: Vec<Row> = diesel::sql_query(format!(
		"WITH {EXPOSURE_CTE} \
		 UPDATE vulnerability_items i SET model_attempted_at = now() \
		 WHERE (i.vulnerability_id, i.source) IN ( \
		   SELECT c.vulnerability_id, c.source FROM vulnerability_items c \
		   WHERE EXISTS (SELECT 1 FROM exposure e \
		         WHERE e.vulnerability_id = c.vulnerability_id AND e.source = c.source) \
		   AND NOT EXISTS (SELECT 1 FROM vulnerability_suggestions s \
		        WHERE s.vulnerability_id = c.vulnerability_id AND s.source = c.source) \
		   AND ( \
		     NOT EXISTS (SELECT 1 FROM vulnerability_decisions d \
		          WHERE d.vulnerability_id = c.vulnerability_id AND d.source = c.source) \
		     OR (SELECT d.id FROM vulnerability_decisions d \
		          WHERE d.vulnerability_id = c.vulnerability_id AND d.source = c.source \
		          ORDER BY d.created_at DESC, d.id LIMIT 1) \
		        IN (SELECT decision_id FROM vulnerability_decision_withdrawals \
		            UNION SELECT decision_id FROM vulnerability_lapses) \
		   ) \
		   AND (c.model_attempted_at IS NULL OR c.model_attempted_at < now() - make_interval(secs => $4)) \
		   ORDER BY c.first_seen_at LIMIT $3 \
		   FOR UPDATE OF c SKIP LOCKED) \
		 RETURNING i.vulnerability_id, i.source"
	))
	.bind::<sql_types::Nullable<sql_types::Array<sql_types::Text>>, _>(None::<Vec<String>>)
	.bind::<sql_types::Text, _>(ServerRank::Production.to_string())
	.bind::<sql_types::BigInt, _>(limit)
	.bind::<sql_types::Double, _>(retry_after.as_secs_f64())
	.load(conn)
	.await?;
	Ok(rows
		.into_iter()
		.filter_map(|row| Some((row.vulnerability_id, Source::parse(&row.source)?)))
		.collect())
}

/// Give a decision's withdrawn entries as a suggestion on the item with the
/// basis that says nothing was checked, as what the model decided before it
/// only suggested.
pub async fn suggest_from_decision(
	conn: &mut AsyncPgConnection,
	decision: &Decision,
	model: &str,
) -> Result<()> {
	let entries = decision
		.entries
		.iter()
		.cloned()
		.map(|mut entry| {
			entry.reason = entry.reason.as_deref().map(shortened_reason);
			entry
		})
		.collect();
	Suggestion::record(
		conn,
		&decision.vulnerability_id,
		&decision.source,
		entries,
		Basis::InsufficientInformation,
		model,
	)
	.await
}
