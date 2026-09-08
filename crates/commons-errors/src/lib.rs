use std::{env::VarError, str::FromStr as _};

use axum::response::{IntoResponse, Response};
use diesel_async::pooled_connection::PoolError;
use http::{StatusCode, Uri};
use problem_details::ProblemDetails;
use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, AppError>;

/// Standard error response body, returned for every non-2xx response.
///
/// This follows the RFC 7807 "Problem Details" shape: a stable machine-readable
/// `type`, a short `title`, the repeated HTTP `status` code, and an optional
/// `detail` string with specifics of this particular occurrence.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ProblemDetailsSchema {
	/// A URI reference identifying the problem type. Stable across
	/// occurrences of the same error, so callers can match on it.
	#[serde(rename = "type")]
	#[schema(format = "uri", example = "/errors/resource-not-found")]
	pub type_: String,
	/// Short, human-readable summary of the problem type. Does not vary
	/// between occurrences of the same `type`.
	pub title: String,
	/// HTTP status code of the response, repeated here for convenience.
	#[schema(example = 404)]
	pub status: u16,
	/// Human-readable explanation specific to this occurrence of the
	/// problem, if any extra detail is available.
	pub detail: Option<String>,
}

#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum AppError {
	#[error("{0}")]
	Custom(String),

	#[error("{0}")]
	Problem(Box<ProblemDetails>),

	#[error("not implemented")]
	NotImplemented,

	#[error("environment: {0}")]
	Environment(#[from] VarError),

	#[error("header: {0}")]
	Header(String),

	#[error("version parse error: {0}")]
	VersionParse(Box<node_semver::SemverError>),

	#[error("database: {0}")]
	DatabasePool(#[from] mobc::Error<PoolError>),

	#[error("database: {0}")]
	DatabaseQuery(#[from] diesel::result::Error),

	#[error("render: {0}")]
	Tera(#[from] tera::Error),

	#[error("io: {0}")]
	Io(String),

	#[error("no versions match given range")]
	NoMatchingVersions,

	#[error("no such artifact for this version")]
	ArtifactNotFound,

	#[error("artifact does not match its digest")]
	ArtifactDigestMismatch,

	#[error("version range is not usable")]
	UnusableRange,

	#[error("timesync: {0}")]
	Timesync(#[from] timesimp::ParseError),

	#[error("missing authentication header: {0}")]
	AuthMissingHeader(&'static str),

	#[error("missing authentication certificate")]
	AuthMissingCertificate,

	#[error("invalid certificate format")]
	AuthInvalidCertificate(String),

	#[error("certificate not found or inactive")]
	AuthCertificateNotFound,

	#[error("insufficient permissions: {required} role required")]
	AuthInsufficientPermissions { required: String },

	#[error("authentication failed: {reason}")]
	AuthFailed { reason: String },

	#[error("device is not registered against any server")]
	DeviceHasNoServer,

	#[error("tailnet directory is unavailable")]
	AuthTailnetDirectoryUnavailable,

	#[error("tailnet node lacks the required tag")]
	AuthTailnetNodeNotPermitted,

	#[error("tagged-device callers are only allowed on /public/... routes")]
	TaggedDeviceNotAllowed,

	#[error("source '{0}' is denied ingestion")]
	IngestDenied(String),

	#[error("another device already claims this tailscale node id")]
	DeviceTailscaleNodeAlreadyClaimed,

	#[error("cannot merge devices: conflict on tailscale identity or server attachment")]
	DeviceMergeConflict,

	#[error("no tailnet identity could be resolved for this request")]
	AuthTailnetIdentityMissing,

	/// Deliberately opaque refusal for bearer-token-gated endpoints: a missing,
	/// malformed, unknown, revoked, and expired token all collapse to this so
	/// the response is not a token-lifecycle oracle. Maps to 401.
	#[error("missing or invalid access token")]
	AuthTokenNotValid,

	/// User-supplied input was syntactically or semantically invalid.
	/// Maps to 400 so callers don't have to chase a generic 500.
	#[error("invalid request: {0}")]
	BadRequest(String),

	/// A specific resource was looked up and not found, where the lookup
	/// doesn't go through Diesel's own `NotFound` — an `Option` returned by a
	/// directory, a cache, or a helper that already mapped its error away.
	/// Maps to 404, and reuses the `resource-not-found` problem type so a
	/// caller matches one slug for "not found" however the miss was produced.
	#[error("not found: {0}")]
	NotFound(String),

	/// Resource conflict — e.g. attempting to import a ticket whose
	/// canonical URL already belongs to a different server id. Maps
	/// to 409.
	#[error("conflict: {0}")]
	Conflict(String),

	/// A certificate was requested for a key that was revoked as compromised, so
	/// Canopy will never certify it again. Its own problem type rather than a
	/// generic conflict because the caller can act on it without a human: the
	/// remedy is always to generate a fresh key and ask again, and an agent
	/// should not have to read English prose to work that out. Maps to 409.
	// spec: CRT#revocation
	#[error("the key in this request was revoked as compromised: {0}")]
	CertificateKeyCompromised(String),

	/// The server's name management is paused, so Canopy is making no new changes
	/// on its behalf. Its own problem type so an agent can tell *not now* apart
	/// from *not you* — a withheld grant is permanent until an operator grants it,
	/// where a pause is expected to lift — and back off instead of hammering.
	/// Maps to 409.
	// spec: CRT#pausing-a-server
	#[error("name management is paused for this server: {0}")]
	NameManagementPaused(String),

	/// The name asked about is not within any domain the caller's group controls.
	/// Reported the same whether the name is unclaimed or held by another group,
	/// so the endpoint is not a directory of other groups' names. Its own
	/// problem type so an agent can tell it from a withheld grant. Maps to 403.
	// spec: CRT#identity-and-authorisation
	#[error("not entitled to this name: {0}")]
	NameNotEntitled(String),

	/// Deliberately opaque failure for the public enrollment endpoints. Every
	/// pre-completion reason (unknown/archived server, invalid/expired/consumed
	/// token, bad/expired/used challenge nonce, bad signature) collapses to this
	/// so the endpoint isn't an existence/lifecycle oracle. The specific reason
	/// is logged server-side, never returned. Maps to 403.
	#[error("enrollment failed")]
	EnrollmentFailed,

	/// Too many requests from a caller (e.g. the enrollment endpoints'
	/// per-IP / per-server rate limit). Maps to 429.
	#[error("rate limit exceeded")]
	RateLimited,

	/// A dependency Canopy proxies to failed or is not configured — e.g. the
	/// cross-account STS `AssumeRole` for backup-credentials issuance, or
	/// reading a repo-password k8s Secret for `/backup-target`. Maps to 502.
	/// The message is a generic, caller-safe summary; the underlying detail
	/// (which can name roles/buckets/secrets) is logged server-side only.
	#[error("upstream dependency failed: {0}")]
	Upstream(String),

	/// A passphrase rotation is in flight for the group, so the repository
	/// password is about to change and must not be handed out. Maps to 503:
	/// the caller should retry shortly, not treat this as a failure. Rotation
	/// is one short operation, so the wait is seconds to low minutes.
	#[error("a backup passphrase rotation is in progress for this group; retry shortly")]
	BackupRotationInProgress,
}

impl AppError {
	pub fn custom(err: impl ToString) -> Self {
		Self::Custom(err.to_string())
	}
}

impl From<std::io::Error> for AppError {
	fn from(err: std::io::Error) -> Self {
		Self::Io(err.to_string())
	}
}

impl From<ProblemDetails> for AppError {
	fn from(err: ProblemDetails) -> Self {
		Self::Problem(Box::new(err))
	}
}

impl From<node_semver::SemverError> for AppError {
	fn from(err: node_semver::SemverError) -> Self {
		Self::VersionParse(Box::new(err))
	}
}

impl IntoResponse for AppError {
	fn into_response(self) -> Response {
		let status = self.to_http_status();
		let problem = self.to_problem_details();
		let mut res = problem.into_response();
		*res.status_mut() = status;
		res
	}
}

impl Clone for AppError {
	fn clone(&self) -> Self {
		Self::Problem(Box::new(self.to_problem_details()))
	}
}

impl AppError {
	/// The status each variant answers with.
	///
	/// Deliberately exhaustive — no `_` arm. A wildcard here is how
	/// `VersionParse` and `Header`, both purely client-input errors, spent a
	/// long time answering 500: adding a variant and forgetting its status
	/// was silent. Now it doesn't compile, so the status is a decision
	/// somebody makes rather than one that defaults.
	///
	/// Note the arms are ordered, so a variant listed twice silently takes
	/// the first match. Keep each one in exactly one arm.
	fn to_http_status(&self) -> StatusCode {
		match self {
			Self::NotImplemented => StatusCode::NOT_IMPLEMENTED,
			Self::NoMatchingVersions => StatusCode::NOT_FOUND,
			// An artifact a caller is not offered is missing in exactly the
			// way one that never existed is, so which groups hold an artifact
			// is not enumerable through this endpoint.
			// spec: ART#who-is-offered-a-group-scoped-artifact
			Self::ArtifactNotFound => StatusCode::NOT_FOUND,
			Self::ArtifactDigestMismatch => StatusCode::INTERNAL_SERVER_ERROR,
			Self::UnusableRange => StatusCode::BAD_REQUEST,
			// Both arise purely from what a client sent: a version segment in
			// a URL path, or the `X-Version` header. Nothing on the server is
			// wrong, and retrying the same request can never succeed.
			Self::VersionParse(_) => StatusCode::BAD_REQUEST,
			Self::Header(_) => StatusCode::BAD_REQUEST,
			Self::DatabaseQuery(diesel::result::Error::NotFound) => StatusCode::NOT_FOUND,
			Self::AuthMissingHeader(_) => StatusCode::UNAUTHORIZED,
			Self::AuthMissingCertificate => StatusCode::UNAUTHORIZED,
			Self::AuthInvalidCertificate(_) => StatusCode::BAD_REQUEST,
			Self::AuthCertificateNotFound => StatusCode::UNAUTHORIZED,
			Self::AuthInsufficientPermissions { .. } => StatusCode::FORBIDDEN,
			Self::AuthFailed { .. } => StatusCode::UNAUTHORIZED,
			Self::DeviceHasNoServer => StatusCode::PRECONDITION_FAILED,
			Self::AuthTailnetDirectoryUnavailable => StatusCode::SERVICE_UNAVAILABLE,
			Self::AuthTailnetNodeNotPermitted => StatusCode::FORBIDDEN,
			Self::TaggedDeviceNotAllowed => StatusCode::FORBIDDEN,
			Self::IngestDenied(_) => StatusCode::FORBIDDEN,
			Self::DeviceTailscaleNodeAlreadyClaimed => StatusCode::CONFLICT,
			Self::DeviceMergeConflict => StatusCode::CONFLICT,
			Self::AuthTailnetIdentityMissing => StatusCode::UNAUTHORIZED,
			Self::AuthTokenNotValid => StatusCode::UNAUTHORIZED,
			Self::BadRequest(_) => StatusCode::BAD_REQUEST,
			Self::NotFound(_) => StatusCode::NOT_FOUND,
			Self::Conflict(_) => StatusCode::CONFLICT,
			Self::CertificateKeyCompromised(_) => StatusCode::CONFLICT,
			Self::NameManagementPaused(_) => StatusCode::CONFLICT,
			Self::NameNotEntitled(_) => StatusCode::FORBIDDEN,
			Self::EnrollmentFailed => StatusCode::FORBIDDEN,
			Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
			Self::Upstream(_) => StatusCode::BAD_GATEWAY,
			Self::BackupRotationInProgress => StatusCode::SERVICE_UNAVAILABLE,

			// Everything below is a server-side fault or a non-HTTP context,
			// and answers 500. Listed rather than wildcarded so a new variant
			// has to be placed deliberately.
			Self::Custom(_)
			| Self::Problem(_)
			| Self::Environment(_)
			| Self::DatabasePool(_)
			| Self::DatabaseQuery(_)
			| Self::Tera(_)
			| Self::Io(_)
			| Self::Timesync(_) => StatusCode::INTERNAL_SERVER_ERROR,
		}
	}

	fn to_problem_details(&self) -> ProblemDetails {
		if let Self::Problem(problem) = self {
			return *problem.clone();
		}

		let status = self.to_http_status();
		ProblemDetails::new()
			.with_status(status)
			.with_title(self.to_string())
			.with_detail(format!("{self:?}"))
			.with_type(
				Uri::from_str(&format!(
					"/errors/{slug}",
					slug = match self {
						Self::Custom(_) => "other",
						Self::NotImplemented => "not-implemented",
						Self::Environment(_) => "environment",
						Self::Header(_) => "header",
						Self::VersionParse(_) => "version-parse",
						Self::DatabasePool(_) => "database",
						Self::DatabaseQuery(diesel::result::Error::NotFound)
						| Self::NotFound(_) => "resource-not-found",
						Self::DatabaseQuery(_) => "database",
						Self::Tera(_) => "render",
						Self::Io(_) => "io",
						Self::NoMatchingVersions => "no-matching-versions",
						Self::ArtifactNotFound => "artifact-not-found",
						Self::ArtifactDigestMismatch => "artifact-digest-mismatch",
						Self::UnusableRange => "unusable-range",
						Self::Timesync(_) => "timesync",
						Self::AuthMissingHeader(_) => "auth-missing-header",
						Self::AuthMissingCertificate => "auth-missing-certificate",
						Self::AuthInvalidCertificate(_) => "auth-invalid-certificate",
						Self::AuthCertificateNotFound => "auth-certificate-not-found",
						Self::AuthInsufficientPermissions { .. } => "auth-insufficient-permissions",
						Self::AuthFailed { .. } => "auth-failed",
						Self::DeviceHasNoServer => "device-has-no-server",
						Self::AuthTailnetDirectoryUnavailable =>
							"auth-tailnet-directory-unavailable",
						Self::AuthTailnetNodeNotPermitted => "auth-tailnet-node-not-permitted",
						Self::TaggedDeviceNotAllowed => "tagged-device-not-allowed",
						Self::IngestDenied(_) => "ingest-denied",
						Self::DeviceTailscaleNodeAlreadyClaimed =>
							"device-tailscale-node-already-claimed",
						Self::DeviceMergeConflict => "device-merge-conflict",
						Self::AuthTailnetIdentityMissing => "auth-tailnet-identity-missing",
						Self::AuthTokenNotValid => "auth-token-not-valid",
						Self::BadRequest(_) => "bad-request",
						Self::Conflict(_) => "conflict",
						Self::CertificateKeyCompromised(_) => "certificate-key-compromised",
						Self::NameManagementPaused(_) => "name-management-paused",
						Self::NameNotEntitled(_) => "name-not-entitled",
						Self::EnrollmentFailed => "enrollment-failed",
						Self::RateLimited => "rate-limited",
						Self::Upstream(_) => "upstream",
						Self::BackupRotationInProgress => "backup-rotation-in-progress",
						Self::Problem(_) => unreachable!(),
					}
				))
				.unwrap(),
			)
	}
}

impl Serialize for AppError {
	fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
	where
		S: serde::Serializer,
	{
		self.to_problem_details().serialize(serializer)
	}
}

impl<'de> Deserialize<'de> for AppError {
	fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
	where
		D: serde::Deserializer<'de>,
	{
		let value = ProblemDetails::deserialize(deserializer)?;
		Ok(AppError::Problem(Box::new(value)))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// GitHub's own heading anchors: lowercased, punctuation dropped, spaces
	/// hyphenated.
	fn anchor(heading: &str) -> String {
		heading
			.trim()
			.to_lowercase()
			.chars()
			.filter(|c| c.is_ascii_alphanumeric() || *c == ' ' || *c == '-')
			.map(|c| if c == ' ' { '-' } else { c })
			.collect()
	}

	/// `/errors/{slug}` redirects into ERRORS.md by anchor, so a slug with no
	/// heading drops the reader at the top of the file with no way to tell
	/// which error was theirs.
	#[test]
	fn every_slug_has_a_heading_to_land_on() {
		let arms = include_str!("lib.rs")
			.split_once("slug = match self {")
			.expect("the slug match")
			.1
			.split_once("unreachable!()")
			.expect("the end of it")
			.0;
		// Every string literal between those two points is a slug.
		let slugs: Vec<&str> = arms.split('"').skip(1).step_by(2).collect();
		assert!(slugs.len() > 30, "the match was not read: {slugs:?}");

		let anchors: Vec<String> = include_str!("../../../ERRORS.md")
			.lines()
			.filter_map(|line| line.strip_prefix("## "))
			.map(anchor)
			.collect();

		for slug in slugs {
			assert!(
				anchors.iter().any(|a| a == slug),
				"/errors/{slug} lands on an anchor ERRORS.md does not have"
			);
		}
	}

	/// Input an operator or a client controls answers as their mistake, not as
	/// a fault, and carries the slug its documentation is written under.
	#[test]
	fn a_client_mistake_is_not_a_fault() {
		for (error, status, slug) in [
			(
				AppError::BadRequest("no".into()),
				StatusCode::BAD_REQUEST,
				"/errors/bad-request",
			),
			(
				AppError::Conflict("taken".into()),
				StatusCode::CONFLICT,
				"/errors/conflict",
			),
		] {
			let problem = error.to_problem_details();
			assert_eq!(problem.status, Some(status));
			assert_eq!(
				problem.r#type.as_ref().map(ToString::to_string).as_deref(),
				Some(slug)
			);
		}
	}
}
