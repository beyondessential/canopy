//! Certificate orders and the certificates they produce (CRT).
//!
//! One row per (name, certified key). A repeat request for a key Canopy already
//! holds a certificate for is answered from the row rather than ordering again,
//! which is what keeps a server that lost its local copy from spending the
//! authority's budget; a request naming a different key is a different row and a
//! new order.
//!
//! The submitted signing request is kept because renewal reuses it: the key has
//! not changed, so Canopy renews without needing anything from the server.
// spec: CRT

use commons_errors::{AppError, Result};
use commons_types::{backoff::Backoff, dns::normalize_domain};
use diesel::prelude::*;
use diesel::result::Error as DieselError;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::{SignedDuration, Timestamp};
use serde::Serialize;
use uuid::Uuid;

/// The fraction of a certificate's life still remaining when Canopy starts
/// trying to renew, used when the authority publishes no renewal information of
/// its own.
///
/// A fraction rather than a duration because no duration serves both lifetimes
/// an authority may offer: a window measured in weeks leaves a certificate that
/// lives six days permanently overdue, and one measured in hours renews a
/// ninety-day certificate hundreds of times over. At a third, a ninety-day
/// certificate renews with thirty days spare and a six-day one with two — in both
/// cases room for many failed attempts before anything is at risk.
const RENEW_AT_FRACTION_REMAINING: i64 = 3;

/// The fraction of remaining life at which an unrenewed certificate stops being
/// a warning and becomes a failure: half of the renewal window having passed
/// without success.
const CRITICAL_AT_FRACTION_REMAINING: i64 = 6;

/// How long Canopy waits between attempts at an order. Failures are usually the
/// authority being briefly unavailable or a record not yet visible, so the
/// interval grows — but not past the cap, or a certificate could expire while
/// Canopy waits.
pub const ORDER_BACKOFF: Backoff =
	Backoff::new(SignedDuration::from_secs(60), SignedDuration::from_hours(6));

/// The longest Canopy waits between attempts at an order.
pub const MAX_BACKOFF: SignedDuration = ORDER_BACKOFF.cap();

/// Where an order stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderState {
	/// Waiting to be worked, or waiting to be retried after a failure.
	Pending,
	/// A certificate is held.
	Issued,
	/// Given up on for now; the reason is on the row.
	Failed,
	/// Revoked by an operator. Not renewed, not collected, not held: a revoked
	/// certificate is not a certificate any more.
	Revoked,
}

impl OrderState {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Pending => "pending",
			Self::Issued => "issued",
			Self::Failed => "failed",
			Self::Revoked => "revoked",
		}
	}

	pub fn from_column(raw: &str) -> Self {
		match raw {
			"issued" => Self::Issued,
			"failed" => Self::Failed,
			"revoked" => Self::Revoked,
			_ => Self::Pending,
		}
	}
}

/// A certificate Canopy holds, or an order in flight to obtain one.
#[derive(Debug, Clone, Serialize, Queryable, Selectable, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::application_certificates)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct ApplicationCertificate {
	pub id: Uuid,
	/// The server the certificate was obtained for.
	pub application_id: Uuid,
	/// The single name it covers, normalised.
	pub name: String,
	/// Hex SHA-256 over the subject public key info of the certified key.
	pub key_fingerprint: String,
	/// The signing request as submitted, kept so renewal needs nothing from the
	/// server.
	#[serde(skip)]
	#[schema(value_type = String)]
	pub csr: Vec<u8>,
	/// `pending`, `issued`, or `failed`.
	pub state: String,
	/// The issued chain, PEM. Public material only.
	pub chain: Option<String>,
	/// When the certificate expires.
	#[serde(skip_serializing_if = "Option::is_none")]
	#[diesel(
		deserialize_as = jiff_diesel::NullableTimestamp,
		serialize_as = jiff_diesel::NullableTimestamp,
		treat_none_as_default_value = false
	)]
	pub not_after: Option<Timestamp>,
	/// When it was issued.
	#[serde(skip_serializing_if = "Option::is_none")]
	#[diesel(
		deserialize_as = jiff_diesel::NullableTimestamp,
		serialize_as = jiff_diesel::NullableTimestamp,
		treat_none_as_default_value = false
	)]
	pub issued_at: Option<Timestamp>,
	/// Whether the order in flight is extending a certificate that already
	/// issued, so a renewal failure is told apart from a first issuance that
	/// never came up.
	pub renewing: bool,
	pub attempts: i32,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub next_attempt_at: Timestamp,
	/// Why the last attempt failed, if it did.
	pub last_error: Option<String>,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub updated_at: Timestamp,
	/// The profile the certificate was issued under, which is the authority's
	/// name for a lifetime. `None` for one issued before Canopy asked for a
	/// profile, or where the authority offers none.
	pub profile: Option<String>,
	/// When to next consider renewing: from the authority's own renewal
	/// information where it publishes any, otherwise a fraction of this
	/// certificate's life. `None` until something has been issued.
	#[serde(skip_serializing_if = "Option::is_none")]
	#[diesel(
		deserialize_as = jiff_diesel::NullableTimestamp,
		serialize_as = jiff_diesel::NullableTimestamp,
		treat_none_as_default_value = false
	)]
	pub renew_after: Option<Timestamp>,
	/// When an operator revoked this certificate.
	#[serde(skip_serializing_if = "Option::is_none")]
	#[diesel(
		deserialize_as = jiff_diesel::NullableTimestamp,
		serialize_as = jiff_diesel::NullableTimestamp,
		treat_none_as_default_value = false
	)]
	pub revoked_at: Option<Timestamp>,
	/// Who revoked it.
	pub revoked_by: Option<String>,
	/// The reason given, by name.
	pub revocation_reason: Option<String>,
}

/// Why a certificate was revoked. The names are the RFC 5280 reasons an
/// authority accepts; Canopy offers the few an operator would actually reach
/// for rather than the whole set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RevocationReason {
	/// No reason given.
	Unspecified,
	/// The private key is known to be exposed. Additionally bars that key from
	/// ever being certified again.
	KeyCompromise,
	/// Replaced by another certificate.
	Superseded,
	/// The name is no longer in service.
	CessationOfOperation,
}

impl RevocationReason {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Unspecified => "unspecified",
			Self::KeyCompromise => "key_compromise",
			Self::Superseded => "superseded",
			Self::CessationOfOperation => "cessation_of_operation",
		}
	}

	/// Whether this reason means the key itself must never be certified again.
	pub fn bars_the_key(self) -> bool {
		matches!(self, Self::KeyCompromise)
	}
}

/// One certificate going stale under a paused server, with enough of the pause
/// to say who has forgotten what.
#[derive(Debug, Clone)]
pub struct PausedLapse {
	pub application_id: Uuid,
	pub server_name: String,
	/// The name the certificate covers.
	pub name: String,
	pub not_after: Option<Timestamp>,
	/// Whether it has run out already, as against merely being past renewal.
	pub expired: bool,
	pub paused_at: Option<Timestamp>,
	pub pause_reason: Option<String>,
}

/// How urgently a held certificate needs attention, judged against its own
/// lifetime so the same reading applies to a six-day certificate and a
/// ninety-day one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
	/// Comfortably current.
	None,
	/// Past the point Canopy meant to renew, with room left to recover.
	AtRisk,
	/// Most of the renewal window gone, or expired outright.
	Critical,
}

impl ApplicationCertificate {
	pub fn order_state(&self) -> OrderState {
		OrderState::from_column(&self.state)
	}

	/// Whether the held certificate is usable now: issued, and not expired.
	pub fn is_current(&self) -> bool {
		self.order_state() == OrderState::Issued
			&& self.chain.is_some()
			&& self.not_after.is_some_and(|at| at > Timestamp::now())
	}

	/// Whether this certificate was revoked in a way that condemns the key as
	/// well as the certificate.
	///
	/// A server collecting this needs the distinction: any revocation means stop
	/// serving the certificate, but only a compromised key means the key pair
	/// itself has to be replaced before asking again. Everything else can be
	/// re-requested with the key already held.
	// spec: CRT#revocation
	pub fn requires_new_key(&self) -> bool {
		self.revocation_reason.as_deref() == Some(RevocationReason::KeyCompromise.as_str())
	}

	/// Whether an operator has revoked this certificate.
	pub fn is_revoked(&self) -> bool {
		self.order_state() == OrderState::Revoked
	}

	/// Whether a server can still be served this chain.
	///
	/// Deliberately not the same question as [`Self::is_current`]. A renewal in
	/// flight puts the row back to pending while the chain it holds is still
	/// perfectly valid, and the server must keep being given it — otherwise an
	/// agent polling mid-renewal is told it has nothing and stops serving TLS on a
	/// name whose certificate has weeks left. What disqualifies a chain is being
	/// revoked or being expired, not there being newer work under way.
	// spec: CRT#fulfilment-is-not-immediate
	pub fn is_collectable(&self) -> bool {
		!self.is_revoked()
			&& self.chain.is_some()
			&& self.not_after.is_some_and(|at| at > Timestamp::now())
	}

	/// This certificate's whole life, from issuance to expiry. `None` for one not
	/// issued yet.
	pub fn lifetime(&self) -> Option<SignedDuration> {
		let (issued, expires) = (self.issued_at?, self.not_after?);
		(expires - issued).try_into().ok()
	}

	/// How much of this certificate's life is left. Negative once expired.
	pub fn remaining(&self) -> Option<SignedDuration> {
		(self.not_after? - Timestamp::now()).try_into().ok()
	}

	/// How urgently this certificate needs attention, as a fraction of its own
	/// lifetime rather than an absolute duration — so a six-day certificate isn't
	/// judged by a ninety-day certificate's standards.
	///
	/// An order with nothing issued yet carries no risk of its own: whether it is
	/// overdue is the order queue's business.
	pub fn risk(&self) -> Risk {
		let (Some(lifetime), Some(remaining)) = (self.lifetime(), self.remaining()) else {
			return Risk::None;
		};
		if remaining.is_zero() || remaining.is_negative() {
			return Risk::Critical;
		}
		let critical_at = lifetime / CRITICAL_AT_FRACTION_REMAINING as i32;
		let at_risk_at = lifetime / RENEW_AT_FRACTION_REMAINING as i32;
		if remaining <= critical_at {
			Risk::Critical
		} else if remaining <= at_risk_at {
			Risk::AtRisk
		} else {
			Risk::None
		}
	}

	/// Ask for a certificate for `name` covering the key in `csr`.
	///
	/// Idempotent by (name, key): a request for a key Canopy already holds a
	/// current certificate for returns that certificate untouched, one already
	/// in flight returns the order as it stands, and one that had failed is
	/// picked up again from the start. Only a genuinely new (name, key) opens a
	/// new order.
	pub async fn request(
		db: &mut AsyncPgConnection,
		application_id: Uuid,
		name: &str,
		key_fingerprint: &str,
		csr: &[u8],
	) -> Result<Self> {
		use crate::schema::application_certificates::dsl;

		let name = normalize_domain(name)?;

		// An order exists only for a name the application declares for
		// certificates, so Canopy always knows which workload a certificate is
		// for and knows to stop renewing it once the declaration ends. An
		// application may want a certificate before it is reachable, so the
		// request declares the name rather than needing an address registered.
		//
		// This path is device-facing, so a name another application holds is
		// refused word for word as a name nobody declares. `declare` itself
		// names the holder, which is right for an operator who sees the whole
		// fleet and wrong here, where it would make the endpoint a directory of
		// what other machines serve.
		// spec: DNS#declared-dns-names
		crate::application_certificate_names::ApplicationCertificateName::declare(
			db,
			application_id,
			&name,
		)
		.await
		.map_err(|err| match err {
			AppError::Conflict(_) => AppError::DnsNameUndeclared(name.clone()),
			other => other,
		})?;

		// A key revoked for compromise is never certified again, whatever asks
		// for it: the server has to generate a new one. Its own error type, not a
		// generic refusal, so an agent can rotate the key on the problem type
		// alone rather than parsing the sentence.
		if is_key_compromised(db, key_fingerprint).await? {
			return Err(AppError::CertificateKeyCompromised(format!(
				"key {key_fingerprint} will not be certified again; generate a fresh key pair, \
				 submit a new signing request for {name}, and discard the old key"
			)));
		}

		if let Some(existing) = Self::for_name_and_key(db, &name, key_fingerprint).await? {
			return match existing.order_state() {
				// Held and usable, or already being worked: nothing to do.
				OrderState::Issued if existing.is_current() => Ok(existing),
				OrderState::Pending => Ok(existing),
				// Expired, or given up on: try again now, from the start. The
				// server may have changed hands, so the requester is recorded
				// afresh.
				_ => diesel::update(dsl::application_certificates.filter(dsl::id.eq(existing.id)))
					.set((
						dsl::application_id.eq(application_id),
						dsl::state.eq(OrderState::Pending.as_str()),
						dsl::renewing.eq(existing.chain.is_some()),
						dsl::attempts.eq(0),
						dsl::next_attempt_at.eq(jiff_diesel::Timestamp::from(Timestamp::now())),
						dsl::last_error.eq::<Option<String>>(None),
					))
					.returning(Self::as_select())
					.get_result(db)
					.await
					.map_err(AppError::from),
			};
		}

		diesel::insert_into(dsl::application_certificates)
			.values((
				dsl::application_id.eq(application_id),
				dsl::name.eq(&name),
				dsl::key_fingerprint.eq(key_fingerprint),
				dsl::csr.eq(csr),
				dsl::state.eq(OrderState::Pending.as_str()),
			))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)
	}

	pub async fn for_name_and_key(
		db: &mut AsyncPgConnection,
		name: &str,
		key_fingerprint: &str,
	) -> Result<Option<Self>> {
		use crate::schema::application_certificates::dsl;
		let name = normalize_domain(name)?;
		dsl::application_certificates
			.select(Self::as_select())
			.filter(dsl::name.eq(name))
			.filter(dsl::key_fingerprint.eq(key_fingerprint))
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	pub async fn get(db: &mut AsyncPgConnection, id: Uuid) -> Result<Self> {
		use crate::schema::application_certificates::dsl;
		dsl::application_certificates
			.select(Self::as_select())
			.filter(dsl::id.eq(id))
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)?
			.ok_or(AppError::DatabaseQuery(DieselError::NotFound))
	}

	/// Every certificate and in-flight order for a server, newest first.
	pub async fn for_server(db: &mut AsyncPgConnection, application_id: Uuid) -> Result<Vec<Self>> {
		use crate::schema::application_certificates::dsl;
		dsl::application_certificates
			.select(Self::as_select())
			.filter(dsl::application_id.eq(application_id))
			.order((dsl::name.asc(), dsl::created_at.desc()))
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Every certificate and in-flight order for any of `application_ids`, each
	/// name's newest first: one query for a page listing several applications.
	pub async fn for_applications(
		db: &mut AsyncPgConnection,
		application_ids: &[Uuid],
	) -> Result<Vec<Self>> {
		use crate::schema::application_certificates::dsl;
		dsl::application_certificates
			.select(Self::as_select())
			.filter(dsl::application_id.eq_any(application_ids))
			.order((dsl::name.asc(), dsl::created_at.desc()))
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Orders due to be attempted, soonest first. Claimed with `SKIP LOCKED` so
	/// two workers never drive the same order.
	///
	/// Skips paused applications: while a server is paused Canopy makes no new changes
	/// on its behalf, so its orders sit where they are and resume when the pause
	/// lifts.
	// spec: DNS#pausing-an-application
	pub async fn claim_due(db: &mut AsyncPgConnection, limit: i64) -> Result<Vec<Self>> {
		use crate::schema::{application_certificates, applications};

		let now = jiff_diesel::Timestamp::from(Timestamp::now());

		// Two steps on purpose. The eligibility question spans `applications` (is it
		// paused? archived?), but the lock belongs on the certificate rows alone:
		// `FOR UPDATE` over the join would lock server rows too, and an unrelated
		// edit to a server would then block a worker claiming its orders.
		let candidates: Vec<Uuid> = application_certificates::table
			.inner_join(applications::table)
			.filter(application_certificates::state.eq(OrderState::Pending.as_str()))
			.filter(application_certificates::next_attempt_at.le(now))
			.filter(applications::deleted_at.is_null())
			.filter(applications::name_management_paused_at.is_null())
			.select(application_certificates::id)
			.order(application_certificates::next_attempt_at.asc())
			.limit(limit)
			.load(db)
			.await
			.map_err(AppError::from)?;

		if candidates.is_empty() {
			return Ok(Vec::new());
		}

		// Re-apply the row-local filters: a candidate may have been worked by
		// another worker between the two statements.
		application_certificates::table
			.filter(application_certificates::id.eq_any(candidates))
			.filter(application_certificates::state.eq(OrderState::Pending.as_str()))
			.filter(application_certificates::next_attempt_at.le(now))
			.select(Self::as_select())
			.order(application_certificates::next_attempt_at.asc())
			.for_update()
			.skip_locked()
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Record a certificate obtained, clearing the order.
	///
	/// `renew_after` is when the authority would like it replaced, where it says;
	/// `None` falls back to a fraction of the certificate's own life.
	pub async fn record_issued(
		db: &mut AsyncPgConnection,
		id: Uuid,
		chain: &str,
		not_after: Timestamp,
		profile: Option<&str>,
		renew_after: Option<Timestamp>,
	) -> Result<()> {
		use crate::schema::application_certificates::dsl;

		let issued_at = Timestamp::now();
		let renew_after = renew_after.unwrap_or_else(|| default_renew_after(issued_at, not_after));

		diesel::update(dsl::application_certificates.filter(dsl::id.eq(id)))
			.set((
				dsl::state.eq(OrderState::Issued.as_str()),
				dsl::chain.eq(Some(chain)),
				dsl::not_after.eq(jiff_diesel::NullableTimestamp::from(Some(not_after))),
				dsl::issued_at.eq(jiff_diesel::NullableTimestamp::from(Some(issued_at))),
				dsl::profile.eq(profile),
				dsl::renew_after.eq(jiff_diesel::NullableTimestamp::from(Some(renew_after))),
				dsl::renewing.eq(false),
				dsl::attempts.eq(0),
				dsl::last_error.eq::<Option<String>>(None),
			))
			.execute(db)
			.await?;
		Ok(())
	}

	/// Record a failed attempt and when to try again.
	///
	/// The interval doubles per attempt up to [`MAX_BACKOFF`]. The order stays
	/// pending rather than becoming `failed`: an order Canopy has been asked for
	/// is worth continuing to try, and the alerting is what tells an operator it
	/// isn't working.
	pub async fn record_failure(db: &mut AsyncPgConnection, id: Uuid, error: &str) -> Result<()> {
		use crate::schema::application_certificates::dsl;

		let current = Self::get(db, id).await?;
		let attempts = current.attempts.saturating_add(1);
		let backoff = backoff_for(attempts);

		diesel::update(dsl::application_certificates.filter(dsl::id.eq(id)))
			.set((
				dsl::attempts.eq(attempts),
				dsl::next_attempt_at.eq(jiff_diesel::Timestamp::from(Timestamp::now() + backoff)),
				dsl::last_error.eq(Some(error)),
			))
			.execute(db)
			.await?;
		Ok(())
	}

	/// Certificates the authority (or, failing that, their own lifetime) says it
	/// is time to renew. Marks each pending so the worker picks it up, and
	/// returns them.
	pub async fn start_renewals(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::application_certificates::dsl;

		let now = Timestamp::now();
		// Paused applications are skipped: their renewals fall due again when the
		// pause lifts.
		// spec: DNS#pausing-an-application
		let due: Vec<Uuid> = {
			use crate::schema::{application_certificates, applications};
			application_certificates::table
				.inner_join(applications::table)
				.filter(application_certificates::state.eq(OrderState::Issued.as_str()))
				.filter(application_certificates::renew_after.is_not_null())
				.filter(
					application_certificates::renew_after
						.le(jiff_diesel::NullableTimestamp::from(Some(now))),
				)
				.filter(applications::deleted_at.is_null())
				.filter(applications::name_management_paused_at.is_null())
				.filter(still_declared())
				.select(application_certificates::id)
				.load(db)
				.await
				.map_err(AppError::from)?
		};

		if due.is_empty() {
			return Ok(Vec::new());
		}

		diesel::update(dsl::application_certificates.filter(dsl::id.eq_any(&due)))
			.set((
				dsl::state.eq(OrderState::Pending.as_str()),
				dsl::renewing.eq(true),
				dsl::attempts.eq(0),
				dsl::next_attempt_at.eq(jiff_diesel::Timestamp::from(Timestamp::now())),
			))
			.execute(db)
			.await?;

		dsl::application_certificates
			.select(Self::as_select())
			.filter(dsl::id.eq_any(due))
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Held certificates whose remaining life has run down far enough to report,
	/// each with how urgently — the per-server alert's work list.
	///
	/// Only certificates for names the server is *still entitled to* are
	/// returned. A group that released the domain, a grant revoked, or a server
	/// archived all stop Canopy renewing, so the certificate running out is the
	/// intended outcome rather than a failure; reporting it would leave an alert
	/// behind that no action could clear. Entitlement is asked here rather than
	/// remembered from when renewal stopped, so a domain reclaimed by its group
	/// brings its certificates back into scope.
	// spec: CRT#when-issuance-fails
	pub async fn at_risk(db: &mut AsyncPgConnection) -> Result<Vec<(Self, Risk)>> {
		use crate::schema::{application_certificates, applications};

		let held: Vec<(Self, Option<Uuid>, bool)> = application_certificates::table
			.inner_join(applications::table)
			.filter(application_certificates::state.eq(OrderState::Issued.as_str()))
			.filter(applications::deleted_at.is_null())
			.filter(applications::may_manage_tls.eq(true))
			// A paused server raises nothing: Canopy was told to stop acting on
			// its behalf, so a certificate running down is the expected
			// consequence. The pause is what gets reported instead.
			// spec: DNS#pausing-an-application
			.filter(applications::name_management_paused_at.is_null())
			.filter(still_declared())
			.select((
				Self::as_select(),
				applications::group_id,
				applications::may_manage_tls,
			))
			.load(db)
			.await
			.map_err(AppError::from)?;

		if held.is_empty() {
			return Ok(Vec::new());
		}

		// One read of every claim, then match in memory: the alternative is a
		// suffix join per certificate, and the claim table has one row per domain
		// a group controls.
		let claims = crate::server_domains::ServerGroupDomain::list_all(db).await?;

		let mut out = Vec::new();
		for (cert, group_id, _) in held {
			let risk = cert.risk();
			if risk == Risk::None {
				continue;
			}
			// The name has to still sit under a domain the server's own group
			// controls.
			let entitled = group_id.is_some_and(|group| {
				claims.iter().any(|claim| {
					claim.group_id == group
						&& commons_types::dns::is_within(&cert.name, &claim.domain)
				})
			});
			if entitled {
				out.push((cert, risk));
			}
		}
		out.sort_by_key(|(cert, _)| cert.not_after);
		Ok(out)
	}

	/// Certificates lapsing underneath a pause: past the point Canopy would have
	/// renewed them, or expired outright, on a server it has been told to stop
	/// acting for.
	///
	/// This is the other half of [`Self::at_risk`], which deliberately skips paused
	/// applications. A pause suppresses the per-server alerting that would chase a
	/// certificate running out, so the forgetting is what has to become visible —
	/// and against Canopy rather than the group, since nobody in that
	/// group can lift a pause.
	///
	/// Entitlement is filtered the same way: a name the server is no longer
	/// entitled to raises nothing however far past expiry it is, because Canopy
	/// stopped renewing it on purpose.
	// spec: DNS#pausing-an-application
	pub async fn lapsing_under_pause(db: &mut AsyncPgConnection) -> Result<Vec<PausedLapse>> {
		use crate::schema::{application_certificates, applications};

		/// `(certificate, group, server name, server type, paused at, pause
		/// reason)` — the columns the report needs from either side of the join.
		type PausedRow = (
			ApplicationCertificate,
			Option<Uuid>,
			Option<String>,
			String,
			jiff_diesel::NullableTimestamp,
			Option<String>,
		);

		let held: Vec<PausedRow> = application_certificates::table
			.inner_join(applications::table)
			.filter(application_certificates::state.eq(OrderState::Issued.as_str()))
			.filter(applications::deleted_at.is_null())
			.filter(applications::may_manage_tls.eq(true))
			.filter(applications::name_management_paused_at.is_not_null())
			.select((
				Self::as_select(),
				applications::group_id,
				applications::name,
				applications::type_,
				applications::name_management_paused_at,
				applications::name_management_pause_reason,
			))
			.load(db)
			.await
			.map_err(AppError::from)?;

		if held.is_empty() {
			return Ok(Vec::new());
		}

		let claims = crate::server_domains::ServerGroupDomain::list_all(db).await?;
		let now = Timestamp::now();

		let mut out = Vec::new();
		for (cert, group_id, server_name, server_type, paused_at, pause_reason) in held {
			let paused_at: Option<Timestamp> = paused_at.into();
			// Past renewal is the interesting line here, not `risk()`: what wants
			// reporting is that Canopy would have acted by now and did not, which
			// happens well before the certificate is in trouble.
			let overdue = cert.renew_after.is_some_and(|at| at <= now);
			let expired = cert.not_after.is_some_and(|at| at <= now);
			if !overdue && !expired {
				continue;
			}
			let entitled = group_id.is_some_and(|group| {
				claims.iter().any(|claim| {
					claim.group_id == group
						&& commons_types::dns::is_within(&cert.name, &claim.domain)
				})
			});
			if !entitled {
				continue;
			}
			out.push(PausedLapse {
				application_id: cert.application_id,
				server_name: crate::applications::Application::display_name_of(
					server_name,
					&server_type,
				),
				name: cert.name.clone(),
				not_after: cert.not_after,
				expired,
				paused_at,
				pause_reason,
			});
		}
		out.sort_by_key(|lapse| lapse.not_after);
		Ok(out)
	}

	/// Orders that have never produced a certificate and have failed repeatedly
	/// — a server that never came up, as distinct from one about to go dark.
	pub async fn stuck_first_issuances(
		db: &mut AsyncPgConnection,
		min_attempts: i32,
	) -> Result<Vec<Self>> {
		use crate::schema::application_certificates::dsl;
		dsl::application_certificates
			.select(Self::as_select())
			.filter(dsl::chain.is_null())
			.filter(dsl::attempts.ge(min_attempts))
			.order(dsl::created_at.asc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Record a certificate as revoked, once the authority has accepted it.
	///
	/// Revoking stops renewal and stops the certificate being collected: it is no
	/// longer something Canopy holds. Where the reason is that the key is
	/// compromised, that key is barred from ever being certified again — for any
	/// name, by any server, since a leaked key is leaked whoever asks next.
	///
	/// Not reversible: the remedy for a mistaken revocation is a new certificate.
	// spec: CRT#revocation
	pub async fn record_revoked(
		db: &mut AsyncPgConnection,
		id: Uuid,
		reason: RevocationReason,
		revoked_by: Option<&str>,
	) -> Result<()> {
		use crate::schema::{application_certificates::dsl, compromised_keys};
		use diesel_async::AsyncConnection;

		let cert = Self::get(db, id).await?;
		let by = revoked_by.map(str::to_string);

		db.transaction::<_, AppError, _>(async |conn| {
			diesel::update(dsl::application_certificates.filter(dsl::id.eq(id)))
				.set((
					dsl::state.eq(OrderState::Revoked.as_str()),
					dsl::revoked_at
						.eq(jiff_diesel::NullableTimestamp::from(Some(Timestamp::now()))),
					dsl::revoked_by.eq(by.as_deref()),
					dsl::revocation_reason.eq(reason.as_str()),
					// Nothing to renew any more.
					dsl::renew_after.eq(jiff_diesel::NullableTimestamp::from(None)),
				))
				.execute(conn)
				.await
				.map_err(AppError::from)?;

			// Stop the machinery rather than merely redirecting it: an agent would
			// otherwise request a replacement within minutes, and if the key leaked
			// because the host was compromised that hands the same attacker a
			// fresh certificate. An operator decides when to start again.
			// spec: DNS#pausing-an-application
			crate::applications::Application::pause_name_management(
				conn,
				cert.application_id,
				by.as_deref(),
				&format!(
					"certificate for {} revoked ({})",
					cert.name,
					reason.as_str()
				),
			)
			.await?;

			if reason.bars_the_key() {
				diesel::insert_into(compromised_keys::table)
					.values((
						compromised_keys::key_fingerprint.eq(&cert.key_fingerprint),
						compromised_keys::certificate_id.eq(Some(id)),
						compromised_keys::noted_by.eq(by.as_deref()),
					))
					// Already barred by an earlier revocation: still barred.
					.on_conflict(compromised_keys::key_fingerprint)
					.do_nothing()
					.execute(conn)
					.await
					.map_err(AppError::from)?;
			}
			Ok(())
		})
		.await
	}

	/// Stop working an order and stop renewing what it produced — the name is no
	/// longer the group's, or the server may no longer hold it.
	pub async fn stop(db: &mut AsyncPgConnection, id: Uuid, reason: &str) -> Result<()> {
		use crate::schema::application_certificates::dsl;
		diesel::update(dsl::application_certificates.filter(dsl::id.eq(id)))
			.set((
				dsl::state.eq(OrderState::Failed.as_str()),
				dsl::last_error.eq(Some(reason)),
			))
			.execute(db)
			.await?;
		Ok(())
	}
}

/// Whether this key has been revoked as compromised, and so must never be
/// certified again.
pub async fn is_key_compromised(db: &mut AsyncPgConnection, key_fingerprint: &str) -> Result<bool> {
	use crate::schema::compromised_keys::dsl;
	let found: Option<String> = dsl::compromised_keys
		.select(dsl::key_fingerprint)
		.filter(dsl::key_fingerprint.eq(key_fingerprint))
		.first(db)
		.await
		.optional()
		.map_err(AppError::from)?;
	Ok(found.is_some())
}

/// Certificates whose name the application still declares.
///
/// Canopy acts on a name for the application that declares it, so once a
/// declaration ends — released to another workload on the box, or forgotten
/// after the name was withdrawn — it stops renewing that name's certificates
/// and stops raising them as running out. Renewing past a release would order
/// for a name another application now serves, and alerting on it would report a
/// deliberate act as a fault. What was already issued stays issued and
/// collectable until it expires.
// spec: DNS#declared-dns-names
type StillDeclared = diesel::dsl::exists<
	diesel::helper_types::Filter<
		diesel::helper_types::Filter<
			crate::schema::application_certificate_names::table,
			diesel::dsl::Eq<
				crate::schema::application_certificate_names::application_id,
				crate::schema::application_certificates::application_id,
			>,
		>,
		diesel::dsl::Eq<
			crate::schema::application_certificate_names::name,
			crate::schema::application_certificates::name,
		>,
	>,
>;

fn still_declared() -> StillDeclared {
	use crate::schema::{application_certificate_names, application_certificates};
	diesel::dsl::exists(
		application_certificate_names::table
			.filter(
				application_certificate_names::application_id
					.eq(application_certificates::application_id),
			)
			.filter(application_certificate_names::name.eq(application_certificates::name)),
	)
}

/// When to renew a certificate whose authority publishes no renewal information
/// of its own: once all but a fraction of its life has passed.
pub fn default_renew_after(issued_at: Timestamp, not_after: Timestamp) -> Timestamp {
	let Ok(lifetime): std::result::Result<SignedDuration, _> = (not_after - issued_at).try_into()
	else {
		return issued_at;
	};
	// A certificate whose expiry is not after its issuance is already spent;
	// renewing it immediately is the only sensible reading.
	if lifetime.is_negative() || lifetime.is_zero() {
		return issued_at;
	}
	not_after - lifetime / RENEW_AT_FRACTION_REMAINING as i32
}

/// How long to wait after an order's `attempts`th failure, on [`ORDER_BACKOFF`].
pub fn backoff_for(attempts: i32) -> SignedDuration {
	ORDER_BACKOFF.after(attempts.max(0) as u32)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn backoff_grows_then_settles() {
		assert_eq!(backoff_for(1), SignedDuration::from_secs(60));
		assert_eq!(backoff_for(2), SignedDuration::from_secs(120));
		assert_eq!(backoff_for(3), SignedDuration::from_secs(240));
		// Capped, and stays capped however many attempts have gone by.
		assert_eq!(backoff_for(20), MAX_BACKOFF);
		assert_eq!(backoff_for(i32::MAX), MAX_BACKOFF);
		// A zeroth attempt is treated as the first rather than as no wait.
		assert_eq!(backoff_for(0), SignedDuration::from_secs(60));
	}

	#[test]
	fn order_state_round_trips_through_the_column() {
		for state in [OrderState::Pending, OrderState::Issued, OrderState::Failed] {
			assert_eq!(OrderState::from_column(state.as_str()), state);
		}
		// Anything unrecognised is treated as work still to do rather than as
		// success.
		assert_eq!(
			OrderState::from_column("something else"),
			OrderState::Pending
		);
	}
}
