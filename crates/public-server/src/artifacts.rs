use axum::{
	Json,
	extract::{DefaultBodyLimit, Path, Query, State},
};
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::device_auth::{AuthDevice, ReleaserDevice};
use commons_types::{
	device::DeviceRole,
	version::{VersionStatus, VersionStr},
};
use database::{
	Db,
	artifacts::{
		Artifact as ArtifactRow, MAX_HELD_ARTIFACT_BYTES, NewArtifact, Scope, digest_of,
		parse_sri_opt, sri,
	},
	machines::Machine,
	restore::RestoreReplica,
	versions::{NewVersion, Version},
};
use diesel::SelectableHelper as _;
use diesel_async::RunQueryDsl as _;
use serde::Serialize;
use uuid::Uuid;

use crate::state::AppState;

/// A downloadable artifact belonging to a release version: an installer,
/// package, or other file published for a given type and platform.
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
	/// URL the artifact can be downloaded from.
	pub download_url: String,
	/// The device that registered this artifact, if it was registered by a
	/// releaser device rather than created by an operator.
	pub device_id: Option<Uuid>,
	/// Semver range this artifact applies to (e.g. `^2.10.0`), for artifacts
	/// shared across a range of versions rather than pinned to one. `null`
	/// for exact-version artifacts.
	pub version_range_pattern: Option<String>,
	/// Subresource Integrity digest of the artifact's bytes, e.g.
	/// `sha256-LCTbqp…`, where one was recorded.
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
			digest: row.digest.as_deref().map(sri),
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

/// Body budget for a schema upload. Sized above the held-bytes cap so an
/// over-limit upload is the handler's structured refusal naming the limit,
/// rather than axum's plain-text 413.
const MAX_UPLOAD_BODY_BYTES: usize = MAX_HELD_ARTIFACT_BYTES + 64 * 1024;

/// The artifact type a reporting-schema build publishes.
const REPORTING_SCHEMA_TYPE: &str = "reporting-schema";

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new().routes(routes!(create)).merge(
		OpenApiRouter::new()
			.routes(routes!(register_for_group))
			.layer(DefaultBodyLimit::max(MAX_UPLOAD_BODY_BYTES)),
	)
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
		("digest" = Option<String>, Query, description = "Subresource Integrity digest of the bytes at the URL, e.g. `sha256-LCTbqp…`. Whoever fetches the artifact checks what it got against this; an artifact registered without one is fetched unchecked."),
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
	Query(named): Query<RegisterQuery>,
	headers: axum::http::HeaderMap,
	url: String,
) -> Result<Json<Artifact>> {
	use node_semver::{Range, Version as SemverVersion};

	// A releaser registers unscoped artifacts and carries no authorisation for
	// any group, so the group-scoped path is not reachable from this endpoint
	// at all rather than being refused per group.
	// spec: ART#registration
	if named.group.is_some() {
		return Err(AppError::AuthInsufficientPermissions {
			required: "authorisation for the named group".into(),
		});
	}

	// Where the artifact rests, and the refusal for a body that is no location
	// at all, is `Artifact::register`'s to settle.
	let digest = parse_sri_opt(named.digest.as_deref())?;

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
			digest,
			run_id: None,
		},
	)
	.await?;

	let base = crate::versions::public_base_url(&headers);
	Ok(Json(Artifact::offered(row, &base, &version)))
}

/// Register a reporting schema for one group, carrying its bytes.
///
/// Requires a device certificate whose restore declaration for the named group
/// advertises that it builds reporting schemas. The bytes travel on this
/// connection and Canopy holds them, so the builder is issued no credential to
/// any store. The path names the group the artifact is for, the exact version
/// it was built against, and the artifact's type and target platform.
///
/// The version must be one Canopy already holds: a build is dispatched for a
/// group and version Canopy knows about, so a version that does not exist is
/// refused rather than drafted. A range pattern is refused for the same reason:
/// a schema follows the migrations one exact version applies.
///
/// Returns the created artifact record.
#[utoipa::path(
	post,
	path = "/groups/{group}/{version}/{artifact_type}/{platform}",
	operation_id = "register_group_artifact",
	tag = "artifacts",
	security(("backup-restore-device" = [])),
	params(
		("group" = Uuid, Path, description = "Group the artifact is for."),
		("version" = String, Path, description = "Exact semver (e.g. `2.10.5`) the schema was built against."),
		("artifact_type" = String, Path, description = "Must be `reporting-schema`: the authorisation is defined with that artifact."),
		("platform" = String, Path),
		("run" = Option<Uuid>, Query, description = "The run that produced the artifact, where one produced it."),
	),
	request_body(content = Vec<u8>, content_type = "application/octet-stream", description = "The artifact's bytes, which Canopy holds and records the digest of."),
	responses(
		(status = 200, body = Artifact),
		(status = 400, body = ProblemDetailsSchema),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
#[axum::debug_handler]
async fn register_for_group(
	device: AuthDevice,
	State(db): State<Db>,
	Path((group, version, artifact_type, platform)): Path<(Uuid, String, String, String)>,
	Query(named): Query<GroupRegisterQuery>,
	headers: axum::http::HeaderMap,
	body: axum::body::Bytes,
) -> Result<Json<Artifact>> {
	use node_semver::Version as SemverVersion;

	let mut db = db.get().await?;
	let device_id = device.0.id;

	// What a schema builder is authorised for is the artifact its declaration
	// names. Any other type registered under it would displace the releaser's
	// own for every machine in the group, and those machines fetch and run what
	// they are offered.
	// spec: ART#registration
	if artifact_type != REPORTING_SCHEMA_TYPE {
		return Err(AppError::AuthInsufficientPermissions {
			required: format!("a group-scoped artifact to be a {REPORTING_SCHEMA_TYPE}"),
		});
	}

	let authorised = device.0.role == DeviceRole::Admin
		|| RestoreReplica::authorizes_schema_artifacts(&mut db, device_id, group).await?;
	if !authorised {
		// Refused the same way whether the group exists or not, so the endpoint
		// is not a directory of which groups have a builder.
		return Err(AppError::AuthInsufficientPermissions {
			required: "an enabled declaration building this group's artifacts".into(),
		});
	}

	if body.len() > MAX_HELD_ARTIFACT_BYTES {
		return Err(AppError::BadRequest(format!(
			"artifact is larger than the {MAX_HELD_ARTIFACT_BYTES} byte limit"
		)));
	}
	if body.is_empty() {
		return Err(AppError::BadRequest(
			"a group-scoped artifact carries its bytes".into(),
		));
	}

	// Provenance is what an operator reads to answer what produced the bytes,
	// so a run already recorded for somebody else is not one this registration
	// may name.
	if let Some(run) = named.run
		&& RestoreReplica::run_claimed_elsewhere(&mut db, run, device_id, group).await?
	{
		return Err(AppError::BadRequest(
			"the named run belongs to another consumer or group".into(),
		));
	}

	// A schema follows the migrations one exact version applies, and Canopy
	// resolves a range artifact for every version it covers.
	// spec: RPT#the-build-contract
	let semver = SemverVersion::parse(&version)
		.map_err(|_| AppError::BadRequest("a reporting schema names an exact version".into()))?;

	// A build is dispatched for a pair whose version Canopy already holds, so
	// this names one rather than drafting a release nobody has cut.
	// spec: RPT#pairs
	let version_row = match Version::get_by_version(&mut db, VersionStr(semver)).await {
		Ok(version) => version,
		Err(AppError::DatabaseQuery(diesel::result::Error::NotFound)) => {
			return Err(AppError::BadRequest(format!(
				"no version {version} to register a group-scoped artifact against"
			)));
		}
		Err(error) => return Err(error),
	};

	let content_type = headers
		.get(axum::http::header::CONTENT_TYPE)
		.and_then(|v| v.to_str().ok())
		.map(str::to_owned);

	// Canopy holds these bytes, so it records the digest of what it actually
	// took in rather than one the registration claims for them.
	// spec: ART#digests
	let digest = digest_of(&body);

	let row = ArtifactRow::register(
		&mut db,
		NewArtifact {
			version_id: Some(version_row.id),
			platform,
			artifact_type,
			download_url: None,
			device_id: Some(device_id),
			version_range_pattern: None,
			group_id: Some(group),
			content: Some(Vec::from(body)),
			content_type,
			digest: Some(digest),
			run_id: named.run,
		},
	)
	.await?;

	let base = crate::versions::public_base_url(&headers);
	Ok(Json(Artifact::offered(row, &base, &version)))
}

/// What a group-scoped registration names beside the path.
#[derive(Debug, serde::Deserialize)]
struct GroupRegisterQuery {
	/// The run that produced the artifact, where one produced it.
	run: Option<Uuid>,
}

/// What a registration names beside the path.
#[derive(Debug, serde::Deserialize)]
struct RegisterQuery {
	/// The group the artifact is for, where it names one.
	group: Option<Uuid>,
	/// The Subresource Integrity digest whoever registers it records, where
	/// they record one. An unscoped artifact is fetched from its location by
	/// the caller rather than by Canopy, so this is what that caller checks
	/// against.
	// spec: ART#digests
	digest: Option<String>,
}
