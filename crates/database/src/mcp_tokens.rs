//! Bearer tokens for the public (internet-facing) MCP mount.
//!
//! Spec: `.workhorse/specs/private-server/mcp.md` (id `MCP`), "Access tokens".
//!
//! The plaintext token is handed to the operator once at minting; we persist
//! only its SHA-256 hash. A token is usable while `revoked_at IS NULL AND
//! expires_at > now()`. Lifetime is fixed at one year from minting and is not
//! a mint-time choice.

use base64::Engine;
use commons_errors::{AppError, Result};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::{SignedDuration, Timestamp};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Recognizable prefix on the plaintext so a leaked token is identifiable in
/// secret scanning, and so operators can tell what a stray credential is for.
pub const TOKEN_PREFIX: &str = "canopy_mcp_";

/// What a token may do beyond reading, chosen when it is minted.
pub const GRANT_TRIAGE: &str = "triage";

/// The grants a token can carry.
pub const GRANTS: &[&str] = &[GRANT_TRIAGE];

/// Fixed token lifetime: one year from minting.
pub const TOKEN_TTL: SignedDuration = SignedDuration::from_hours(365 * 24);

/// How far ahead of expiry the fleet-wide rotation alert raises.
pub const EXPIRY_ALERT_LEAD: SignedDuration = SignedDuration::from_hours(15 * 24);

#[derive(Clone, Debug, Queryable, Selectable)]
#[diesel(table_name = crate::schema::mcp_tokens)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct McpToken {
	pub id: Uuid,
	pub name: String,
	pub token_hash: Vec<u8>,
	pub created_by: String,
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	pub expires_at: Timestamp,
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp)]
	pub revoked_at: Option<Timestamp>,
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp)]
	pub last_used_at: Option<Timestamp>,
	/// What the token may do beyond reading, fixed at minting.
	#[diesel(deserialize_as = Grants)]
	pub grants: Vec<String>,
}

/// A `TEXT[]` column read as the strings it holds. Diesel types an array's
/// elements as nullable, and a grant is never null.
pub struct Grants(Vec<Option<String>>);

impl
	diesel::deserialize::Queryable<
		diesel::sql_types::Array<diesel::sql_types::Nullable<diesel::sql_types::Text>>,
		diesel::pg::Pg,
	> for Grants
{
	type Row = Vec<Option<String>>;

	fn build(row: Self::Row) -> diesel::deserialize::Result<Self> {
		Ok(Self(row))
	}
}

impl From<Grants> for Vec<String> {
	fn from(grants: Grants) -> Self {
		grants.0.into_iter().flatten().collect()
	}
}

/// SHA-256 of the token string. Unsalted is correct here: the token is 256 bits
/// of CSPRNG output, so there is no dictionary/brute-force risk — do not
/// "upgrade" this to HMAC/argon. The whole-digest equality lives in the SQL
/// `WHERE`, never an in-memory plaintext compare.
fn hash_token(plaintext: &str) -> Vec<u8> {
	Sha256::digest(plaintext.as_bytes()).to_vec()
}

impl McpToken {
	/// Whether the token may record triage decisions.
	pub fn may_triage(&self) -> bool {
		self.grants.iter().any(|grant| grant == GRANT_TRIAGE)
	}

	/// Mint a fresh token, returning the row and the plaintext (which the
	/// caller must show once and never persist or log). Expiry is always
	/// [`TOKEN_TTL`] from now; there is deliberately no way to ask for longer.
	pub async fn mint(
		db: &mut AsyncPgConnection,
		name: &str,
		created_by: &str,
		grants: &[String],
	) -> Result<(Self, String)> {
		use crate::schema::mcp_tokens::dsl;

		let mut raw = [0u8; 32];
		getrandom::fill(&mut raw).map_err(|e| AppError::custom(format!("CSPRNG failure: {e}")))?;
		let plaintext = format!(
			"{TOKEN_PREFIX}{}",
			base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)
		);
		let token_hash = hash_token(&plaintext);
		let expires_at = Timestamp::now()
			.checked_add(TOKEN_TTL)
			.map_err(|e| AppError::custom(format!("bad token ttl: {e}")))?;

		let token = diesel::insert_into(dsl::mcp_tokens)
			.values((
				dsl::name.eq(name),
				dsl::token_hash.eq(&token_hash),
				dsl::created_by.eq(created_by),
				dsl::expires_at.eq(jiff_diesel::Timestamp::from(expires_at)),
				dsl::grants.eq(grants),
			))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)?;

		Ok((token, plaintext))
	}

	/// Look up a usable (un-revoked, un-expired) token by its plaintext.
	/// `None` for unknown, revoked, and expired alike — the caller must not
	/// distinguish those to the requester.
	pub async fn find_active(db: &mut AsyncPgConnection, plaintext: &str) -> Result<Option<Self>> {
		use crate::schema::mcp_tokens::dsl;

		dsl::mcp_tokens
			.select(Self::as_select())
			.filter(dsl::token_hash.eq(hash_token(plaintext)))
			.filter(dsl::revoked_at.is_null())
			.filter(dsl::expires_at.gt(diesel::dsl::now))
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// Record use of a token. Throttled: skips the write when `last_used_at`
	/// is under a minute old, so a busy token costs one UPDATE per minute,
	/// not one per request.
	pub async fn touch_last_used(db: &mut AsyncPgConnection, id: Uuid) -> Result<()> {
		use crate::schema::mcp_tokens::dsl;

		let cutoff = Timestamp::now()
			.checked_sub(SignedDuration::from_secs(60))
			.map_err(|e| AppError::custom(format!("bad throttle window: {e}")))?;
		diesel::update(
			dsl::mcp_tokens.filter(dsl::id.eq(id)).filter(
				dsl::last_used_at
					.is_null()
					.or(dsl::last_used_at.lt(jiff_diesel::Timestamp::from(cutoff))),
			),
		)
		.set(dsl::last_used_at.eq(jiff_diesel::NullableTimestamp::from(Some(Timestamp::now()))))
		.execute(db)
		.await
		.map_err(AppError::from)?;
		Ok(())
	}

	/// All tokens, newest first, revoked ones included (the UI shows history).
	pub async fn list(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::mcp_tokens::dsl;

		dsl::mcp_tokens
			.select(Self::as_select())
			.order(dsl::created_at.desc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Revoke a token, effective immediately; idempotent on an already-revoked
	/// token. Errors (404) on an unknown id.
	pub async fn revoke(db: &mut AsyncPgConnection, id: Uuid) -> Result<()> {
		use crate::schema::mcp_tokens::dsl;

		let affected = diesel::update(
			dsl::mcp_tokens
				.filter(dsl::id.eq(id))
				.filter(dsl::revoked_at.is_null()),
		)
		.set(dsl::revoked_at.eq(jiff_diesel::NullableTimestamp::from(Some(Timestamp::now()))))
		.execute(db)
		.await
		.map_err(AppError::from)?;

		if affected == 0 {
			// Distinguish "unknown" (404) from "already revoked" (fine).
			let exists: i64 = dsl::mcp_tokens
				.filter(dsl::id.eq(id))
				.count()
				.get_result(db)
				.await
				.map_err(AppError::from)?;
			if exists == 0 {
				return Err(diesel::result::Error::NotFound.into());
			}
		}
		Ok(())
	}

	/// Un-revoked tokens within [`EXPIRY_ALERT_LEAD`] of expiry (already-expired
	/// ones included, so a lapsed-but-unrotated token keeps alerting).
	pub async fn expiring_soon(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::mcp_tokens::dsl;

		let horizon = Timestamp::now()
			.checked_add(EXPIRY_ALERT_LEAD)
			.map_err(|e| AppError::custom(format!("bad alert lead: {e}")))?;
		dsl::mcp_tokens
			.select(Self::as_select())
			.filter(dsl::revoked_at.is_null())
			.filter(dsl::expires_at.lt(jiff_diesel::Timestamp::from(horizon)))
			.order(dsl::expires_at.asc())
			.load(db)
			.await
			.map_err(AppError::from)
	}
}

/// The `(CANOPY_SOURCE, ref)` alert key for token rotation. Like the backup
/// refs, this is a contract: silences and Slack messages reference it.
pub const EXPIRY_REF: &str = "mcp-token-expiry";

pub const EXPIRY_DOC: &str = "## Description

One or more MCP access tokens (bearer tokens agents use against the public /mcp endpoint) are inside the rotation lead window of their fixed one-year expiry.

## Results

- **fail** — an un-revoked token expires soon (or has expired); recovers once every such token is rotated or revoked.

## Solve

Mint a replacement token in Settings, update the agent using the old one, then revoke it. The message lists each expiring token by name and minter.";

/// Rotation alert, run from the monitor loop: a [self-alert](crate::self_alerts)
/// — one coalescing record, one notification per raise/recovery, never one per
/// group. Active while any un-revoked token is inside [`EXPIRY_ALERT_LEAD`] of
/// its expiry; recovers when none are. Idle sweeps write nothing.
pub async fn sweep_token_expiry(db: &mut AsyncPgConnection) -> Result<usize> {
	use commons_types::status::CheckResult;

	let expiring = McpToken::expiring_soon(db).await?;

	if expiring.is_empty() {
		return Ok(crate::self_alerts::recover(
			db,
			EXPIRY_REF,
			"all mcp access tokens rotated or revoked",
		)
		.await?
		.map(|_| 1)
		.unwrap_or(0));
	}

	let now = Timestamp::now();
	let message = expiring
		.iter()
		.map(|t| {
			let days = (t.expires_at.duration_since(now).as_secs_f64() / 86_400.0).ceil() as i64;
			let when = t.expires_at.strftime("%Y-%m-%d");
			let relative = if days > 0 {
				format!("in {days} day{}", if days == 1 { "" } else { "s" })
			} else {
				format!("{} days ago", -days)
			};
			format!(
				"MCP access token \"{}\" (minted by {}) expires {when} ({relative}); \
				 mint a replacement in Settings and update the agent using it.",
				t.name, t.created_by,
			)
		})
		.collect::<Vec<_>>()
		.join("\n");

	crate::self_alerts::raise(
		db,
		EXPIRY_REF,
		CheckResult::Failed,
		CheckResult::Failed,
		false,
		Some(EXPIRY_DOC),
		"MCP access token nearing expiry",
		&message,
	)
	.await?;
	Ok(1)
}
