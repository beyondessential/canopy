use std::str::FromStr as _;
#[cfg(feature = "ui")]
use std::sync::Arc;

#[cfg(feature = "ui")]
use axum::response::Html;
use axum::{
	Json,
	body::{Body, Bytes},
	extract::{Path, State},
	http::{StatusCode, header},
	response::IntoResponse,
	routing::{Router, get},
};
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::device_auth::{AdminDevice, AuthDevice, ReleaserDevice};
use commons_types::version::{VersionRange, VersionStr};
use database::{
	Db,
	artifacts::{Artifact as ArtifactRow, Scope},
	version_known_issues::VersionKnownIssue,
	versions::{NewVersion, Version, ViewVersion},
};
use diesel::{
	BoolExpressionMethods as _, ExpressionMethods as _, QueryDsl as _, SelectableHelper as _,
};
use diesel_async::{AsyncPgConnection, RunQueryDsl as _};
use futures::AsyncReadExt;
#[cfg(feature = "ui")]
use pulldown_cmark::{Options, Parser, html};
#[cfg(feature = "ui")]
use qrcode::{QrCode, render::svg};
#[cfg(feature = "ui")]
use serde::{Deserialize, Serialize};
#[cfg(feature = "ui")]
use tera::{Context, Tera};

use crate::{
	artifacts::{Artifact, caller_scope},
	state::AppState,
};

/// Drop versions that any known issue's range still covers. The public
/// site never serves these — the admin UI shows them, but clients only
/// see what's been vouched for.
async fn filter_ready(
	conn: &mut AsyncPgConnection,
	versions: Vec<Version>,
) -> Result<Vec<Version>> {
	let ids: Vec<_> = versions.iter().map(|v| v.id).collect();
	let affected = VersionKnownIssue::affected_versions(conn, &ids).await?;
	Ok(versions
		.into_iter()
		.filter(|v| !affected.contains(&v.id))
		.collect())
}

/// Pick the latest *ready* version that satisfies `range`. Mirrors
/// `Version::get_latest_matching` but skips versions any known issue
/// still covers.
async fn latest_matching_ready(
	conn: &mut AsyncPgConnection,
	range: node_semver::Range,
) -> Result<Version> {
	use database::schema::versions::*;

	let floor = range.min_version().ok_or(AppError::UnusableRange)?;

	let candidates: Vec<Version> = table
		.select(Version::as_select())
		.filter(status.eq(commons_types::version::VersionStatus::Published))
		.filter(database::versions::at_or_above_version(&floor))
		.order_by(major.desc())
		.then_order_by(minor.desc())
		.then_order_by(patch.desc())
		.load(conn)
		.await
		.map_err(AppError::from)?;

	let ids: Vec<_> = candidates.iter().map(|v| v.id).collect();
	let affected = VersionKnownIssue::affected_versions(conn, &ids).await?;

	candidates
		.into_iter()
		.filter(|v| !affected.contains(&v.id))
		.find(|v| range.satisfies(&v.as_semver()))
		.ok_or(AppError::NoMatchingVersions)
}

pub fn routes() -> OpenApiRouter<AppState> {
	let api = OpenApiRouter::new()
		.routes(routes!(list))
		.routes(routes!(update_for))
		.routes(routes!(create, remove))
		.routes(routes!(list_artifacts));

	// Streaming download proxy doesn't have a JSON wire shape; mount as a
	// plain route so it stays out of the OpenAPI spec.
	let extras: Router<AppState> = Router::new().route(
		"/{version}/artifacts/{artifact_id}/download",
		get(download_artifact),
	);

	#[cfg(feature = "ui")]
	let extras = extras
		.route("/rss", get(releases_rss))
		.route("/{version}", get(view_artifacts))
		.route("/{version}/mobile", get(view_mobile_install));

	api.merge(OpenApiRouter::from(extras))
}

#[cfg(feature = "ui")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ArtifactWithQR {
	#[serde(flatten)]
	artifact: Artifact,
	qr_code_svg: String,
}

#[cfg(feature = "ui")]
impl From<Artifact> for ArtifactWithQR {
	fn from(artifact: Artifact) -> Self {
		let code = QrCode::new(&artifact.download_url).expect("Failed to generate QR code");
		let svg_image = code
			.render::<svg::Color>()
			.min_dimensions(100, 100)
			.dark_color(svg::Color("#000000"))
			.light_color(svg::Color("#ffffff"))
			.build();

		Self {
			artifact,
			qr_code_svg: svg_image,
		}
	}
}

#[cfg(feature = "ui")]
pub fn parse_markdown(text: &str) -> String {
	let mut options = Options::empty();
	options.insert(Options::ENABLE_FOOTNOTES);
	options.insert(Options::ENABLE_GFM);
	options.insert(Options::ENABLE_SMART_PUNCTUATION);
	options.insert(Options::ENABLE_STRIKETHROUGH);
	options.insert(Options::ENABLE_TABLES);
	let parser = Parser::new_ext(text, options);
	let mut html_output = String::new();
	html::push_html(&mut html_output, parser);
	html_output
}

/// List published, ready-to-serve versions.
///
/// Returns every version currently in the published state, excluding any
/// version a recorded known-issue range still covers (whether that issue
/// is still open or has since been fixed in a later patch). Ordered
/// newest first.
#[utoipa::path(
	get,
	path = "/",
	operation_id = "list_versions",
	tag = "versions",
	responses(
		(status = 200, description = "All published versions.", body = Vec<Version>),
	),
)]
async fn list(State(db): State<Db>) -> Result<Json<Vec<Version>>> {
	let mut db = db.get().await?;
	let versions = Version::get_all(&mut db).await?;
	let versions = filter_ready(&mut db, versions).await?;
	Ok(Json(versions))
}

/// Base URL for absolute links Canopy emits about itself. Prefers the
/// configured `PUBLIC_URL`; otherwise reconstructs the origin from the
/// request's forwarded scheme and `Host` header so local and test runs still
/// emit well-formed links.
pub(crate) fn public_base_url(headers: &axum::http::HeaderMap) -> String {
	if let Ok(url) = std::env::var("PUBLIC_URL") {
		let trimmed = url.trim_end_matches('/');
		if !trimmed.is_empty() {
			return trimmed.to_owned();
		}
	}

	let scheme = headers
		.get("x-forwarded-proto")
		.and_then(|v| v.to_str().ok())
		.unwrap_or("https");
	let host = headers
		.get(header::HOST)
		.and_then(|v| v.to_str().ok())
		.unwrap_or("localhost");
	format!("{scheme}://{host}")
}

/// RSS 2.0 feed of published releases.
///
/// Emits one item per published, ready-to-serve version (the same set as
/// the `/versions` listing), newest first, with the changelog rendered to
/// HTML as the item content. Feed readers poll this to learn about new
/// releases.
#[cfg(feature = "ui")]
async fn releases_rss(
	State(db): State<Db>,
	headers: axum::http::HeaderMap,
) -> Result<impl IntoResponse> {
	use rss::{ChannelBuilder, GuidBuilder, ItemBuilder};

	let mut db = db.get().await?;
	let versions = Version::get_all(&mut db).await?;
	let versions = filter_ready(&mut db, versions).await?;

	let base = public_base_url(&headers);

	let items: Vec<rss::Item> = versions
		.into_iter()
		.map(|v| {
			let version = format!("{}.{}.{}", v.major, v.minor, v.patch);
			let link = format!("{base}/versions/{version}");
			// RFC 2822 date, in UTC (jiff timestamps carry no offset).
			let pub_date = v
				.created_at
				.strftime("%a, %d %b %Y %H:%M:%S %z")
				.to_string();
			ItemBuilder::default()
				.title(format!("Canopy {version}"))
				.link(link.clone())
				.guid(GuidBuilder::default().value(link).permalink(true).build())
				.pub_date(pub_date)
				.description(parse_markdown(&v.changelog))
				.build()
		})
		.collect();

	let channel = ChannelBuilder::default()
		.title("Canopy releases")
		.link(format!("{base}/"))
		.description("Latest published Canopy releases and their changelogs.")
		.items(items)
		.build();

	Ok((
		[(header::CONTENT_TYPE, "application/rss+xml; charset=utf-8")],
		channel.to_string(),
	))
}

/// Publish a version with its changelog.
///
/// Requires a device certificate with the releaser role (or admin). The
/// path parameter is the exact version being published (e.g. `2.10.5`);
/// the request body is the changelog for that version, as up to 1 MiB of
/// markdown text.
///
/// If the version already exists in the draft state — for example
/// because an artifact was registered against it before its changelog
/// was written — the draft is published in place, with this changelog
/// replacing whatever it had before. Otherwise a new version is created
/// directly in the published state. Publishing a version that already
/// exists and is not a draft (already published, or yanked) fails.
///
/// Returns the resulting version record.
#[utoipa::path(
	post,
	path = "/{version}",
	operation_id = "create_version",
	tag = "versions",
	security(("releaser-device" = [])),
	params(("version" = String, Path)),
	request_body(content = String, description = "Changelog markdown text, up to 1 MiB."),
	responses(
		(status = 200, body = Version),
		(status = 400, body = ProblemDetailsSchema),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
async fn create(
	device: ReleaserDevice,
	Path(version): Path<String>,
	State(db): State<Db>,
	data: Bytes,
) -> Result<Json<Version>> {
	use commons_types::version::VersionStatus;

	let mut db = db.get().await?;
	const CHANGELOG_LIMIT: usize = 1024 * 1024; // a MiB, as documented

	let mut stream = data.take(CHANGELOG_LIMIT as u64);
	let mut changelog = String::with_capacity(data.len().min(CHANGELOG_LIMIT));
	stream.read_to_string(&mut changelog).await?;
	let version_str = VersionStr::from_str(&version)?;
	let device_id = device.0.0.id;

	// Check if a draft version already exists
	let version = match Version::get_by_version(&mut db, version_str.clone()).await {
		Ok(existing_version) if existing_version.status == VersionStatus::Draft => {
			// Update the draft to published and replace the changelog
			Version::update_status(&mut db, version_str.clone(), VersionStatus::Published).await?;
			Version::update_changelog(&mut db, version_str.clone(), changelog).await?;
			Version::update_device_id(&mut db, version_str.clone(), device_id).await?;
			Version::get_by_version(&mut db, version_str).await?
		}
		Ok(_) => {
			// Version exists but is not a draft, let the insert fail with constraint violation
			diesel::insert_into(database::schema::versions::table)
				.values(NewVersion {
					major: version_str.0.major as _,
					minor: version_str.0.minor as _,
					patch: version_str.0.patch as _,
					changelog,
					status: VersionStatus::Published,
					device_id: Some(device_id),
				})
				.returning(Version::as_select())
				.get_result(&mut db)
				.await?
		}
		Err(_) => {
			// Version doesn't exist, create it as published
			diesel::insert_into(database::schema::versions::table)
				.values(NewVersion {
					major: version_str.0.major as _,
					minor: version_str.0.minor as _,
					patch: version_str.0.patch as _,
					changelog,
					status: VersionStatus::Published,
					device_id: Some(device_id),
				})
				.returning(Version::as_select())
				.get_result(&mut db)
				.await?
		}
	};

	Ok(Json(version))
}

/// Yank a version.
///
/// Requires a device certificate with the admin role. Marks the given
/// exact version as yanked, hiding it from listings, update checks, and
/// artifact lookups without deleting its history.
#[utoipa::path(
	delete,
	path = "/{version}",
	tag = "versions",
	security(("admin-device" = [])),
	params(("version" = String, Path)),
	responses(
		(status = 200, description = "Version marked yanked."),
		(status = 400, body = ProblemDetailsSchema),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
async fn remove(
	_device: AdminDevice,
	Path(version): Path<String>,
	State(db): State<Db>,
) -> Result<()> {
	use commons_types::version::VersionStatus;
	use database::schema::versions::dsl::*;

	let mut db = db.get().await?;
	let version = VersionStr::from_str(&version)?;
	diesel::update(versions)
		.filter(database::versions::predicate_version!(version.0))
		.set(status.eq(VersionStatus::Yanked))
		.execute(&mut db)
		.await?;

	Ok(())
}

#[cfg(feature = "ui")]
async fn view_artifacts(
	Path(version): Path<String>,
	State(db): State<Db>,
	State(tera): State<Arc<Tera>>,
	headers: axum::http::HeaderMap,
) -> Result<Html<String>> {
	use commons_types::version::VersionStatus;
	use diesel::QueryDsl;
	use serde::Serialize;

	#[derive(Debug, Clone, Serialize)]
	struct VersionForTemplate {
		#[serde(flatten)]
		version: Version,
		created_at_date: String,
		min_chrome_version: Option<u32>,
		related_versions: Vec<RelatedVersion>,
	}

	#[derive(Debug, Clone, Serialize)]
	struct RelatedVersion {
		major: i32,
		minor: i32,
		patch: i32,
		changelog: String,
	}

	let mut db = db.get().await?;
	let version = VersionRange::from_str(&version)?;
	let mut version = latest_matching_ready(&mut db, version.0).await?;
	version.changelog = parse_markdown(&version.changelog);
	let artifacts = offered_artifacts(&mut db, &version, Scope::Unscoped, &headers).await?;

	// The latest *ready* version in this minor. The page's own version came
	// through `latest_matching_ready`, so the banner has to use the same set:
	// a version a known issue still covers is hidden from every listing and
	// its own page 404s, so recommending one — and linking to it — sends the
	// reader nowhere.
	let latest_in_minor = {
		use database::schema::versions::dsl::*;
		let published: Vec<Version> = versions
			.filter(major.eq(version.major))
			.filter(minor.eq(version.minor))
			.filter(status.eq(VersionStatus::Published))
			.order_by(patch.desc())
			.select(Version::as_select())
			.load(&mut db)
			.await
			.map_err(AppError::from)?;
		// `filter_ready` keeps the order it's given, so the first survivor is
		// the highest ready patch.
		filter_ready(&mut db, published).await?.into_iter().next()
	};

	let is_latest = latest_in_minor
		.as_ref()
		.map(|v| v.patch == version.patch)
		.unwrap_or(true);

	let latest_version_str =
		latest_in_minor.map(|v| format!("{}.{}.{}", v.major, v.minor, v.patch));

	let created_at_date = version.created_at.strftime("%Y-%m-%d").to_string();

	// Compute min chrome version based on head release date (X.Y.0)
	let min_chrome_version = if let Ok(head_release_date) =
		Version::get_head_release_date(&mut db, VersionStr(version.as_semver())).await
	{
		database::chrome_releases::ChromeRelease::get_min_version_at_date(
			&mut db,
			head_release_date,
		)
		.await
		.ok()
		.flatten()
	} else {
		None
	};

	// Get all lower patch versions in this minor release
	let related_versions = Version::get_all_in_minor(&mut db, VersionStr(version.as_semver()))
		.await
		.unwrap_or_default()
		.into_iter()
		.map(|mut v| {
			v.changelog = parse_markdown(&v.changelog);
			RelatedVersion {
				major: v.major,
				minor: v.minor,
				patch: v.patch,
				changelog: v.changelog,
			}
		})
		.collect();

	let version_for_template = VersionForTemplate {
		version,
		created_at_date,
		min_chrome_version,
		related_versions,
	};

	let mut context = Context::new();
	context.insert("version", &version_for_template);
	context.insert("artifacts", &artifacts);
	context.insert("is_latest", &is_latest);
	context.insert("latest_version", &latest_version_str);
	Ok(Html(tera.render("artifacts", &context)?))
}

/// List the artifacts available for a version or version range.
///
/// The path parameter accepts either an exact version or a semver range
/// pattern (e.g. `2.10.x`, `^2.10.0`). It resolves to the latest
/// published, ready version satisfying the input, then returns that
/// version's artifacts — both ones registered against the exact version
/// and ones registered against a range pattern that covers it. Returns
/// 404 if no published, ready version matches.
#[utoipa::path(
	get,
	path = "/{version}/artifacts",
	tag = "versions",
	params(("version" = String, Path)),
	responses(
		(status = 200, description = "Artifacts that match the given exact version or range.", body = Vec<Artifact>),
		(status = 400, body = ProblemDetailsSchema),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
async fn list_artifacts(
	device: Option<AuthDevice>,
	Path(version): Path<String>,
	State(db): State<Db>,
	headers: axum::http::HeaderMap,
) -> Result<Json<Vec<Artifact>>> {
	let mut db = db.get().await?;
	let version = VersionRange::from_str(&version)?;
	let version = latest_matching_ready(&mut db, version.0).await?;
	let scope = caller_scope(&mut db, device).await?;

	Ok(Json(
		offered_artifacts(&mut db, &version, scope, &headers).await?,
	))
}

/// The artifacts of a version as `scope` is offered them.
async fn offered_artifacts(
	db: &mut AsyncPgConnection,
	version: &Version,
	scope: Scope,
	headers: &axum::http::HeaderMap,
) -> Result<Vec<Artifact>> {
	let base = public_base_url(headers);
	let shown = version.as_semver().to_string();

	Ok(ArtifactRow::get_for_version(db, version.id, scope)
		.await?
		.into_iter()
		.map(|row| Artifact::offered(row, &base, &shown))
		.collect())
}

#[cfg(feature = "ui")]
async fn view_mobile_install(
	Path(version): Path<String>,
	State(db): State<Db>,
	State(tera): State<Arc<Tera>>,
	headers: axum::http::HeaderMap,
) -> Result<Html<String>> {
	let mut db = db.get().await?;
	let version = VersionRange::from_str(&version)?;
	let version = latest_matching_ready(&mut db, version.0).await?;
	let artifacts = offered_artifacts(&mut db, &version, Scope::Unscoped, &headers)
		.await?
		.into_iter()
		.filter(|a| a.artifact_type == "mobile")
		.map(ArtifactWithQR::from)
		.collect::<Vec<_>>();

	let mut context = Context::new();
	context.insert("version", &version);
	context.insert("artifacts", &artifacts);
	Ok(Html(tera.render("mobile", &context)?))
}

/// Check for available updates from a given version.
///
/// The path parameter is the caller's currently-installed exact version.
/// For each later minor release line within the same major version,
/// returns the latest published version that hasn't been excluded by a
/// recorded known-issue range — falling back to an older ready patch
/// within that same minor line rather than dropping the line entirely, if
/// the newest patch isn't ready. Clients use this to discover and offer
/// available updates.
#[utoipa::path(
	get,
	path = "/update-for/{version}",
	tag = "versions",
	params(("version" = String, Path, description = "Currently-installed version (exact semver).")),
	responses(
		(status = 200, description = "Published versions above the given one, used by clients to discover updates.", body = Vec<ViewVersion>),
		(status = 400, body = ProblemDetailsSchema),
	),
)]
async fn update_for(
	State(db): State<Db>,
	Path(version): Path<String>,
) -> Result<Json<Vec<ViewVersion>>> {
	use commons_types::version::VersionStatus;
	use database::schema::versions::dsl::*;
	use std::collections::HashMap;

	let mut db = db.get().await?;
	let target = VersionStr::from_str(&version)?.0;

	// Pull all candidate updates within the same major, ahead of the
	// caller's exact version.
	let candidates: Vec<Version> = versions
		.filter(major.eq(target.major as i32))
		.filter(status.eq(VersionStatus::Published))
		.filter(
			minor.gt(target.minor as i32).or(minor
				.eq(target.minor as i32)
				.and(patch.gt(target.patch as i32))),
		)
		.select(Version::as_select())
		.load(&mut db)
		.await?;

	// Filter to ready THEN reduce to latest-per-(major,minor); doing this
	// in this order means a not-ready latest patch falls back to an older
	// ready one within the same minor (instead of dropping the minor).
	let candidates = filter_ready(&mut db, candidates).await?;
	let mut latest: HashMap<(i32, i32), Version> = HashMap::new();
	for v in candidates {
		let key = (v.major, v.minor);
		latest
			.entry(key)
			.and_modify(|cur| {
				if v.patch > cur.patch {
					*cur = v.clone();
				}
			})
			.or_insert(v);
	}

	let mut out: Vec<ViewVersion> = latest
		.into_values()
		.map(|v| ViewVersion {
			id: v.id,
			major: v.major,
			minor: v.minor,
			patch: v.patch,
			status: v.status,
			changelog: v.changelog,
		})
		.collect();
	out.sort_by_key(|v| (v.major, v.minor));
	Ok(Json(out))
}

async fn download_artifact(
	device: Option<AuthDevice>,
	State(db): State<Db>,
	Path((version, artifact_id)): Path<(String, String)>,
) -> Result<impl IntoResponse> {
	use uuid::Uuid;

	let mut db = db.get().await?;
	let version = VersionRange::from_str(&version)?;
	let version = latest_matching_ready(&mut db, version.0).await?;
	let scope = caller_scope(&mut db, device).await?;

	let artifact_uuid = Uuid::parse_str(&artifact_id)
		.map_err(|_| AppError::BadRequest("Invalid artifact ID".into()))?;

	// Resolution is what enforces the boundary: an artifact scoped to a group
	// this caller is not offered is simply not in the set, so it is missing in
	// exactly the way an artifact that never existed is.
	// spec: ART#who-is-offered-a-group-scoped-artifact
	let artifacts = ArtifactRow::get_for_version(&mut db, version.id, scope).await?;
	let artifact = artifacts
		.into_iter()
		.find(|a| a.id == artifact_uuid)
		.ok_or(AppError::ArtifactNotFound)?;

	if let Some(held) = ArtifactRow::content_for(&mut db, artifact.id).await? {
		let recomputed = database::artifacts::digest_of(&held.bytes);
		if recomputed != held.digest {
			tracing::error!(
				artifact = %artifact.id,
				"held artifact does not match its digest; refusing to serve"
			);
			return Err(AppError::ArtifactDigestMismatch);
		}

		let content_type = held
			.content_type
			.unwrap_or_else(|| "application/octet-stream".to_owned());

		return Ok((
			StatusCode::OK,
			[(header::CONTENT_TYPE, content_type)],
			Body::from(held.bytes),
		)
			.into_response());
	}

	let Some(download_url) = artifact.download_url else {
		return Err(AppError::ArtifactNotFound);
	};

	let client = reqwest::Client::builder()
		.build()
		.map_err(|err| AppError::custom(format!("failed to build HTTP client: {err}")))?;
	let response = client
		.get(&download_url)
		.send()
		.await
		.map_err(|err| AppError::custom(format!("Failed to download artifact: {err}")))?;

	let status = response.status();
	let content_type = response
		.headers()
		.get(header::CONTENT_TYPE)
		.and_then(|v| v.to_str().ok())
		.unwrap_or("application/octet-stream")
		.to_string();

	let body = Body::from_stream(response.bytes_stream());

	Ok((status, [(header::CONTENT_TYPE, content_type)], body).into_response())
}
