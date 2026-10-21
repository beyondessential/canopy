//! Identity tokens (IDT): a machine obtains a short-lived signed token naming
//! itself or one of its applications, for a relying party that trusts Canopy as
//! an OpenID Connect issuer.
//!
//! Refusals are checked in the fixed order IDT gives, stopping at the first, so
//! the refusal a client sees is the same however many conditions hold.
// spec: IDT

use std::time::Duration;

use axum::{Json, extract::State};
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::{
	audit::{Audit, device_actor},
	device_auth::ServerDevice,
};
use commons_types::server::app_type::ApplicationType;
use database::{
	applications::Application,
	audit::Action,
	identity::{self, ConfiguredKey, GrantSubject, IdentityKey, RelyingParty},
	machines::Machine,
	server_groups::ServerGroup,
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::state::AppState;

/// How many tokens one caller may ask for in [`RATE_WINDOW`]. Each request mints
/// a new token, so a client asks whenever it needs one; this is far above that,
/// and only holds back a client stuck in a loop.
// spec: IDT#requesting
const RATE_LIMIT: u32 = 60;
const RATE_WINDOW: Duration = Duration::from_secs(60);

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new().routes(routes!(public, audit(IdentityTokenIssued): issue))
}

/// What a machine asks for.
#[derive(Debug, Deserialize, ToSchema)]
pub struct IdentityTokenArgs {
	/// The relying party, by the audience it expects tokens to carry.
	pub audience: String,
	/// The application on the calling machine the token names. Without it, the
	/// token names the machine itself.
	#[serde(default)]
	pub application_type: Option<ApplicationType>,
}

/// The token, and what it says, so a client can log what it was given without
/// decoding it.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct IdentityTokenResponse {
	/// The signed token.
	pub token: String,
	/// When it expires.
	pub expires_at: Timestamp,
	/// The subject it names.
	pub subject: String,
}

/// A token's claims.
// spec: IDT#the-token
#[derive(Debug, Serialize)]
struct Claims<'a> {
	iss: &'a str,
	aud: &'a str,
	iat: i64,
	nbf: i64,
	exp: i64,
	jti: String,
	sub: &'a str,
	group_id: String,
	group_name: &'a str,
	rank: String,
	machine_id: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	application_id: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	application_type: Option<String>,
}

/// Obtain an identity token.
///
/// Each request mints a new token with an identifier of its own; nothing is
/// cached between requests.
#[utoipa::path(
	post,
	path = "/identity-tokens",
	tag = "identity",
	security(("server-device" = [])),
	request_body = IdentityTokenArgs,
	responses(
		(status = 200, body = IdentityTokenResponse),
		(status = 403, description = "The subject holds no grant for the relying party.", body = ProblemDetailsSchema),
		(status = 404, description = "No relying party has this audience, or no live application of this type is on the machine.", body = ProblemDetailsSchema),
		(status = 409, description = "The subject's machine is pending or belongs to no group.", body = ProblemDetailsSchema),
		(status = 422, description = "The application type is spelled as one of the subject's labels.", body = ProblemDetailsSchema),
		(status = 412, description = "Device is not bound to a live machine.", body = ProblemDetailsSchema),
		(status = 429, description = "Asking faster than Canopy accepts.", body = ProblemDetailsSchema),
		(status = 502, description = "Canopy cannot sign: no issuer or key is configured, or the key service is unavailable.", body = ProblemDetailsSchema),
	),
)]
pub async fn issue(
	State(state): State<AppState>,
	audit: Audit,
	device: ServerDevice,
	Json(args): Json<IdentityTokenArgs>,
) -> Result<Json<IdentityTokenResponse>> {
	let device = &device.0.0;

	// 1. Ahead of everything, so a client stuck in a loop is held back whatever
	// it asks for.
	if !state.rate_limiter.check(
		&format!("identity-token:{}", device.id),
		RATE_LIMIT,
		RATE_WINDOW,
	) {
		return Err(AppError::RateLimited);
	}

	let mut conn = state.db.get().await?;

	// 2.
	let machine = Machine::get_by_device_id(&mut conn, device.id)
		.await?
		.filter(|m| m.deleted_at.is_none())
		.ok_or(AppError::DeviceHasNoServer)?;

	// 3.
	let party = RelyingParty::by_audience(&mut conn, args.audience.trim())
		.await?
		.ok_or_else(|| AppError::IdentityAudienceUnknown(args.audience.clone()))?;

	// 4.
	let application = match &args.application_type {
		None => None,
		Some(wanted) => Some(application_of(&mut conn, &machine, wanted).await?),
	};

	// 5. Beside no such subject, and ahead of the grant and placement, since
	// neither an operator nor waiting can clear it.
	if let Some(wanted) = &args.application_type
		&& identity::is_subject_label(&wanted.to_string())
	{
		return Err(AppError::IdentitySubjectUnrepresentable(format!(
			"the application type {wanted} is spelled as one of the subject's labels"
		)));
	}
	audit.acting_as(device_actor(device, application.as_ref().map(|a| a.id)));
	let (subject, subject_record) = match &application {
		Some(application) => (
			GrantSubject::Application(application.id),
			database::audit::Record::new(
				database::audit::RecordKind::Application,
				application.id,
				application.display_name(),
			),
		),
		None => (
			GrantSubject::Machine(machine.id),
			database::audit::Record::new(
				database::audit::RecordKind::Machine,
				machine.id,
				machine.name.clone(),
			),
		),
	};
	let placement = identity::placement(&machine);
	let sub = placement
		.as_ref()
		.ok()
		.and_then(|p| identity::subject(p, &machine, application.as_ref()));

	// 6. A workload asking for access no operator has given it, which is kept:
	// committed ahead, since the refusal rolls back the handler's transaction,
	// and settled failed at once.
	// spec: IDT#recording
	if !identity::is_granted(&mut conn, subject, party.id).await? {
		drop(conn);
		let pending = audit
			.commit_ahead(Action::IdentityTokenIssued {
				subject: subject_record,
				relying_party: party.record(),
				sub,
				jti: None,
				kid: None,
				expires_at: None,
			})
			.await?;
		audit
			.settle_failed(
				&pending,
				Some(serde_json::json!({ "reason": "not_granted" })),
			)
			.await?;
		return Err(AppError::IdentityTokenNotGranted(format!(
			"{} holds no grant for {}",
			match subject {
				GrantSubject::Machine(_) => "this machine",
				GrantSubject::Application(_) => "this application",
			},
			party.name,
		)));
	}

	// 7. After the grant, so a subject missing both learns first of the one
	// waiting cannot clear.
	let placement = placement
		.map_err(|unplaced| AppError::IdentitySubjectNotPlaced(unplaced.describe().to_owned()))?;
	// Every other value is an identifier, a rank or a slug, so this is a fault
	// in Canopy rather than a refusal.
	let Some(sub) = sub else {
		return Err(AppError::custom(
			"this subject has a value outside the subject alphabet, so no token is issued for it",
		));
	};
	let group = ServerGroup::get_by_id(&mut conn, placement.group_id).await?;

	// 8.
	let Some(issuer) = &state.identity else {
		return Err(AppError::Upstream(
			"identity tokens are not configured on this Canopy instance".into(),
		));
	};
	let keys = issuer.keyring.keys().await;
	let observed = IdentityKey::all(&mut conn).await?;
	// Not held across the key service's round trip: the issuance is recorded in
	// the audited transaction.
	drop(conn);

	let configured: Vec<ConfiguredKey> = keys
		.iter()
		.map(|(key_ref, key)| ConfiguredKey {
			key_ref,
			kid: key.as_ref().map(|k| k.kid()),
		})
		.collect();
	let key = identity::signing(&configured, &observed, Timestamp::now())
		.map_err(|cannot| cannot.describe())
		.and_then(|i| {
			keys[i].1.clone().ok_or_else(|| {
				format!(
					"the signing key {} cannot be read from the key service",
					keys[i].0
				)
			})
		})
		.map_err(|why| {
			tracing::error!(%why, "identity-tokens: no signing key is available");
			AppError::Upstream("no signing key is available".into())
		})?;

	// Whole seconds, as `exp` carries it, so the expiry reported and recorded
	// is the token's own.
	let now = Timestamp::from_second(Timestamp::now().as_second())
		.expect("the current second is a timestamp");
	let expires_at = now + party.token_lifetime();
	let jti = Uuid::new_v4().to_string();
	let claims = Claims {
		iss: &issuer.issuer,
		aud: &party.audience,
		iat: now.as_second(),
		nbf: now.as_second(),
		exp: expires_at.as_second(),
		jti: jti.clone(),
		sub: &sub,
		group_id: placement.group_id.to_string(),
		group_name: &group.name,
		rank: placement.rank.to_string(),
		machine_id: machine.id.to_string(),
		application_id: application.as_ref().map(|a| a.id.to_string()),
		application_type: application.as_ref().map(|a| a.r#type.to_string()),
	};
	let token = oidc::encode(&key, &claims).await.map_err(|err| {
		tracing::error!(kid = key.kid(), %err, "identity-tokens: signing failed");
		AppError::Upstream("the key service could not sign".into())
	})?;

	// Written with the issuance and committed before the response leaves, so a
	// token whose entry cannot be written is never handed out.
	// spec: IDT#recording
	let mut tx = audit.begin().await?;
	if observed
		.get(key.kid())
		.is_none_or(|row| row.first_signed_at.is_none())
	{
		IdentityKey::mark_signed(&mut tx, key.kid(), &key.key_ref).await?;
	}
	tx.record(Action::IdentityTokenIssued {
		subject: subject_record,
		relying_party: party.record(),
		sub: Some(sub.clone()),
		jti: Some(jti),
		kid: Some(key.kid().to_owned()),
		expires_at: Some(expires_at),
	})
	.await?;

	Ok(Json(IdentityTokenResponse {
		token,
		expires_at,
		subject: sub,
	}))
}

/// The live application of `wanted` type on `machine`, named by type the way the
/// DNS name endpoints name one.
async fn application_of(
	conn: &mut database::diesel_async::AsyncPgConnection,
	machine: &Machine,
	wanted: &ApplicationType,
) -> Result<Application> {
	let mut matching: Vec<Application> = machine
		.applications(conn)
		.await?
		.into_iter()
		.filter(|a| a.r#type == *wanted)
		.collect();
	match matching.len() {
		1 => Ok(matching.remove(0)),
		0 => Err(AppError::IdentitySubjectNotFound(format!(
			"this machine has no live {wanted} application"
		))),
		_ => Err(AppError::IdentitySubjectNotFound(format!(
			"this machine has several {wanted} applications, so the type names none of them"
		))),
	}
}

#[cfg(test)]
mod tests {
	use std::collections::BTreeSet;

	use super::*;

	/// The discovery document advertises the claims a token carries, so the two
	/// are held to the same list.
	#[test]
	fn every_claim_is_advertised_and_every_advertised_claim_is_carried() {
		let claims = Claims {
			iss: "",
			aud: "",
			iat: 0,
			nbf: 0,
			exp: 0,
			jti: String::new(),
			sub: "",
			group_id: String::new(),
			group_name: "",
			rank: String::new(),
			machine_id: String::new(),
			application_id: Some(String::new()),
			application_type: Some(String::new()),
		};
		let carried: BTreeSet<String> = serde_json::to_value(&claims)
			.unwrap()
			.as_object()
			.unwrap()
			.keys()
			.cloned()
			.collect();
		let advertised: BTreeSet<String> = oidc::CLAIMS.iter().map(|c| c.to_string()).collect();
		assert_eq!(carried, advertised);
	}
}
