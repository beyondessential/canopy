#[cfg(feature = "ui")]
use axum::extract::State;
use canopy_utoipa_axum::router::OpenApiRouter;

use crate::state::AppState;

pub mod applications;
pub mod artifacts;
pub mod backup;
pub mod bestool;
pub mod calendar;
pub mod enrollment;
pub mod machines;
pub mod mcp;
pub mod names;
pub mod openapi;
#[cfg(feature = "ui")]
pub mod password;
pub mod ratelimit;
pub mod restore;
#[cfg(feature = "ui")]
pub mod server_versions;
pub mod state;
pub mod statuses;
pub mod tags;
#[cfg(feature = "ui")]
pub mod timesync;
pub mod versions;

pub fn routes() -> OpenApiRouter<AppState> {
	#[cfg_attr(not(feature = "ui"), expect(unused_mut))]
	let mut router = OpenApiRouter::new()
		.merge(backup::routes())
		.merge(restore::routes())
		.nest("/artifacts", artifacts::routes())
		.nest("/bestool", bestool::routes())
		.nest("/certificates", names::certificate_routes())
		.nest("/names", names::routes())
		.nest("/machines", machines::routes())
		.nest("/applications", applications::alias_routes())
		.nest(
			"/servers",
			// Enrolment is the machine's, but fielded agents call it here.
			applications::routes().merge(enrollment::routes()),
		)
		.nest("/status", statuses::routes())
		.nest("/tags", tags::routes())
		.nest("/versions", versions::routes());

	#[cfg(feature = "ui")]
	{
		use axum::Router;
		use axum::routing::get;
		use tower_http::services::ServeDir;
		let ui_router: Router<AppState> = Router::new()
			.route("/", get(index))
			.route("/errors/{slug}", get(error))
			.merge(timesync::routes())
			.merge(password::routes())
			.nest_service("/static", ServeDir::new("static"))
			// Mount server-versions route (secret is checked in the handler)
			.nest("/server-versions", server_versions::routes());
		router = router.merge(OpenApiRouter::from(ui_router));
	}

	router
}

#[cfg(feature = "ui")]
async fn index(
	State(db): State<database::Db>,
	State(tera): State<std::sync::Arc<tera::Tera>>,
) -> commons_errors::Result<axum::response::Html<String>> {
	use commons_types::version::VersionStatus;
	use database::version_known_issues::VersionKnownIssue;
	use database::versions::Version;
	use serde::Serialize;
	use std::collections::BTreeMap;
	use tera::Context;

	#[derive(Debug, Clone, Serialize)]
	struct VersionData {
		major: i32,
		minor: i32,
		patch: i32,
		status: String,
		created_at: jiff::Timestamp,
		#[serde(rename = "created_at")]
		formatted_created_at: String,
	}

	#[derive(Debug, Clone, Serialize)]
	struct MinorVersionGroup {
		major: i32,
		minor: i32,
		count: usize,
		latest_patch: i32,
		first_created_at: jiff::Timestamp,
		#[serde(rename = "first_created_at")]
		formatted_first_created_at: String,
		versions: Vec<VersionData>,
	}

	let mut db = db.get().await?;
	let versions = Version::get_all_including_drafts(&mut db).await?;

	// Public listing only exposes ready versions, matching the JSON
	// endpoints and the per-version detail pages (which 404 for
	// non-ready versions via latest_matching_ready).
	let version_ids: Vec<_> = versions.iter().map(|v| v.id).collect();
	let affected = VersionKnownIssue::affected_versions(&mut db, &version_ids).await?;
	let versions: Vec<Version> = versions
		.into_iter()
		.filter(|v| !affected.contains(&v.id))
		.collect();

	let mut grouped: BTreeMap<(i32, i32), Vec<Version>> = BTreeMap::new();
	for version in versions {
		grouped
			.entry((version.major, version.minor))
			.or_default()
			.push(version);
	}

	let mut groups: Vec<MinorVersionGroup> = grouped
		.into_iter()
		.filter_map(|((major, minor), mut versions)| {
			// Filter to only published versions
			versions.retain(|v| v.status == VersionStatus::Published);

			// Skip groups with no published versions
			if versions.is_empty() {
				return None;
			}

			versions.sort_by(|a, b| b.patch.cmp(&a.patch));

			let count = versions.len();
			let latest_patch = versions.first().map(|v| v.patch).unwrap_or(0);

			let first_created_at = versions
				.iter()
				.find(|v| v.patch == 0)
				.map(|v| v.created_at)
				.unwrap_or_else(|| versions.last().map(|v| v.created_at).unwrap());

			let formatted_first_created_at = first_created_at.strftime("%Y-%m-%d").to_string();

			let version_data: Vec<VersionData> = versions
				.into_iter()
				.map(|v| VersionData {
					major: v.major,
					minor: v.minor,
					patch: v.patch,
					status: v.status.to_string().to_lowercase(),
					created_at: v.created_at,
					formatted_created_at: v.created_at.strftime("%Y-%m-%d").to_string(),
				})
				.collect();

			Some(MinorVersionGroup {
				major,
				minor,
				count,
				latest_patch,
				first_created_at,
				formatted_first_created_at,
				versions: version_data,
			})
		})
		.collect();

	groups.sort_by(|a, b| b.major.cmp(&a.major).then_with(|| b.minor.cmp(&a.minor)));

	let env = std::env::vars().collect::<std::collections::BTreeMap<String, String>>();
	let mut context = Context::new();
	context.insert("groups", &groups);
	context.insert("env", &env);
	let html = tera.render("versions", &context)?;
	Ok(axum::response::Html(html))
}

#[cfg(feature = "ui")]
async fn error(axum::extract::Path(slug): axum::extract::Path<String>) -> axum::response::Redirect {
	axum::response::Redirect::temporary(&format!(
		"https://github.com/beyondessential/canopy/blob/main/ERRORS.md#{slug}",
	))
}
