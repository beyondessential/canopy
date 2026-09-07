use axum::{
	Json,
	extract::{Path, Query, State},
};
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::device_auth::{AuthDevice, ReleaserDevice};
use commons_types::version::{VersionStatus, VersionStr};
use database::{
	Db,
	artifacts::{Artifact as ArtifactRow, NewArtifact, Scope},
	machines::Machine,
	versions::{NewVersion, Version},
};
use diesel::SelectableHelper as _;
use diesel_async::RunQueryDsl as _;
use serde::Serialize;
use uuid::Uuid;

use crate::state::AppState;

/// An artifact as it is offered to a caller.
#[derive(Debug, Clone, Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct Artifact {
	/// Unique identifier of the artifact.
	pub id: Uuid,
	/// The exact version this artifact belongs to. `null` for range
	/// artifacts, which apply to every version matching
	/// `version_range_pattern` instead.
	pub version_id: Option<Uuid>,
	/// What kind of artifact this is (e.g. an installer or package name).
	pub artifact_type: String,
	/// The platform the artifact targets (e.g. an OS or architecture name).
	pub platform: String,
	/// URL the artifact can be downloaded from. For an artifact whose bytes
	/// Canopy holds, this is Canopy's own download endpoint for it.
	pub download_url: String,
	/// The device that registered this artifact, if it was registered by a
	/// releaser device rather than created by an operator.
	pub device_id: Option<Uuid>,
	/// Semver range this artifact applies to (e.g. `^2.10.0`), for artifacts
	/// shared across a range of versions rather than pinned to one. `null`
	/// for exact-version artifacts.
	pub version_range_pattern: Option<String>,
	/// The group this artifact is for. `null` for an artifact that is for
	/// every group.
	pub group_id: Option<Uuid>,
	/// Algorithm-prefixed digest of the artifact's bytes, e.g.
	/// `sha256:2cf24dba…`, where one was recorded.
	pub digest: Option<String>,
}

impl Artifact {
	/// Present a stored row to a caller it is offered to.
	///
	/// An artifact Canopy holds has no location of its own, so it is offered
	/// Canopy's download endpoint: whoever is offered an artifact is given one
	/// URL to fetch it from, whichever of the two it turned out to be.
	// spec: ART#where-an-artifact-rests
	pub(crate) fn offered(row: ArtifactRow, base: &str, version: &str) -> Self {
		let download_url = row
			.download_url
			.clone()
			.unwrap_or_else(|| format!("{base}/versions/{version}/artifacts/{}/download", row.id));

		Self {
			id: row.id,
			version_id: row.version_id,
			artifact_type: row.artifact_type,
			platform: row.platform,
			download_url,
			device_id: row.device_id,
			version_range_pattern: row.version_range_pattern,
			group_id: row.group_id,
			digest: row.digest,
		}
	}
}

/// What the authenticated caller may see.
///
/// A caller's group is derived from its identity and never taken from the
/// request, and a caller with no identity, no machine, or no group is offered
/// the unscoped artifacts alone rather than refused.
// spec: ART#who-is-offered-a-group-scoped-artifact
pub(crate) async fn caller_scope(
	conn: &mut database::diesel_async::AsyncPgConnection,
	device: Option<AuthDevice>,
) -> Result<Scope> {
	let Some(device) = device else {
		return Ok(Scope::Unscoped);
	};

	let machine = Machine::get_by_device_id(conn, device.0.id).await?;
	Ok(Scope::for_caller(machine.and_then(|m| m.group_id)))
}

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new().routes(routes!(create))
}

/// Register a downloadable artifact for a version or version range.
///
/// Requires a device certificate with the releaser role (or admin). The
/// path identifies the version the artifact belongs to — either an exact
/// version (e.g. `2.10.5`) or a semver range pattern (e.g. `2.10.x`,
/// `^2.10.0`) — followed by the artifact's type and target platform. The
/// request body is the plain-text URL clients should download the
/// artifact from.
///
/// When an exact version is given and it doesn't exist yet, it is created
/// automatically as an unpublished draft so the artifact has a version to
/// attach to; publishing that version later (via the version-creation
/// endpoint) is a separate step. When a range pattern is given instead,
/// the artifact isn't tied to one version — it matches whichever
/// published version currently satisfies the range at lookup time.
///
/// Returns the created artifact record. Returns 400 if the version or
/// range syntax can't be parsed.
#[utoipa::path(
	post,
	path = "/{version}/{artifact_type}/{platform}",
	operation_id = "register_artifact",
	tag = "artifacts",
	security(("releaser-device" = [])),
	params(
		("version" = String, Path, description = "Exact semver (e.g. `2.10.5`) or range pattern (e.g. `2.10.x`, `^2.10.0`)."),
		("artifact_type" = String, Path),
		("platform" = String, Path),
		("group" = Option<Uuid>, Query, description = "Group the artifact is for. A releaser credential carries no authorisation for any group, so naming one here is refused."),
	),
	request_body(content = String, description = "Download URL for the artifact, as a plain-text body."),
	responses(
		(status = 200, body = Artifact),
		(status = 400, body = ProblemDetailsSchema),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
#[axum::debug_handler]
async fn create(
	device: ReleaserDevice,
	State(db): State<Db>,
	Path((version, artifact_type, platform)): Path<(String, String, String)>,
	Query(scope): Query<RegisterScope>,
	headers: axum::http::HeaderMap,
	url: String,
) -> Result<Json<Artifact>> {
	use node_semver::{Range, Version as SemverVersion};

	// A releaser registers unscoped artifacts and carries no authorisation for
	// any group, so the group-scoped path is not reachable from this endpoint
	// at all rather than being refused per group.
	// spec: ART#registration
	if scope.group.is_some() {
		return Err(AppError::AuthInsufficientPermissions {
			required: "authorisation for the named group".into(),
		});
	}

	let mut db = db.get().await?;
	let device_id = device.0.0.id;

	let (version_id, version_range_pattern) = if let Ok(semver) = SemverVersion::parse(&version) {
		let version_str = VersionStr(semver);

		// The version an artifact names may not exist yet: it is created as a
		// draft so the artifact has something to attach to, and publishing it
		// stays a separate step.
		let version_id = match Version::get_by_version(&mut db, version_str.clone()).await {
			Ok(version) => version.id,
			Err(_) => {
				let new_version = NewVersion {
					major: version_str.0.major as _,
					minor: version_str.0.minor as _,
					patch: version_str.0.patch as _,
					changelog: String::new(),
					status: VersionStatus::Draft,
					device_id: Some(device_id),
				};

				diesel::insert_into(database::schema::versions::table)
					.values(new_version)
					.returning(Version::as_select())
					.get_result::<Version>(&mut db)
					.await?
					.id
			}
		};

		(Some(version_id), None)
	} else {
		Range::parse(&version).map_err(|_| AppError::custom("Invalid version or version range"))?;

		(None, Some(version.clone()))
	};

	let row = ArtifactRow::register(
		&mut db,
		NewArtifact {
			version_id,
			platform,
			artifact_type,
			download_url: Some(url),
			device_id: Some(device_id),
			version_range_pattern,
			group_id: None,
			content: None,
			content_type: None,
			digest: None,
			run_id: None,
		},
	)
	.await?;

	let base = crate::versions::public_base_url(&headers);
	Ok(Json(Artifact::offered(row, &base, &version)))
}

/// The group a registration names, where it names one.
#[derive(Debug, serde::Deserialize)]
struct RegisterScope {
	group: Option<Uuid>,
}
