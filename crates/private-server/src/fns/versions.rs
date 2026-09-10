use std::collections::BTreeMap;
use std::str::FromStr;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Query, State};
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::{TailscaleAdmin, TailscaleUser};
use commons_types::version::{VersionStatus, VersionStr};
use database::{
	artifacts::{
		Artifact, MAX_HELD_ARTIFACT_BYTES, NewArtifact, Scope, digest_of, parse_sri, parse_sri_opt,
		sri,
	},
	server_groups::ServerGroup,
	version_known_issues::VersionKnownIssue,
	versions::Version,
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::state::AppState;

/// Header the SPA sets on an upload, which no cross-origin page can send
/// without the browser preflighting the request first.
const FETCH_HEADER: &str = "x-canopy-upload";

/// Body budget for `upload_artifact`. Sizing above the cap keeps an over-limit
/// upload the handler's structured refusal rather than axum's plain-text 413.
const MAX_UPLOAD_ARTIFACT_BODY_BYTES: usize = MAX_HELD_ARTIFACT_BYTES + 64 * 1024;

/// A single released (or draft) software version.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VersionData {
	/// Major version number.
	pub major: i32,
	/// Minor version number.
	pub minor: i32,
	/// Patch version number.
	pub patch: i32,
	/// Publication status of this version (e.g. draft, published, yanked).
	pub status: VersionStatus,
	/// When this version was created.
	pub created_at: Timestamp,
	/// `true` when this version has no unresolved known issues.
	pub ready: bool,
}

/// A group of versions sharing the same major.minor release line, with a
/// summary of the line's overall readiness.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MinorVersionGroup {
	/// Major version number for this release line.
	pub major: i32,
	/// Minor version number for this release line.
	pub minor: i32,
	/// Number of patch versions in this release line.
	pub count: usize,
	/// Highest published patch number in this release line, or 0 if none
	/// are published yet.
	pub latest_patch: i32,
	/// Creation time of the first (patch 0) published version in this
	/// release line. Falls back to the earliest published patch, or the
	/// current time, if patch 0 hasn't been published.
	pub first_created_at: Timestamp,
	/// Creation time of the latest published patch in this release line,
	/// or the current time if nothing has been published yet.
	pub last_created_at: Timestamp,
	/// All versions in this release line, patch descending.
	pub versions: Vec<VersionData>,
	/// `true` when the latest published patch in this release line is
	/// itself ready. An old, since-fixed issue on an earlier patch doesn't
	/// dim the whole release line. Also `true` when nothing has been
	/// published yet.
	pub ready: bool,
}

/// Full detail for a single version, including its changelog, related
/// versions in the same release line, and known issues.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VersionDetail {
	/// Unique identifier for this version.
	pub id: Uuid,
	/// Major version number.
	pub major: i32,
	/// Minor version number.
	pub minor: i32,
	/// Patch version number.
	pub patch: i32,
	/// Publication status of this version (e.g. draft, published, yanked).
	pub status: VersionStatus,
	/// When this version was created.
	pub created_at: Timestamp,
	/// When this version was last updated.
	pub updated_at: Timestamp,
	/// Changelog text for this version.
	pub changelog: String,
	/// Minimum embedded browser version required by this version, if
	/// known.
	pub min_chrome_version: Option<u32>,
	/// `true` when this is the highest patch published in its release
	/// line.
	pub is_latest_in_minor: bool,
	/// Other versions in the same major.minor release line.
	pub related_versions: Vec<RelatedVersionData>,
	/// `true` when this exact version has no unresolved known issues.
	/// Issues fixed at or before this patch don't count against it.
	pub ready: bool,
	/// Every known issue ever raised against this version's release line,
	/// resolved or not.
	pub known_issues: Vec<KnownIssueData>,
}

/// A caveat or defect recorded against a range of patches within a release
/// line.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct KnownIssueData {
	/// Unique identifier for this known issue.
	pub id: Uuid,
	/// When this known issue was recorded.
	pub created_at: Timestamp,
	/// Login of the operator who recorded this known issue.
	pub author: String,
	/// Description of the issue.
	pub description: String,
	/// Major version of the first affected patch.
	pub min_major: i32,
	/// Minor version of the first affected patch.
	pub min_minor: i32,
	/// Patch number of the first affected patch.
	pub min_patch: i32,
	/// Major version of the first unaffected (fixed) patch, if resolved.
	/// Absent while unresolved — an open issue implicitly covers every
	/// patch from the minimum affected patch to the end of its release
	/// line.
	pub max_major: Option<i32>,
	/// Minor version of the first unaffected (fixed) patch, if resolved.
	pub max_minor: Option<i32>,
	/// Patch number of the first unaffected (fixed) patch, if resolved.
	pub max_patch: Option<i32>,
	/// When this issue was resolved, if it has been.
	pub resolved_at: Option<Timestamp>,
	/// Login of the operator who resolved this issue, if any.
	pub resolved_by: Option<String>,
	/// Explanation given when resolving this issue, if any.
	pub resolution_message: Option<String>,
}

impl From<VersionKnownIssue> for KnownIssueData {
	fn from(k: VersionKnownIssue) -> Self {
		Self {
			id: k.id,
			created_at: k.created_at,
			author: k.author,
			description: k.description,
			min_major: k.min_major,
			min_minor: k.min_minor,
			min_patch: k.min_patch,
			max_major: k.max_major,
			max_minor: k.max_minor,
			max_patch: k.max_patch,
			resolved_at: k.resolved_at,
			resolved_by: k.resolved_by,
			resolution_message: k.resolution_message,
		}
	}
}

/// Another patch version within the same release line as a version being
/// viewed.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RelatedVersionData {
	/// Major version number.
	pub major: i32,
	/// Minor version number.
	pub minor: i32,
	/// Patch version number.
	pub patch: i32,
	/// Changelog text for this version.
	pub changelog: String,
}

/// A downloadable artifact (for example an installer) associated with a
/// version, either tied to that exact version or matched via a version
/// range pattern.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ArtifactData {
	/// Unique identifier for this artifact.
	pub id: Uuid,
	/// Kind of artifact (for example, an installer or update package).
	pub artifact_type: String,
	/// Target platform this artifact is built for.
	pub platform: String,
	/// URL clients use to download this artifact. `null` when Canopy holds
	/// the bytes itself.
	pub download_url: Option<String>,
	/// The group this artifact is for, when it is for one alone.
	pub group_id: Option<Uuid>,
	/// Name of that group, for display.
	pub group_name: Option<String>,
	/// Subresource Integrity digest recorded for the artifact, where there is
	/// one.
	pub digest: Option<String>,
	/// `true` when Canopy holds this artifact's bytes rather than a location.
	pub canopy_holds_bytes: bool,
	/// `true` when this artifact is tied to the exact version being
	/// queried; `false` when it was matched via a version range pattern
	/// instead.
	pub is_exact: bool,
	/// Version range pattern this artifact applies to, if it's a
	/// range-matched artifact rather than an exact one.
	pub version_range_pattern: Option<String>,
	/// Only meaningful when `is_exact` is `true`: `true` when a
	/// range-matched artifact of the same type and platform also matches
	/// this version, and would be served instead if this exact artifact
	/// were removed.
	pub has_range_override: bool,
	/// `true` when this is the artifact actually served to public API
	/// clients requesting a download for this version.
	pub is_used_in_public_api: bool,
}

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(get_grouped_versions))
		.routes(routes!(get_version_detail))
		.routes(routes!(get_version_artifacts))
		.routes(routes!(update_version_status))
		.routes(routes!(update_version_changelog))
		.routes(routes!(update_artifact))
		.routes(routes!(create_artifact))
		.merge(
			OpenApiRouter::new()
				.routes(routes!(upload_artifact))
				.layer(DefaultBodyLimit::max(MAX_UPLOAD_ARTIFACT_BODY_BYTES)),
		)
		.routes(routes!(delete_artifact))
		.routes(routes!(list_known_issues))
		.routes(routes!(add_known_issue))
		.routes(routes!(resolve_known_issue))
}

/// List all versions, grouped by release line.
///
/// Returns every version, including drafts, grouped by major.minor release
/// line and ordered newest release line first. Each group includes a
/// readiness flag reflecting whether its latest published patch has any
/// unresolved known issues.
#[utoipa::path(
	post,
	path = "/get_grouped_versions",
	tag = "versions",
	responses(
		(status = 200, body = Vec<MinorVersionGroup>),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
pub async fn get_grouped_versions(
	State(state): State<AppState>,
) -> Result<Json<Vec<MinorVersionGroup>>> {
	let mut conn = state.db_read.get().await?;
	let versions = Version::get_all_including_drafts(&mut conn).await?;

	// One batched query to compute `ready` for every version returned.
	let version_ids: Vec<Uuid> = versions.iter().map(|v| v.id).collect();
	let affected = VersionKnownIssue::affected_versions(&mut conn, &version_ids).await?;

	let mut grouped: BTreeMap<(i32, i32), Vec<Version>> = BTreeMap::new();
	for version in versions {
		grouped
			.entry((version.major, version.minor))
			.or_default()
			.push(version);
	}

	let mut result: Vec<MinorVersionGroup> = grouped
		.into_iter()
		.map(|((major, minor), mut versions)| {
			versions.sort_by(|a, b| b.patch.cmp(&a.patch));
			let count = versions.len();

			let published_versions: Vec<_> = versions
				.iter()
				.filter(|v| v.status == VersionStatus::Published)
				.collect();

			let latest_patch = published_versions.first().map(|v| v.patch).unwrap_or(0);
			// The minor is "ready" iff its latest published patch is itself
			// ready. If no patches are published, treat the minor as ready
			// — there's nothing for users to receive yet.
			let ready = published_versions
				.first()
				.map(|v| !affected.contains(&v.id))
				.unwrap_or(true);

			let first_created_at = published_versions
				.iter()
				.find(|v| v.patch == 0)
				.map(|v| v.created_at)
				.unwrap_or_else(|| {
					published_versions
						.last()
						.map(|v| v.created_at)
						.unwrap_or_else(Timestamp::now)
				});

			let last_created_at = published_versions
				.first()
				.map(|v| v.created_at)
				.unwrap_or_else(Timestamp::now);

			let version_data: Vec<VersionData> = versions
				.into_iter()
				.map(|v| VersionData {
					ready: !affected.contains(&v.id),
					major: v.major,
					minor: v.minor,
					patch: v.patch,
					status: v.status,
					created_at: v.created_at,
				})
				.collect();

			MinorVersionGroup {
				major,
				minor,
				count,
				latest_patch,
				first_created_at,
				last_created_at,
				versions: version_data,
				ready,
			}
		})
		.collect();

	result.sort_by(|a, b| b.major.cmp(&a.major).then_with(|| b.minor.cmp(&a.minor)));
	Ok(Json(result))
}

/// Identifies a version by its exact version string.
#[derive(Deserialize, ToSchema)]
pub struct VersionStringArgs {
	/// Exact version string to look up (e.g. `"1.2.3"`).
	pub version: String,
}

/// Get full details for a single version.
///
/// Returns the version's changelog, minimum browser version requirement,
/// whether it's the latest patch in its release line, other versions in
/// the same release line, its readiness, and its full known-issue history.
/// Returns 404 if the version doesn't exist.
#[utoipa::path(
	post,
	path = "/get_version_detail",
	tag = "versions",
	request_body = VersionStringArgs,
	responses(
		(status = 200, body = VersionDetail),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn get_version_detail(
	State(state): State<AppState>,
	Json(args): Json<VersionStringArgs>,
) -> Result<Json<VersionDetail>> {
	let mut conn = state.db_read.get().await?;
	let version = VersionStr::from_str(&args.version)?;
	let version_record = Version::get_by_version(&mut conn, version.clone()).await?;

	let min_chrome_version = if let Ok(head_release_date) =
		Version::get_head_release_date(&mut conn, version.clone()).await
	{
		database::chrome_releases::ChromeRelease::get_min_version_at_date(
			&mut conn,
			head_release_date,
		)
		.await
		.ok()
		.flatten()
	} else {
		None
	};

	let is_latest_in_minor = Version::is_latest_in_minor(&mut conn, version.clone())
		.await
		.unwrap_or(true);

	let related_versions = Version::get_all_in_minor(&mut conn, version.clone())
		.await
		.unwrap_or_default()
		.into_iter()
		.map(|v| RelatedVersionData {
			major: v.major,
			minor: v.minor,
			patch: v.patch,
			changelog: v.changelog,
		})
		.collect();

	// Surface every issue ever raised against this minor (resolved or
	// not), so the operator sees the full timeline of caveats for the
	// branch.
	let known_issues: Vec<KnownIssueData> =
		VersionKnownIssue::list_for_minor(&mut conn, version_record.major, version_record.minor)
			.await?
			.into_iter()
			.map(KnownIssueData::from)
			.collect();
	// `ready` is whether THIS exact patch is unaffected — older issues
	// fixed below this patch don't affect us.
	let ready = VersionKnownIssue::version_is_ready(
		&mut conn,
		version_record.major,
		version_record.minor,
		version_record.patch,
	)
	.await?;

	Ok(Json(VersionDetail {
		id: version_record.id,
		major: version_record.major,
		minor: version_record.minor,
		patch: version_record.patch,
		status: version_record.status,
		created_at: version_record.created_at,
		updated_at: version_record.updated_at,
		changelog: version_record.changelog,
		min_chrome_version,
		is_latest_in_minor,
		related_versions,
		ready,
		known_issues,
	}))
}

/// List the downloadable artifacts available for a version.
///
/// Returns one entry per artifact type/platform combination actually
/// served for this version, whether tied to the exact version or matched
/// via a version range pattern. Returns 404 if the version doesn't exist.
#[utoipa::path(
	post,
	path = "/get_version_artifacts",
	tag = "versions",
	security(("tailscale-user" = [])),
	request_body = VersionStringArgs,
	responses(
		(status = 200, body = Vec<ArtifactData>),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn get_version_artifacts(
	State(state): State<AppState>,
	// Every group's artifacts, digests and group names, which ART discloses to
	// an operator and to nobody else. The tagged-device layer above only turns
	// away a caller that both carries no identity and comes from the tailnet.
	// spec: ART#who-is-offered-a-group-scoped-artifact, ADM
	_user: TailscaleUser,
	Json(args): Json<VersionStringArgs>,
) -> Result<Json<Vec<ArtifactData>>> {
	let mut conn = state.db_read.get().await?;
	let version = VersionStr::from_str(&args.version)?;
	let version_record = Version::get_by_version(&mut conn, version).await?;
	Ok(Json(artifacts_of(&mut conn, version_record.id).await?))
}

/// Every artifact of a version as an operator sees it: the full set, including
/// what specificity passed over and every group's, because what resolution
/// hides is a fact about how a version was published.
// spec: ART#what-a-version-offers
async fn artifacts_of(
	conn: &mut database::diesel_async::AsyncPgConnection,
	version_id: Uuid,
) -> Result<Vec<ArtifactData>> {
	let artifacts_with_metadata =
		Artifact::get_for_version_all_matches_with_metadata(conn, version_id, Scope::Fleet).await?;
	let groups: Vec<Uuid> = artifacts_with_metadata
		.iter()
		.filter_map(|(artifact, ..)| artifact.group_id)
		.collect();
	let group_names = ServerGroup::names_by_ids(conn, &groups).await?;
	Ok(artifacts_with_metadata
		.into_iter()
		.map(
			|(a, is_exact, has_range_override, is_used_in_public_api)| ArtifactData {
				id: a.id,
				artifact_type: a.artifact_type,
				platform: a.platform,
				canopy_holds_bytes: a.download_url.is_none(),
				download_url: a.download_url,
				group_name: a.group_id.and_then(|g| group_names.get(&g).cloned()),
				group_id: a.group_id,
				digest: a.digest.as_deref().map(sri),
				is_exact,
				version_range_pattern: a.version_range_pattern,
				has_range_override,
				is_used_in_public_api,
			},
		)
		.collect())
}

/// Identifies a version and the publication status to set on it.
#[derive(Deserialize, ToSchema)]
pub struct UpdateStatusArgs {
	/// Exact version string to update (e.g. `"1.2.3"`).
	pub version: String,
	/// New status. Accepted values are `"draft"`, `"published"`, and
	/// `"yanked"` (case-insensitive); an unrecognized value is treated as
	/// `"draft"`.
	pub status: String,
}

/// Change a version's publication status.
///
/// Returns 400 if the change would move a published version back to draft
/// while it isn't the latest published patch in its release line — older
/// published patches can't be un-published out from under a newer one.
#[utoipa::path(
	post,
	path = "/update_version_status",
	tag = "versions",
	security(("tailscale-admin" = [])),
	request_body = UpdateStatusArgs,
	responses(
		(status = 200),
		(status = 400, body = ProblemDetailsSchema),
	),
)]
pub async fn update_version_status(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<UpdateStatusArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	let version = VersionStr::from_str(&args.version)?;
	let new_status = VersionStatus::from(args.status);

	let version_record = Version::get_by_version(&mut conn, version.clone()).await?;
	if version_record.status == VersionStatus::Published && new_status == VersionStatus::Draft {
		let is_latest = Version::is_latest_in_minor(&mut conn, version.clone()).await?;
		if !is_latest {
			return Err(AppError::BadRequest(
				"Cannot change a published version to draft unless it is the latest in its \
				 minor version"
					.into(),
			));
		}
	}

	Version::update_status(&mut conn, version, new_status).await?;
	Ok(Json(()))
}

/// Identifies a version and the changelog text to set on it.
#[derive(Deserialize, ToSchema)]
pub struct UpdateChangelogArgs {
	/// Exact version string to update (e.g. `"1.2.3"`).
	pub version: String,
	/// New changelog text for the version.
	pub changelog: String,
}

/// Replace a version's changelog text.
///
/// Overwrites the changelog of the version identified by its exact
/// version string. Updating a version that doesn't exist succeeds
/// without effect.
#[utoipa::path(
	post,
	path = "/update_version_changelog",
	tag = "versions",
	security(("tailscale-admin" = [])),
	request_body = UpdateChangelogArgs,
	responses(
		(status = 200),
	),
)]
pub async fn update_version_changelog(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<UpdateChangelogArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	let version = VersionStr::from_str(&args.version)?;
	Version::update_changelog(&mut conn, version, args.changelog).await?;
	Ok(Json(()))
}

/// Changes to apply to an existing artifact.
#[derive(Deserialize, ToSchema)]
pub struct UpdateArtifactArgs {
	/// Id of the artifact to update.
	pub artifact_id: Uuid,
	/// New artifact type.
	pub artifact_type: String,
	/// New target platform.
	pub platform: String,
	/// New download URL. Leave unset for an artifact whose bytes Canopy holds.
	pub download_url: Option<String>,
}

/// Update an existing artifact's type, platform, and download URL.
///
/// All three fields are replaced with the supplied values; the artifact's
/// version association is unchanged.
#[utoipa::path(
	post,
	path = "/update_artifact",
	tag = "versions",
	security(("tailscale-admin" = [])),
	request_body = UpdateArtifactArgs,
	responses(
		(status = 200),
	),
)]
pub async fn update_artifact(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<UpdateArtifactArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	Artifact::update(
		&mut conn,
		args.artifact_id,
		args.artifact_type,
		args.platform,
		args.download_url,
	)
	.await?;
	Ok(Json(()))
}

/// A new artifact to register against a version, at a location Canopy records.
///
/// An artifact whose bytes Canopy holds is registered through
/// `upload_artifact` instead, since the bytes are the body there.
// spec: ART#where-an-artifact-rests
#[derive(Deserialize, ToSchema)]
pub struct CreateArtifactArgs {
	/// Id of the version to attach the new artifact to.
	pub version_id: Uuid,
	/// Artifact type.
	pub artifact_type: String,
	/// Target platform.
	pub platform: String,
	/// URL the artifact is downloaded from.
	pub download_url: String,
	/// Subresource Integrity digest of the bytes at that URL, e.g.
	/// `sha256-LCTbqp…`, where one is recorded. Whoever fetches the artifact
	/// checks what it got against this.
	pub digest: Option<String>,
}

/// Create a new artifact tied to an exact version.
///
/// Registers a download of the given type and platform against the
/// version, and returns the created artifact.
#[utoipa::path(
	post,
	path = "/create_artifact",
	tag = "versions",
	security(("tailscale-admin" = [])),
	request_body = CreateArtifactArgs,
	responses(
		(status = 200, body = ArtifactData),
		(status = 400, body = ProblemDetailsSchema),
	),
)]
pub async fn create_artifact(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<CreateArtifactArgs>,
) -> Result<Json<ArtifactData>> {
	let mut conn = state.db.get().await?;

	// A digest against a location is what whoever fetches the artifact checks
	// the bytes it got against, so one that cannot be checked against is
	// refused rather than published.
	// spec: ART#digests
	let digest = parse_sri_opt(args.digest.as_deref())?;

	// Where the artifact rests, and the refusal when it names neither place or
	// both, is `Artifact::register`'s to settle.
	let artifact = Artifact::register(
		&mut conn,
		NewArtifact {
			version_id: Some(args.version_id),
			artifact_type: args.artifact_type,
			platform: args.platform,
			download_url: Some(args.download_url),
			device_id: None,
			version_range_pattern: None,
			group_id: None,
			content: None,
			content_type: None,
			digest,
			run_id: None,
		},
	)
	.await?;

	registered(&mut conn, args.version_id, artifact.id).await
}

/// What an upload names beside its bytes.
// spec: ART#where-an-artifact-rests
#[derive(Deserialize, IntoParams)]
pub struct UploadArtifactQuery {
	/// Id of the version to attach the new artifact to.
	pub version_id: Uuid,
	/// Artifact type.
	pub artifact_type: String,
	/// Target platform.
	pub platform: String,
	/// The group this artifact is for.
	pub group_id: Uuid,
	/// Subresource Integrity digest of the body, e.g. `sha256-LCTbqp…`.
	/// Canopy checks the bytes against it as they arrive and refuses the
	/// registration on a mismatch, so a corrupted upload is refused while
	/// whoever sent it is still there to send it again.
	pub digest: String,
}

/// Register an artifact whose bytes Canopy holds, for one group.
///
/// The body is the artifact itself and its `Content-Type` is what the bytes
/// are served back as. Returns the created artifact.
#[utoipa::path(
	post,
	path = "/upload_artifact",
	tag = "versions",
	security(("tailscale-admin" = [])),
	params(
		UploadArtifactQuery,
		("x-canopy-upload" = String, Header, description = "Any value. Required: it makes a browser preflight the request, so a cross-origin page cannot spend an operator's session on this endpoint."),
	),
	request_body(content = Vec<u8>, content_type = "application/octet-stream", description = "The artifact's bytes."),
	responses(
		(status = 200, body = ArtifactData),
		(status = 400, body = ProblemDetailsSchema),
	),
)]
pub async fn upload_artifact(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Query(named): Query<UploadArtifactQuery>,
	headers: axum::http::HeaderMap,
	body: Bytes,
) -> Result<Json<ArtifactData>> {
	// Every other write here carries a JSON body, which is not a content type a
	// form can send, so the browser preflights it and a cross-origin page never
	// reaches it. This one takes raw bytes, so it asks for a header of its own
	// to the same end: the operator's tailnet identity is supplied by the proxy,
	// and a page they merely visited must not be able to spend it.
	if !headers.contains_key(FETCH_HEADER) {
		return Err(AppError::BadRequest(format!(
			"an upload must carry the {FETCH_HEADER} header"
		)));
	}

	if body.len() > MAX_HELD_ARTIFACT_BYTES {
		return Err(AppError::BadRequest(format!(
			"artifact is larger than the {} MiB limit",
			MAX_HELD_ARTIFACT_BYTES / (1024 * 1024)
		)));
	}

	let claimed = parse_sri(&named.digest)?;
	// Hashing the whole artifact is tens of milliseconds with no await in it,
	// and the pool it would be holding while it ran is five connections wide.
	let digest = {
		let body = body.clone();
		tokio::task::spawn_blocking(move || digest_of(&body))
			.await
			.map_err(|err| AppError::custom(format!("digesting the artifact failed: {err}")))?
	};
	if claimed != digest {
		return Err(AppError::BadRequest(format!(
			"the bytes are {}, not the {} the registration names",
			sri(&digest),
			sri(&claimed)
		)));
	}

	// The media type is served back as a header, and the browser sends none
	// for a file it cannot type, so an absent one is the artifact's own
	// default rather than a refusal.
	let content_type = headers
		.get(axum::http::header::CONTENT_TYPE)
		.and_then(|value| value.to_str().ok())
		.map(str::to_owned)
		.filter(|media_type| media_type != "application/octet-stream");

	let mut conn = state.db.get().await?;
	let artifact = Artifact::register(
		&mut conn,
		NewArtifact {
			version_id: Some(named.version_id),
			artifact_type: named.artifact_type,
			platform: named.platform,
			download_url: None,
			device_id: None,
			version_range_pattern: None,
			group_id: Some(named.group_id),
			content: Some(Vec::from(body)),
			content_type,
			digest: Some(digest),
			run_id: None,
		},
	)
	.await?;

	registered(&mut conn, named.version_id, artifact.id).await
}

/// Read a just-registered artifact back through the listing rather than
/// describing the row a second time: whether it overrides a range and whether
/// it is the one served follow from the version's other artifacts.
async fn registered(
	conn: &mut database::diesel_async::AsyncPgConnection,
	version_id: Uuid,
	artifact_id: Uuid,
) -> Result<Json<ArtifactData>> {
	artifacts_of(conn, version_id)
		.await?
		.into_iter()
		.find(|a| a.id == artifact_id)
		.map(Json)
		.ok_or_else(|| AppError::custom("the artifact just registered is not listed"))
}

/// Identifies a single artifact by id.
#[derive(Deserialize, ToSchema)]
pub struct ArtifactIdArgs {
	/// Id of the artifact to delete.
	pub artifact_id: Uuid,
}

/// Permanently delete an artifact.
///
/// The artifact record is removed outright; the file it pointed to is not
/// touched. There is no undo.
#[utoipa::path(
	post,
	path = "/delete_artifact",
	tag = "versions",
	security(("tailscale-admin" = [])),
	request_body = ArtifactIdArgs,
	responses(
		(status = 200),
	),
)]
pub async fn delete_artifact(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<ArtifactIdArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	Artifact::delete(&mut conn, args.artifact_id).await?;
	Ok(Json(()))
}

/// Identifies a single version by id.
#[derive(Deserialize, ToSchema)]
pub struct VersionIdArgs {
	/// Id of a version whose release line to look up.
	pub version_id: Uuid,
}

/// List known issues for a version's release line.
///
/// Returns every known issue ever raised against the major.minor release
/// line the given version belongs to, resolved or not. Returns 404 if the
/// version doesn't exist.
#[utoipa::path(
	post,
	path = "/list_known_issues",
	tag = "versions",
	security(("tailscale-user" = [])),
	request_body = VersionIdArgs,
	responses(
		(status = 200, body = Vec<KnownIssueData>),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn list_known_issues(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<VersionIdArgs>,
) -> Result<Json<Vec<KnownIssueData>>> {
	let mut conn = state.db_read.get().await?;
	let v = Version::get_by_id(&mut conn, args.version_id).await?;
	let rows = VersionKnownIssue::list_for_minor(&mut conn, v.major, v.minor).await?;
	Ok(Json(rows.into_iter().map(KnownIssueData::from).collect()))
}

/// A known issue to record against a version.
#[derive(Deserialize, ToSchema)]
pub struct AddKnownIssueArgs {
	/// Id of a version in the release line the issue affects. The issue is
	/// recorded as affecting that version's exact patch onward, within its
	/// release line.
	pub version_id: Uuid,
	/// Description of the issue. Must not be empty or whitespace-only.
	pub description: String,
}

/// Record a new known issue affecting a version's release line, starting
/// from the given version's exact patch.
///
/// Returns 400 if the description is empty or whitespace-only.
#[utoipa::path(
	post,
	path = "/add_known_issue",
	tag = "versions",
	security(("tailscale-admin" = [])),
	request_body = AddKnownIssueArgs,
	responses(
		(status = 200, body = KnownIssueData),
		(status = 400, body = ProblemDetailsSchema),
	),
)]
pub async fn add_known_issue(
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<AddKnownIssueArgs>,
) -> Result<Json<KnownIssueData>> {
	let description = args.description.trim();
	if description.is_empty() {
		return Err(AppError::BadRequest("Description must not be empty".into()));
	}
	let mut conn = state.db.get().await?;
	let v = Version::get_by_id(&mut conn, args.version_id).await?;
	let row = VersionKnownIssue::add(
		&mut conn,
		(v.major, v.minor, v.patch),
		&admin.0.login,
		description,
		None,
	)
	.await?;
	Ok(Json(KnownIssueData::from(row)))
}

/// Identifies a known issue and the version that fixes it.
#[derive(Deserialize, ToSchema)]
pub struct ResolveKnownIssueArgs {
	/// Id of the known issue to resolve.
	pub known_issue_id: Uuid,
	/// Version string of the first patch that contains the fix. Must be in
	/// the same release line as the issue's earliest affected patch, and
	/// strictly above it.
	pub fix_version: String,
	/// Explanation of how or where the issue was fixed. Must not be empty
	/// or whitespace-only.
	pub resolution_message: String,
}

/// Mark a known issue as resolved as of a given fix version.
///
/// Returns 400 if the resolution message is empty or whitespace-only.
/// Returns 404 if the known issue doesn't exist, is already resolved, or
/// if `fix_version` isn't in the same release line as the issue or isn't
/// strictly above its earliest affected patch.
#[utoipa::path(
	post,
	path = "/resolve_known_issue",
	tag = "versions",
	security(("tailscale-admin" = [])),
	request_body = ResolveKnownIssueArgs,
	responses(
		(status = 200, body = KnownIssueData),
		(status = 400, body = ProblemDetailsSchema),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn resolve_known_issue(
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<ResolveKnownIssueArgs>,
) -> Result<Json<KnownIssueData>> {
	let resolution = args.resolution_message.trim();
	if resolution.is_empty() {
		return Err(AppError::BadRequest(
			"Resolution message must not be empty".into(),
		));
	}
	let fix = VersionStr::from_str(&args.fix_version)?;
	let fix = (fix.0.major as i32, fix.0.minor as i32, fix.0.patch as i32);
	let mut conn = state.db.get().await?;
	let row = VersionKnownIssue::resolve(
		&mut conn,
		args.known_issue_id,
		fix,
		&admin.0.login,
		resolution,
	)
	.await?;
	Ok(Json(KnownIssueData::from(row)))
}
