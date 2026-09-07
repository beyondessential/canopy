use canopy_utoipa_axum::router::OpenApiRouter;
use serde::{Deserialize, Serialize};

pub mod admins;
pub mod applications;
pub mod backups;
pub mod bestool;
pub mod certificates;
pub mod commons;
pub mod devices;
pub mod domains;
pub mod healthchecks;
pub mod incidents;
pub mod inventory;
pub mod inventory_secrets;
pub mod issues;
pub mod machines;
pub mod maintenance;
pub mod mcp_tokens;
pub mod migration_tests;
pub mod restore_replicas;
pub mod self_alerts;
pub mod server_groups;
pub mod silenced_refs;
pub mod sql;
pub mod statuses;
pub mod upgrade_plans;
pub mod versions;

/// A single page of a paginated list response.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Page<T> {
	/// The items in this page.
	pub items: Vec<T>,
	/// The total number of items across all pages, not just this one — use
	/// this to render page counts without a separate request.
	pub total: u64,
}

/// Generate a 4-word lowercase, hyphen-separated passphrase from the EFF large
/// wordlist (~52 bits of entropy), e.g. `correct-horse-battery-staple`. Used to
/// wrap secrets (enrollment tickets, provisioned device keys) that travel to an
/// operator out of band.
///
/// Four of the 7776 words in that list are themselves hyphenated — `drop-down`,
/// `felt-tip`, `t-shirt`, `yo-yo` — and they are skipped. A passphrase is read
/// off one screen and typed into another, so a word carrying the separator
/// makes the boundaries ambiguous: `disband-retrace-drop-down-bodacious` reads
/// as five words, and the operator cannot tell which four were meant.
///
/// Dropping four words costs about a thousandth of a bit.
pub(crate) fn generate_passphrase() -> String {
	use chbs::prelude::WordProvider;

	/// Built once. Constructing the list parses all 7776 words out of a static
	/// string and taking a sampler clones them again, neither of which is
	/// worth redoing per ticket. Sampling itself draws from a thread-local
	/// CSPRNG, so one shared sampler is fine.
	static SAMPLER: std::sync::LazyLock<chbs::word::WordSampler> =
		std::sync::LazyLock::new(|| chbs::word::WordList::builtin_eff_large().sampler());

	std::iter::repeat_with(|| SAMPLER.word())
		.filter(|word| !word.contains('-'))
		.take(4)
		.collect::<Vec<_>>()
		.join("-")
}

/// Where the fleet's endpoints used to be, redirecting to where they are.
///
/// `servers` named the box and the workload at once, and the fleet's records
/// were spread across three top-level prefixes. Each has its own word now, all
/// three under `/api/fleet`.
///
/// The SPA ships with this server and calls the new prefixes, so this is not
/// compatibility for a client we do not control — it is for a tab left open
/// across a deploy, and for a bookmarked or scripted call.
///
/// 308 rather than 302: these are all POSTs, and only the permanent form
/// obliges the caller to repeat the method and body rather than turning the
/// retry into a GET. Not documented in the schema — the schema says where an
/// endpoint is, and one path per endpoint is the point of moving them.
fn moved_paths() -> axum::Router<crate::state::AppState> {
	use axum::{
		extract::{OriginalUri, Path},
		response::Redirect,
		routing::any,
	};

	/// The query string is carried through: nothing here uses one today, but
	/// dropping half a URL on the way to a redirect is the kind of thing that
	/// is found much later.
	fn moved_to(prefix: &str, rest: &str, uri: &axum::http::Uri) -> Redirect {
		let query = uri.query().map(|q| format!("?{q}")).unwrap_or_default();
		Redirect::permanent(&format!("/api/fleet/{prefix}/{rest}{query}"))
	}

	async fn applications(Path(rest): Path<String>, OriginalUri(uri): OriginalUri) -> Redirect {
		moved_to("applications", &rest, &uri)
	}
	async fn machines(Path(rest): Path<String>, OriginalUri(uri): OriginalUri) -> Redirect {
		moved_to("machines", &rest, &uri)
	}
	async fn groups(Path(rest): Path<String>, OriginalUri(uri): OriginalUri) -> Redirect {
		moved_to("groups", &rest, &uri)
	}

	axum::Router::new()
		// `servers` was the applications' prefix before the split gave the
		// workload its own word.
		.route("/api/servers/{*rest}", any(applications))
		.route("/api/applications/{*rest}", any(applications))
		.route("/api/machines/{*rest}", any(machines))
		.route("/api/server_groups/{*rest}", any(groups))
}

pub fn routes() -> OpenApiRouter<crate::state::AppState> {
	OpenApiRouter::new()
		.nest(
			"/api",
			OpenApiRouter::new()
				.nest("/admins", admins::routes())
				// The fleet's own records, under one prefix — the same shape the
				// SPA browses them at. `servers` used to name the box and the
				// workload at once, and the listings sat beside the records.
				.nest(
					"/fleet",
					OpenApiRouter::new()
						.nest("/applications", applications::routes())
						.nest("/machines", machines::routes())
						.nest("/groups", server_groups::routes()),
				)
				.nest("/backups", backups::routes())
				.nest("/bestool", bestool::routes())
				.nest("/certificates", certificates::routes())
				.nest("/commons", commons::routes())
				.nest("/devices", devices::routes())
				.nest("/domains", domains::routes())
				.nest("/healthchecks", healthchecks::routes())
				.nest("/incidents", incidents::routes())
				.nest("/inventory", inventory::routes())
				.nest("/inventory_secrets", inventory_secrets::routes())
				.nest("/issues", issues::routes())
				.nest("/mcp_tokens", mcp_tokens::routes())
				.nest("/migration_tests", migration_tests::routes())
				.nest("/restore_replicas", restore_replicas::routes())
				.nest("/self_alerts", self_alerts::routes())
				.nest("/maintenance", maintenance::routes())
				.nest("/silenced_refs", silenced_refs::routes())
				.nest("/sql", sql::routes())
				.nest("/statuses", statuses::routes())
				.nest("/upgrade_plans", upgrade_plans::routes())
				.nest("/versions", versions::routes()),
		)
		// Merged outside the `/api` nest: these routes carry the prefix
		// themselves, so nesting them would put it on twice.
		.merge(OpenApiRouter::from(moved_paths()))
}

#[cfg(test)]
mod tests {
	use super::generate_passphrase;

	/// The generator's contract is four words separated by hyphens, so the
	/// separator must never appear inside a word. The EFF large wordlist has
	/// four hyphenated entries out of 7776, which a passphrase hits about one
	/// time in five hundred — rare enough to pass review and pass CI for a
	/// while, then fail somewhere unrelated. Enough draws to make that
	/// certain rather than likely.
	#[test]
	fn a_passphrase_is_always_four_hyphen_separated_words() {
		for _ in 0..20_000 {
			let passphrase = generate_passphrase();
			let words: Vec<&str> = passphrase.split('-').collect();
			assert_eq!(
				words.len(),
				4,
				"a hyphenated word made the boundaries ambiguous: {passphrase}"
			);
			assert!(
				words
					.iter()
					.all(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_lowercase())),
				"words are non-empty and lowercase: {passphrase}"
			);
		}
	}
}
