//! `find_vulnerabilities`, `get_vulnerability`, the three tools that change a
//! triage decision (`record_vulnerability_decisions`,
//! `withdraw_vulnerability_decisions` and `edit_vulnerability_decision`), and
//! the `triage_vulnerabilities` prompt.
//!
//! Spec: `.workhorse/specs/private-server/mcp.md` (id `MCP`), "Vulnerabilities".
//!
//! The three writes are held by the triage grant (see [`Caller::may_triage`]).
//! A batch judges each item on its own transaction, so a refused item neither
//! blocks nor rolls back the ones recorded before or after it, and each
//! recorded item is audited as its own decision.

use commons_errors::AppError;
use database::{
	audit::{self, Actor},
	diesel_async::{AsyncConnection, AsyncPgConnection},
	osv::Severity,
	vulnerabilities::{
		Advisory, Decision, Edit, Entry, GroupBy, ItemDetail, ItemSummary, ListQuery, NewDecision,
		Outcome, REPOSITORIES, Sort, SortColumn, Source, Status, Via, Withdrawal,
	},
};
use rmcp::{
	handler::server::{tool::Extension, wrapper::Parameters},
	model::{CallToolResult, ErrorData as McpError, GetPromptResult, PromptMessage, Role},
	prompt, prompt_router, tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
	CanopyMcp, caller,
	util::{mcp_err, not_found, ok_json},
};

/// The most items one call to a batch tool takes.
const MAX_BATCH: usize = 100;

/// The fields of an item `find_vulnerabilities` can return, besides `details`.
const ITEM_FIELDS: &[&str] = &[
	"id",
	"source",
	"aliases",
	"summary",
	"severity",
	"published_at",
	"packages",
	"scopes",
	"workspace_packages",
	"machines",
	"closed",
	"status",
	"decision",
	"withdrawn",
	"edited",
	"lapse",
	"suggestion",
	"first_seen_at",
];

/// The advisory's long text, returned only when asked for.
const DETAILS: &str = "details";

/// The fields every item carries, whatever `fields` says.
const ALWAYS: &[&str] = &["id", "source"];

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SortArg {
	/// The column: `severity`, `summary`, `package`, `source`, `scope`,
	/// `status`, `published`, `machines` or `first_seen`.
	pub column: String,
	/// Highest first. Defaults to ascending.
	pub descending: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindVulnerabilitiesArgs {
	/// Only items of this source: the name of a tracked repository, such as
	/// `tamanu`, or `linux` (packages installed on machines).
	pub source: Option<String>,
	/// Only items with this triage status: `open` (needs triage), `not_affected`,
	/// `accepted_risk`, `will_fix` or `mixed` (a split whose entries differ).
	/// Omitted, every status.
	pub status: Option<String>,
	/// Only items at or above this severity: `low`, `medium`, `high` or
	/// `critical`.
	pub min_severity: Option<String>,
	/// Only items reached through a path of this scope, such as `runtime` or
	/// `development`, or `optional`.
	pub scope: Option<String>,
	/// Only items affecting a package of this name.
	pub package: Option<String>,
	/// Only items that this workspace package reaches.
	pub workspace_package: Option<String>,
	/// Only items that have a model suggestion (true) or none (false).
	pub has_suggestion: Option<bool>,
	/// Include closed items, which nothing counts for any more. Defaults to
	/// false.
	pub include_closed: Option<bool>,
	/// Free text matched, tolerating partial words and small typos, against the
	/// identifier and aliases, summary, package names, workspace packages,
	/// source and the advisory's details. A search is ordered by relevance
	/// unless `sort` is given.
	pub search: Option<String>,
	/// The order. Omitted, by severity and then publication date.
	pub sort: Option<SortArg>,
	/// The `next_cursor` of the page before, for the next page.
	pub cursor: Option<String>,
	/// How many items a page holds, at most 200. Defaults to 50.
	pub page_size: Option<u32>,
	/// The fields of each item to return, out of `id`, `source`, `aliases`,
	/// `summary`, `severity`, `published_at`, `packages`, `scopes`,
	/// `workspace_packages`, `machines`, `closed`, `status`, `decision`,
	/// `withdrawn`, `edited`, `lapse`, `suggestion`, `first_seen_at` and
	/// `details` (the advisory's long text). `id` and `source` are always
	/// returned. Omitted, every field but `details`.
	pub fields: Option<Vec<String>>,
	/// Also return each item's `details`, as naming it in `fields` does.
	pub include_details: Option<bool>,
	/// Instead of items, count them by `package`, `workspace_package`, `scope`,
	/// `source` or `status`. The result is each group with how many matching
	/// items fall in it, and the total matching, with no items. An item with
	/// several packages, scopes or workspace packages counts in each of them.
	pub group_by: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetVulnerabilityArgs {
	/// The advisory's OSV identifier, such as `GHSA-xxxx-xxxx-xxxx`.
	pub id: String,
	/// The item's source: a tracked repository's name, such as `tamanu`, or
	/// `linux`.
	pub source: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EntryArg {
	/// The workspace packages this entry covers. Absent for the catch-all,
	/// which covers every other workspace package and must be last.
	pub workspace_packages: Option<Vec<String>>,
	/// `not_affected`, `accepted_risk` or `will_fix`.
	pub outcome: String,
	/// Why, saying what was checked. Required for `not_affected` and
	/// `accepted_risk`.
	pub reason: Option<String>,
	/// A link to the issue tracking the fix, for `will_fix`. An http or https
	/// address.
	pub issue_url: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DecisionArg {
	/// The advisory's OSV identifier.
	pub id: String,
	/// The item's source: a tracked repository's name, or `linux`.
	pub source: String,
	/// The decision. A whole decision is one entry without workspace packages.
	/// A split names every workspace package that reaches the advisory in some
	/// entry, or ends in a catch-all.
	pub entries: Vec<EntryArg>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RecordVulnerabilityDecisionsArgs {
	/// The decisions to record, at most 100, each judged on its own.
	pub decisions: Vec<DecisionArg>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ItemArg {
	/// The advisory's OSV identifier.
	pub id: String,
	/// The item's source: a tracked repository's name, or `linux`.
	pub source: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WithdrawVulnerabilityDecisionsArgs {
	/// The items whose current decision to withdraw, at most 100, each judged
	/// on its own.
	pub items: Vec<ItemArg>,
	/// Why the decisions are taken away. Recorded against each.
	pub reason: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EditVulnerabilityDecisionArgs {
	/// The advisory's OSV identifier.
	pub id: String,
	/// The item's source: a tracked repository's name, or `linux`.
	pub source: String,
	/// A workspace package the entry to edit covers. Absent edits the
	/// catch-all, which is the only entry of a whole decision.
	pub workspace_package: Option<String>,
	/// The corrected reason. Absent leaves it as it is.
	pub reason: Option<String>,
	/// The corrected issue link, blank to clear it. Absent leaves it as it is.
	pub issue_url: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TriageVulnerabilitiesArgs {
	/// The repository to triage, by the name its items carry as their source:
	/// one of the tracked repositories.
	pub repository: String,
}

/// What came of one item of a batch, as `{id, source, <flag>: bool, error?,
/// <key>?}`: the flag says whether the item was done, and a refused item
/// carries the reason in `error` and changed nothing.
///
/// Only what the caller got wrong refuses an item. Anything else is Canopy
/// failing, not the item being invalid, so it ends the call as an error
/// instead: an agent is told to put a refused item again once corrected, and a
/// fault it cannot correct must not read as one of those.
fn batch_result<T: Serialize>(
	id: String,
	source: String,
	outcome: Result<T, AppError>,
	flag: &str,
	key: &str,
) -> Result<Value, McpError> {
	let mut object = Map::new();
	object.insert("id".into(), Value::String(id));
	object.insert("source".into(), Value::String(source));
	match outcome {
		Ok(result) => {
			object.insert(flag.into(), true.into());
			object.insert(key.into(), serde_json::to_value(result).map_err(mcp_err)?);
		}
		Err(AppError::BadRequest(message) | AppError::NotFound(message)) => {
			object.insert(flag.into(), false.into());
			object.insert("error".into(), Value::String(message));
		}
		Err(e) => return Err(mcp_err(e)),
	}
	Ok(Value::Object(object))
}

/// A batch's results, one per item in the order given, and how many were
/// refused.
fn batch_results(results: Vec<Value>, flag: &str) -> Value {
	let refused = results
		.iter()
		.filter(|result| result[flag] == Value::Bool(false))
		.count();
	serde_json::json!({ "results": results, "refused": refused })
}

#[derive(Debug, Serialize)]
struct Grouping {
	groups: Vec<database::vulnerabilities::Group>,
	/// How many items match, each counted once however many groups it is in.
	total: i64,
}

fn invalid(what: &str, value: &str) -> McpError {
	McpError::invalid_params(format!("invalid {what}: {value}"), None)
}

fn source(value: &str) -> Result<Source, McpError> {
	Source::parse(value).ok_or_else(|| invalid("source", value))
}

/// A database error as the protocol error it should be: a refusal of the input
/// is the caller's to correct.
fn app_err(e: AppError) -> McpError {
	match e {
		AppError::BadRequest(message) => McpError::invalid_params(message, None),
		e => mcp_err(e),
	}
}

fn bad_request(message: String) -> AppError {
	AppError::BadRequest(message)
}

async fn require_triage(
	parts: Extension<http::request::Parts>,
) -> Result<caller::Caller, McpError> {
	let caller = caller::of(parts)?;
	if !caller.may_triage().await.map_err(mcp_err)? {
		return Err(McpError::invalid_request(
			"changing a triage decision needs an administrator, or a token with the triage grant",
			None,
		));
	}
	Ok(caller)
}

fn check_batch(len: usize, what: &str) -> Result<(), McpError> {
	if len == 0 {
		return Err(McpError::invalid_params(format!("no {what} given"), None));
	}
	if len > MAX_BATCH {
		return Err(McpError::invalid_params(
			format!(
				"{len} {what} given, at most {MAX_BATCH} to a call: send them in smaller batches"
			),
			None,
		));
	}
	Ok(())
}

fn entry_of(arg: EntryArg) -> Result<Entry, AppError> {
	let outcome = Outcome::parse(&arg.outcome).ok_or_else(|| {
		bad_request(format!(
			"invalid outcome: {} (not_affected, accepted_risk or will_fix)",
			arg.outcome
		))
	})?;
	Ok(Entry {
		workspace_packages: arg.workspace_packages,
		outcome,
		reason: arg.reason,
		issue_url: arg.issue_url,
	})
}

fn source_of(value: &str) -> Result<Source, AppError> {
	Source::parse(value).ok_or_else(|| bad_request(format!("invalid source: {value}")))
}

async fn record_one(
	conn: &mut AsyncPgConnection,
	actor: &Actor,
	arg: DecisionArg,
) -> Result<Decision, AppError> {
	let source = source_of(&arg.source)?;
	let entries = arg
		.entries
		.into_iter()
		.map(entry_of)
		.collect::<Result<Vec<_>, _>>()?;
	conn.transaction::<_, AppError, _>(async |conn| {
		let decision = Decision::record(
			conn,
			NewDecision {
				vulnerability_id: arg.id,
				source,
				entries,
				actor: actor.clone(),
				via: Via::Mcp,
			},
		)
		.await?;
		audit::record(conn, actor, &decision.audited()).await?;
		Ok(decision)
	})
	.await
}

async fn withdraw_one(
	conn: &mut AsyncPgConnection,
	actor: &Actor,
	arg: &ItemArg,
	reason: &str,
) -> Result<Withdrawal, AppError> {
	let source = source_of(&arg.source)?;
	conn.transaction::<_, AppError, _>(async |conn| {
		let withdrawal =
			Decision::withdraw(conn, &arg.id, &source, reason, actor.clone(), Via::Mcp).await?;
		audit::record(conn, actor, &withdrawal.audited()).await?;
		Ok(withdrawal)
	})
	.await
}

/// The item as the object it serialises to, cut to the fields asked for.
fn project(item: &ItemSummary, fields: &[String], details: Option<&str>) -> Value {
	let Ok(Value::Object(mut object)) = serde_json::to_value(item) else {
		return Value::Null;
	};
	if !fields.is_empty() {
		object.retain(|key, _| {
			ALWAYS.contains(&key.as_str()) || fields.iter().any(|field| field == key)
		});
	}
	if let Some(details) = details {
		object.insert(DETAILS.to_owned(), Value::String(details.to_owned()));
	}
	Value::Object(object)
}

#[tool_router(router = vulnerabilities_router, vis = "pub(crate)")]
impl CanopyMcp {
	#[tool(
		annotations(read_only_hint = true),
		description = "Vulnerabilities in what the production fleet runs, as the operator \
		               interface lists them: one item per OSV advisory within a source, a page at \
		               a time. A source is a tracked repository (`tamanu`, `canopy`, ...), whose \
		               default branch's dependencies are read, or `linux`, the packages installed \
		               on machines. Each item has `id` and `aliases` (such as a CVE), `summary`, \
		               `severity`, `source`, `published_at`, `packages` (name, version carried, \
		               minimum fix version and scopes), `scopes`, `workspace_packages` that reach \
		               it, `machines` it exposes (Linux), `closed` (nothing counts for it any \
		               more), triage `status` (`open` means it needs triage, or `mixed` for a \
		               split whose entries differ), the current `decision` with its entries, \
		               whether it was `withdrawn` or `edited`, the `lapse` that made it need triage again, the \
		               model's `suggestion`, and `first_seen_at`. The advisory's long `details` \
		               text is left out unless named in `fields` or `include_details` is set. \
		               Filter by source, status, min_severity, scope, package, workspace_package, \
		               has_suggestion and search (matched as in the operator interface); closed \
		               items are left out unless include_closed is set. The result is `items`, \
		               `total` matching in all, and `next_cursor` to pass back for the next page \
		               (absent on the last). `fields` returns only the fields named, plus `id` and \
		               `source`, so a long list stays small. `group_by` returns counts instead of \
		               items, to size the work. The model's suggestions were made WITHOUT access \
		               to source code: each is an inference from the advisory and the package's \
		               place in the dependency tree, to be checked by a caller who has the \
		               source, never a finding about how the code uses the package."
	)]
	async fn find_vulnerabilities(
		&self,
		Parameters(args): Parameters<FindVulnerabilitiesArgs>,
	) -> Result<CallToolResult, McpError> {
		let fields = args.fields.unwrap_or_default();
		if let Some(unknown) = fields
			.iter()
			.find(|field| field.as_str() != DETAILS && !ITEM_FIELDS.contains(&field.as_str()))
		{
			return Err(McpError::invalid_params(
				format!(
					"unknown field {unknown}: the fields are {}, and {DETAILS}",
					ITEM_FIELDS.join(", ")
				),
				None,
			));
		}
		let with_details =
			args.include_details.unwrap_or(false) || fields.iter().any(|field| field == DETAILS);

		let group_by = args
			.group_by
			.as_deref()
			.map(|by| {
				GroupBy::parse(by).ok_or_else(|| {
					McpError::invalid_params(
						format!(
							"invalid group_by: {by} (package, workspace_package, scope, source or status)"
						),
						None,
					)
				})
			})
			.transpose()?;
		let sort = args
			.sort
			.map(|sort| {
				serde_json::from_value::<SortColumn>(Value::String(sort.column.clone()))
					.map(|column| Sort {
						column,
						descending: sort.descending.unwrap_or(false),
					})
					.map_err(|_| invalid("sort column", &sort.column))
			})
			.transpose()?;
		let query = ListQuery {
			source: args.source.as_deref().map(source).transpose()?,
			status: args
				.status
				.as_deref()
				.map(|s| Status::parse(s).ok_or_else(|| invalid("status", s)))
				.transpose()?,
			scope: args.scope,
			min_severity: args
				.min_severity
				.as_deref()
				.map(|s| {
					s.parse::<Severity>()
						.ok()
						.filter(|s| *s != Severity::Unrated)
						.ok_or_else(|| invalid("min_severity", s))
				})
				.transpose()?,
			severity: None,
			has_suggestion: args.has_suggestion,
			include_closed: args.include_closed.unwrap_or(false),
			package: args.package,
			workspace_package: args.workspace_package,
			search: args.search,
			sort,
			cursor: args.cursor,
			page_size: args.page_size,
		};

		let mut conn = self.conn().await?;
		if let Some(by) = group_by {
			let groups = ItemSummary::group(&mut conn, &query, by)
				.await
				.map_err(app_err)?;
			let total = ItemSummary::list(
				&mut conn,
				&ListQuery {
					page_size: Some(1),
					cursor: None,
					..query
				},
			)
			.await
			.map_err(app_err)?
			.total;
			return ok_json(&Grouping { groups, total });
		}

		let listing = ItemSummary::list(&mut conn, &query)
			.await
			.map_err(app_err)?;
		let details = if with_details {
			let ids: Vec<String> = listing.items.iter().map(|item| item.id.clone()).collect();
			Advisory::get_many(&mut conn, &ids)
				.await
				.map_err(mcp_err)?
				.into_iter()
				.map(|advisory| (advisory.id, advisory.details))
				.collect()
		} else {
			std::collections::HashMap::new()
		};
		let items: Vec<Value> = listing
			.items
			.iter()
			.map(|item| {
				project(
					item,
					&fields,
					with_details.then(|| {
						details
							.get(&item.id)
							.map(String::as_str)
							.unwrap_or_default()
					}),
				)
			})
			.collect();
		let mut page = Map::new();
		page.insert("items".into(), Value::Array(items));
		page.insert("total".into(), listing.total.into());
		page.insert(
			"next_cursor".into(),
			listing.next_cursor.map_or(Value::Null, Value::String),
		);
		ok_json(&Value::Object(page))
	}

	#[tool(
		annotations(read_only_hint = true),
		description = "One vulnerability in full, as `item` (as find_vulnerabilities gives it), \
		               the `advisory` (its details, references and affected ranges with the \
		               versions that fix it), and: for a repository item, `packages`, each \
		               affected package on the default branch with its version, minimum fix \
		               version and every path that reaches it (the workspace package, scope, \
		               whether optional, whether direct, and the shortest chain of packages), and \
		               the `commit` of the default branch they were read at; for a linux item, \
		               `machines`, each exposed machine with its group, the package version \
		               installed and the minimum fix version; and `history`, newest first, of \
		               decisions, edits (with the earlier text), withdrawals, lapses and \
		               suggestions, each with who and how. The model's `suggestion` carries its \
		               entries, its `basis` and a `note`: it was made WITHOUT access to source \
		               code, an inference from the advisory and the dependency tree for a caller \
		               with the source to check, not a finding."
	)]
	async fn get_vulnerability(
		&self,
		Parameters(args): Parameters<GetVulnerabilityArgs>,
	) -> Result<CallToolResult, McpError> {
		let source = source(&args.source)?;
		let mut conn = self.conn().await?;
		match ItemDetail::load(&mut conn, &args.id, &source).await {
			Ok(detail) => ok_json(&detail),
			Err(AppError::NotFound(message)) => Ok(not_found(message)),
			Err(e) => Err(app_err(e)),
		}
	}

	#[tool(
		annotations(
			read_only_hint = false,
			destructive_hint = false,
			idempotent_hint = false
		),
		description = "Record triage decisions on vulnerabilities, a batch of at most 100. Each \
		               is an item (`id` and `source`) and `entries`: a whole decision is one entry \
		               with no `workspace_packages`; a split gives each entry the workspace \
		               packages it covers, and every workspace package that reaches the advisory \
		               must be covered, or the split ends in a catch-all entry (no \
		               `workspace_packages`, last), which also covers ones that reach it later. A \
		               linux item takes only a whole decision. An entry's `outcome` is \
		               `not_affected` or `accepted_risk`, each with a `reason`, or `will_fix`, \
		               with an optional reason and an `issue_url`. A decision replaces any \
		               current one, an operator's included, and the history keeps the one it \
		               replaced. Each item is judged on its own: the valid ones are recorded and \
		               audited, and the others refused with the reason and no effect. The result \
		               has one entry per item in the order given, with `recorded`, and the \
		               `decision` or the `error`, and `refused` counts the refused. Only an item \
		               you got wrong is refused; if Canopy itself fails the call ends with an \
		               error instead, and what it recorded before that stands. Decisions are recorded as made through this \
		               interface by the caller. To correct a reason, use \
		               edit_vulnerability_decision rather than recording again. Refused, with no \
		               effect, unless the caller is an administrator (on the operator surface) or \
		               holds a token with the triage grant."
	)]
	async fn record_vulnerability_decisions(
		&self,
		parts: Extension<http::request::Parts>,
		Parameters(args): Parameters<RecordVulnerabilityDecisionsArgs>,
	) -> Result<CallToolResult, McpError> {
		let caller = require_triage(parts).await?;
		check_batch(args.decisions.len(), "decisions")?;

		let actor = caller.actor();
		let mut conn = self.db_write.get().await.map_err(mcp_err)?;
		let mut results = Vec::with_capacity(args.decisions.len());
		for arg in args.decisions {
			let (id, source) = (arg.id.clone(), arg.source.clone());
			let outcome = record_one(&mut conn, &actor, arg).await;
			results.push(batch_result(id, source, outcome, "recorded", "decision")?);
		}
		ok_json(&batch_results(results, "recorded"))
	}

	#[tool(
		annotations(
			read_only_hint = false,
			destructive_hint = false,
			idempotent_hint = false
		),
		description = "Withdraw the current triage decision of each of a batch of at most 100 \
		               vulnerabilities (`items`, each an `id` and `source`), with one `reason` \
		               recorded against each. Each item then needs triage again, with the \
		               suggestion it had, and the history keeps the decision and its withdrawal. \
		               Each item is judged on its own: one with no current decision, or unknown, \
		               is refused with the reason and no effect. The result has one entry per \
		               item in the order given, with `withdrawn`, and the `withdrawal` or the \
		               `error`, and `refused` counts the refused. Only an item you got wrong is \
		               refused; if Canopy itself fails the call ends with an error instead, and \
		               what it withdrew before that stands. Refused, with no effect, unless the caller is an \
		               administrator (on the operator surface) or holds a token with the triage \
		               grant."
	)]
	async fn withdraw_vulnerability_decisions(
		&self,
		parts: Extension<http::request::Parts>,
		Parameters(args): Parameters<WithdrawVulnerabilityDecisionsArgs>,
	) -> Result<CallToolResult, McpError> {
		let caller = require_triage(parts).await?;
		check_batch(args.items.len(), "items")?;

		let actor = caller.actor();
		let mut conn = self.db_write.get().await.map_err(mcp_err)?;
		let mut results = Vec::with_capacity(args.items.len());
		for item in &args.items {
			let outcome = withdraw_one(&mut conn, &actor, item, &args.reason).await;
			results.push(batch_result(
				item.id.clone(),
				item.source.clone(),
				outcome,
				"withdrawn",
				"withdrawal",
			)?);
		}
		ok_json(&batch_results(results, "withdrawn"))
	}

	#[tool(
		annotations(
			read_only_hint = false,
			destructive_hint = false,
			idempotent_hint = false
		),
		description = "Correct the reason or issue link of one vulnerability's current decision, \
		               or of one entry of its split: the entry covering `workspace_package`, or \
		               the catch-all when none is given (a whole decision's only entry). \
		               `reason` and `issue_url` that are absent are left as they are, and a \
		               blank `issue_url` clears it; at least one is given. An edit never changes \
		               an outcome or which packages an entry covers (record a new decision for \
		               that), and an edit that changes nothing is refused. The history keeps the \
		               earlier text. The result is the edit. Refused, with no effect, unless the \
		               caller is an administrator (on the operator surface) or holds a token \
		               with the triage grant."
	)]
	async fn edit_vulnerability_decision(
		&self,
		parts: Extension<http::request::Parts>,
		Parameters(args): Parameters<EditVulnerabilityDecisionArgs>,
	) -> Result<CallToolResult, McpError> {
		let caller = require_triage(parts).await?;
		let source = source(&args.source)?;

		let actor = caller.actor();
		let mut conn = self.db_write.get().await.map_err(mcp_err)?;
		let edit: Result<Edit, AppError> = conn
			.transaction::<_, AppError, _>(async |conn| {
				let edit = Decision::edit(
					conn,
					&args.id,
					&source,
					args.workspace_package.as_deref(),
					args.reason.as_deref(),
					args.issue_url.as_deref(),
					actor.clone(),
					Via::Mcp,
				)
				.await?;
				audit::record(conn, &actor, &edit.audited()).await?;
				Ok(edit)
			})
			.await;
		match edit {
			Ok(edit) => ok_json(&edit),
			Err(AppError::NotFound(message)) => Ok(not_found(message)),
			Err(e) => Err(app_err(e)),
		}
	}
}

/// What the `triage_vulnerabilities` prompt tells the agent to do.
fn triage_guidance(repository: &str) -> String {
	format!(
		"Triage the open vulnerabilities of the `{repository}` repository with the Canopy fleet \
		 query tools. You have this repository's source checked out: the reason to do this work \
		 here is that you can read how the code uses each vulnerable package, which Canopy \
		 cannot.\n\n\
		 1. Size the work. Call find_vulnerabilities with source=`{repository}` and status=`open` \
		 (closed items are left out by default), using group_by to count the items by package, \
		 workspace_package or scope. Then list them with `fields` limited to what you need, such \
		 as id, summary, severity, packages, scopes, workspace_packages and suggestion, and page \
		 through with the cursor. Name `details` in `fields` only for an item whose advisory text \
		 you need.\n\n\
		 2. Order the work as it is easiest for a person to review: group items that share a \
		 package, an advisory family or an area of the code, so one reading of the code settles \
		 several, and put the highest severity first within a group.\n\n\
		 3. For each item, read how the code in this checkout uses the vulnerable package: search \
		 for its imports and the calls to the affected functions the advisory names, and check \
		 whether the affected code path is reachable, in which workspace package, and whether it \
		 ships to production or is only a development or build dependency. Use \
		 get_vulnerability for an item's affected ranges, the paths that reach the package and \
		 the commit Canopy read the dependencies at, and check that your checkout is at or near \
		 that commit.\n\n\
		 4. Canopy's model has already suggested an outcome for some items. Those suggestions \
		 were made WITHOUT access to source code: they are an inference from the advisory and the \
		 dependency tree, and are not findings. Treat each as a hypothesis to check against the \
		 code, never as a reason, and never copy one into a decision unchecked.\n\n\
		 5. Propose decisions to the user in batches of ten items. For each, give the outcome \
		 (not_affected, accepted_risk or will_fix), and a reason that says what you checked: the \
		 files and calls you read and what they showed, not a restatement of the advisory. A will \
		 fix takes the link to the issue tracking the fix where there is one. Where the code \
		 shows that workspace packages differ in how they use the package, propose a split \
		 decision with an entry per group of workspace packages, ending in a catch-all if later \
		 ones should be covered; where it does not, a whole decision. A Linux item takes only a \
		 whole decision.\n\n\
		 6. Record a batch with record_vulnerability_decisions ONLY after the user has confirmed \
		 it, with any changes they asked for. Never record a decision the user has not seen, and \
		 never record on the strength of an earlier batch's confirmation. Each item in the result \
		 is judged on its own: report the ones that were refused with their reasons, and fix \
		 and put those again rather than assuming they were recorded. If recording is refused \
		 for lack of the triage grant, say so and stop.\n\n\
		 7. Leave an item for a person, and say so, when you cannot verify from the code which \
		 outcome is right: the advisory is about something the checkout does not show (a service \
		 outside this repository, a runtime or configuration), the use of the package is too \
		 indirect to trace, or the flaw's trigger depends on how the product is deployed. Do not \
		 guess to clear the list.\n\n\
		 8. To correct the reason or issue link of a decision already recorded, use \
		 edit_vulnerability_decision, and to take one back so it needs triage again, use \
		 withdraw_vulnerability_decisions with a reason. Do not record a new decision just to \
		 reword one."
	)
}

#[prompt_router(router = "vulnerabilities_prompts", vis = "pub(crate)")]
impl CanopyMcp {
	#[prompt(
		name = "triage_vulnerabilities",
		description = "Guides an agent that has a tracked repository's source checked out through \
		               triaging that repository's open vulnerabilities: reading how the code uses \
		               each vulnerable package, proposing decisions to the user in batches of ten, \
		               and recording a batch only once the user has confirmed it."
	)]
	async fn triage_vulnerabilities(
		&self,
		Parameters(args): Parameters<TriageVulnerabilitiesArgs>,
	) -> Result<GetPromptResult, McpError> {
		let repository = REPOSITORIES
			.iter()
			.find(|repository| repository.name == args.repository)
			.ok_or_else(|| {
				McpError::invalid_params(
					format!(
						"unknown repository {}: the tracked repositories are {}",
						args.repository,
						REPOSITORIES
							.iter()
							.map(|repository| repository.name)
							.collect::<Vec<_>>()
							.join(", ")
					),
					None,
				)
			})?;
		Ok(GetPromptResult::new(vec![PromptMessage::new_text(
			Role::User,
			triage_guidance(repository.name),
		)])
		.with_description(format!(
			"Triage the open vulnerabilities of {}",
			repository.name
		)))
	}
}
