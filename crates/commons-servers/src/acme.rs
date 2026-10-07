//! Obtaining certificates from an ACME authority.
//!
//! Canopy holds the account and the zone access; a server holds the private key.
//! That split is the whole point: Canopy can prove control of a name, which is
//! what an authority asks for, and never has to be told a private key to do it.
//!
//! The order is driven end to end here — publish the challenge record, tell the
//! authority to look, hand over the server's signing request, collect the chain,
//! clean the record up — because every step of it is one conversation with one
//! authority about one name, and splitting it across the worker would leave a
//! challenge record behind whenever a step in between failed.
// spec: CRT#certificates

use std::sync::{Arc, Mutex};
use std::time::Duration;

use commons_errors::{AppError, Result};
use commons_types::dns::ManagedZone;
use hickory_resolver::{
	Resolver, TokioResolver,
	config::{CLOUDFLARE, GOOGLE, ResolverConfig, ResolverOpts, ServerGroup},
	net::runtime::TokioRuntimeProvider,
	proto::rr::{RData, RecordType},
};
use instant_acme::{
	Account, AuthorizationStatus, CertificateIdentifier, ChallengeType, Identifier, LetsEncrypt,
	NewOrder, OrderStatus, RetryPolicy, RevocationRequest,
};
use jiff::Timestamp;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, pem::PemObject};
use tokio::time::Instant;
use tracing::{debug, info, warn};

use crate::dns_provider::{DnsProvider, RecordSet};

/// How long to wait for a challenge record Canopy has just published to become
/// visible to the authority before giving up on this attempt. Generous, because
/// a resolver holding a negative answer is the common case and the alternative
/// to waiting is spending another authorisation; the order stays pending either
/// way and the caller's backoff handles a genuine stall.
const AUTHORISATION_TIMEOUT: Duration = Duration::from_secs(180);

/// How long to wait for the authority to sign after the request is handed over.
/// Signing is prompt; this is a backstop, not a budget.
const FINALISE_TIMEOUT: Duration = Duration::from_secs(120);

/// How long to wait for a freshly-published challenge record to become visible
/// to resolvers independent of Canopy's own zone before telling the authority
/// to look. The authority validates promptly once asked and does not keep
/// retrying the DNS lookup on Canopy's behalf, so signalling ready before the
/// write has propagated anywhere wastes the whole attempt rather than merely
/// risking it — [`AUTHORISATION_TIMEOUT`] is patience for the authority's
/// answer, not for the record to exist in the first place.
const PROPAGATION_TIMEOUT: Duration = Duration::from_secs(90);

/// How often to recheck while waiting for propagation.
const PROPAGATION_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Per-query timeout against one public resolver, short so an unreachable
/// resolver cannot itself eat the [`PROPAGATION_TIMEOUT`] budget.
const PROPAGATION_QUERY_TIMEOUT: Duration = Duration::from_secs(3);

/// Whose problem a failed conversation with the authority is. The type is shared
/// with the alerting, which decides what to report from it; the classification
/// below is this module's, because only here is the authority's own error visible.
pub use commons_types::acme::AuthorityFault as Fault;

/// Read the authority's error for whose problem it is.
// spec: CRT#when-issuance-fails
fn fault_of(error: &instant_acme::Error) -> Fault {
	use instant_acme::Error;
	match error {
		Error::Api(problem) => match problem.r#type.as_deref() {
			Some(t) if t.ends_with(":rateLimited") => Fault::Throttled,
			Some(t)
				if t.ends_with(":accountDoesNotExist")
					|| t.ends_with(":unauthorized")
					|| t.ends_with(":userActionRequired") =>
			{
				Fault::Account
			}
			_ => Fault::Order,
		},
		// A request that got no answer and one that got it too late are the same
		// thing from Canopy's side, and call for the same response. `Hyper` is the
		// transport error in practice, instant-acme's default client being enabled.
		Error::Http(_) | Error::InvalidUri(_) | Error::Timeout(_) | Error::Hyper(_) => {
			Fault::Unreachable
		}
		// A key Canopy cannot use is an account Canopy cannot use.
		Error::Crypto | Error::KeyRejected => Fault::Account,
		_ => Fault::Order,
	}
}

/// A failed conversation with the authority, carrying whose problem it is.
#[derive(Debug, Clone)]
pub struct Failure {
	pub fault: Fault,
	pub message: String,
}

impl Failure {
	/// A failure of this order alone.
	fn order(message: impl Into<String>) -> Self {
		Self {
			fault: Fault::Order,
			message: message.into(),
		}
	}

	/// Classify an error from the ACME client, keeping `context` as the sentence
	/// an operator reads.
	fn from_acme(context: &str, error: instant_acme::Error) -> Self {
		Self {
			fault: fault_of(&error),
			message: format!("{context}: {error}"),
		}
	}
}

impl std::fmt::Display for Failure {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(&self.message)
	}
}

impl std::error::Error for Failure {}

impl From<Failure> for AppError {
	fn from(failure: Failure) -> Self {
		AppError::Upstream(failure.message)
	}
}

impl From<AppError> for Failure {
	/// A Canopy-side error inside an order — a zone write, an unreadable chain —
	/// is that order's problem and not the authority's.
	fn from(error: AppError) -> Self {
		Self::order(error.to_string())
	}
}

/// The result of a conversation with the authority.
pub type AcmeResult<T> = std::result::Result<T, Failure>;

/// What Canopy got back from an authority, and what it wants to remember.
#[derive(Debug, Clone)]
pub struct Issued {
	/// The chain, PEM, leaf first.
	pub chain: String,
	/// When the leaf expires, read from the certificate rather than assumed from
	/// the profile — the authority's word on its own lifetime is the only one that
	/// counts.
	pub not_after: Timestamp,
	/// The profile it was issued under, where one was asked for.
	pub profile: Option<String>,
	/// When the authority would like it replaced, where it publishes renewal
	/// information. `None` leaves the caller to fall back on a fraction of the
	/// certificate's own life.
	pub renew_after: Option<Timestamp>,
}

/// A revocation reason as the authority names it.
/// `database::application_certificates::RevocationReason` maps onto this by name;
/// kept separate so the database model does not depend on an ACME client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokeFor {
	Unspecified,
	KeyCompromise,
	Superseded,
	CessationOfOperation,
}

impl RevokeFor {
	fn as_acme(self) -> instant_acme::RevocationReason {
		match self {
			Self::Unspecified => instant_acme::RevocationReason::Unspecified,
			Self::KeyCompromise => instant_acme::RevocationReason::KeyCompromise,
			Self::Superseded => instant_acme::RevocationReason::Superseded,
			Self::CessationOfOperation => instant_acme::RevocationReason::CessationOfOperation,
		}
	}

	/// Read from the name Canopy stored against the certificate. An unrecognised
	/// reason revokes for no stated reason rather than not revoking: the operator
	/// asked for the certificate to stop being trusted, and that part is clear.
	pub fn from_stored(name: &str) -> Self {
		match name {
			"key_compromise" => Self::KeyCompromise,
			"superseded" => Self::Superseded,
			"cessation_of_operation" => Self::CessationOfOperation,
			_ => Self::Unspecified,
		}
	}
}

/// The certificate authority Canopy uses, or a stand-in that signs its own.
#[derive(Clone)]
pub enum Acme {
	Real(Arc<Account>),
	/// Signs with a throwaway authority of its own instead of talking to a real
	/// one, so the worker's whole path is exercisable — including the challenge
	/// records it publishes, which the fake DNS provider records just as a real
	/// order's would be.
	Fake(Arc<Mutex<FakeCa>>),
}

/// The state behind [`Acme::Fake`].
#[derive(Debug)]
pub struct FakeCa {
	/// The names it has signed for, oldest first.
	pub issued: Vec<String>,
	/// The chains it has been asked to revoke.
	pub revoked: Vec<String>,
	/// When set, every order and revocation fails with this message, blamed on
	/// this fault.
	pub fail_with: Option<(Fault, String)>,
	/// How long the certificates it signs live. Adjustable so a test can hold one
	/// that is already in its renewal window.
	pub lifetime: Duration,
	/// What it claims to advertise, so the profile path is exercisable.
	pub profiles: Vec<String>,
	/// When set, reported as the authority's renewal information.
	pub renew_after: Option<Timestamp>,
}

impl Default for FakeCa {
	fn default() -> Self {
		Self {
			issued: Vec::new(),
			revoked: Vec::new(),
			fail_with: None,
			lifetime: Duration::from_secs(90 * 24 * 60 * 60),
			profiles: vec!["classic".into(), "shortlived".into()],
			renew_after: None,
		}
	}
}

impl std::fmt::Debug for Acme {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Real(_) => f.write_str("Acme::Real"),
			Self::Fake(_) => f.write_str("Acme::Fake"),
		}
	}
}

impl Acme {
	/// Build from the Canopy instance's configuration, or `None` where no account
	/// key is set — an instance that has not been given one is not expected to issue,
	/// and the worker says so once rather than failing every order.
	///
	/// - `CANOPY_ACME_ACCOUNT_KEY`: PKCS#8 PEM private key for Canopy's account at
	///   the authority. The account is found or created from the key on every
	///   start, so there is one secret to hold and no state to write back.
	/// - `CANOPY_ACME_DIRECTORY`: the authority's directory URL. Defaults to
	///   Let's Encrypt production.
	/// - `CANOPY_ACME_CONTACT`: a contact URI (`mailto:…`) the authority can reach
	///   an operator at.
	pub async fn from_env() -> AcmeResult<Option<Self>> {
		let Ok(key_pem) = std::env::var("CANOPY_ACME_ACCOUNT_KEY") else {
			return Ok(None);
		};
		let directory = std::env::var("CANOPY_ACME_DIRECTORY")
			.unwrap_or_else(|_| LetsEncrypt::Production.url().to_string());

		let pkcs8 =
			PrivatePkcs8KeyDer::from_pem_slice(key_pem.as_bytes()).map_err(|e| Failure {
				fault: Fault::Account,
				message: format!("CANOPY_ACME_ACCOUNT_KEY is not a PKCS#8 PEM private key: {e}"),
			})?;
		let key = instant_acme::Key::from_pkcs8_der(pkcs8.clone_key())
			.map_err(|e| Failure::from_acme("the ACME account key is unusable", e))?;

		let builder = Account::builder()
			.map_err(|e| Failure::from_acme("could not build an ACME client", e))?;
		let (account, _credentials) = builder
			.create_from_key((key, PrivateKeyDer::Pkcs8(pkcs8)), directory.clone())
			.await
			.map_err(|e| Failure::from_acme("could not use the ACME account", e))?;

		// Set separately because finding-or-creating from a key carries no contact.
		// A contact the authority won't take is worth reporting but not worth
		// refusing to issue over.
		if let Ok(contact) = std::env::var("CANOPY_ACME_CONTACT")
			&& let Err(err) = account.update_contacts(&[contact.as_str()]).await
		{
			warn!("the authority would not record the configured contact: {err}");
		}

		info!(
			directory = %directory,
			account = %account.id(),
			profiles = ?account.profiles().map(|p| p.name.to_string()).collect::<Vec<_>>(),
			"ACME account ready"
		);
		Ok(Some(Self::Real(Arc::new(account))))
	}

	/// An authority that signs its own certificates without leaving the process.
	pub fn fake() -> Self {
		Self::Fake(Arc::new(Mutex::new(FakeCa::default())))
	}

	/// Make an [`Acme::Fake`] fail every order as this order's own problem, to
	/// exercise retry and the per-server alerting.
	pub fn fail_with(&self, message: impl Into<String>) {
		self.fail_with_fault(Fault::Order, message);
	}

	/// Make an [`Acme::Fake`] fail every order and blame `fault` — for the
	/// fleet-wide paths, where what is being tested is that Canopy reports the
	/// authority rather than the server that happened to ask.
	pub fn fail_with_fault(&self, fault: Fault, message: impl Into<String>) {
		if let Self::Fake(state) = self {
			state.lock().expect("fake ca lock").fail_with = Some((fault, message.into()));
		}
	}

	/// Let an [`Acme::Fake`] succeed again, after [`Acme::fail_with`].
	pub fn recover(&self) {
		if let Self::Fake(state) = self {
			state.lock().expect("fake ca lock").fail_with = None;
		}
	}

	/// How long an [`Acme::Fake`]'s certificates live. No effect on a real
	/// authority, which decides for itself.
	pub fn set_lifetime(&self, lifetime: Duration) {
		if let Self::Fake(state) = self {
			state.lock().expect("fake ca lock").lifetime = lifetime;
		}
	}

	/// Make an [`Acme::Fake`] publish renewal information, as an authority
	/// supporting ARI does.
	pub fn set_renew_after(&self, at: Timestamp) {
		if let Self::Fake(state) = self {
			state.lock().expect("fake ca lock").renew_after = Some(at);
		}
	}

	/// The names an [`Acme::Fake`] has signed for, oldest first.
	pub fn signed(&self) -> Vec<String> {
		match self {
			Self::Real(_) => Vec::new(),
			Self::Fake(state) => state.lock().expect("fake ca lock").issued.clone(),
		}
	}

	/// The chains an [`Acme::Fake`] has been asked to revoke.
	pub fn revocations(&self) -> Vec<String> {
		match self {
			Self::Real(_) => Vec::new(),
			Self::Fake(state) => state.lock().expect("fake ca lock").revoked.clone(),
		}
	}

	/// The profiles the authority advertises, as it names them. Empty where it
	/// advertises none, which is not the same as Canopy having no opinion: it
	/// means asking for one would be refused.
	// spec: CRT#lifetime
	pub fn profiles(&self) -> Vec<String> {
		match self {
			Self::Real(account) => account.profiles().map(|p| p.name.to_string()).collect(),
			Self::Fake(state) => state.lock().expect("fake ca lock").profiles.clone(),
		}
	}

	/// Obtain a certificate for `name` from the signing request `csr_der`.
	///
	/// `replacing` is the chain this order extends, where it extends one: an
	/// authority that accounts for renewals wants to be told which certificate is
	/// being replaced, so a renewal is not counted as an additional certificate.
	/// A `profile` the authority does not advertise is refused by it rather than
	/// quietly ignored, which is the outcome the spec asks for.
	// spec: CRT#renewal
	pub async fn obtain(
		&self,
		dns: &DnsProvider,
		zone: &ManagedZone,
		name: &str,
		csr_der: &[u8],
		profile: Option<&str>,
		replacing: Option<&str>,
	) -> AcmeResult<Issued> {
		match self {
			Self::Fake(state) => Self::obtain_fake(state, dns, zone, name, profile).await,
			Self::Real(account) => {
				Self::obtain_real(account, dns, zone, name, csr_der, profile, replacing).await
			}
		}
	}

	/// Tell the authority a certificate is no longer to be trusted. Canopy's
	/// account obtained it, which is authority enough; the server's key is not
	/// needed and is not asked for.
	// spec: CRT#revocation
	pub async fn revoke(&self, chain_pem: &str, reason: RevokeFor) -> AcmeResult<()> {
		match self {
			Self::Fake(state) => {
				let mut state = state.lock().expect("fake ca lock");
				if let Some((fault, message)) = &state.fail_with {
					return Err(Failure {
						fault: *fault,
						message: format!("fake ca: {message}"),
					});
				}
				state.revoked.push(chain_pem.to_string());
				Ok(())
			}
			// Only the real path parses the chain, because only it has to: the
			// authority is told which certificate by its bytes.
			Self::Real(account) => account
				.revoke(&RevocationRequest {
					certificate: &leaf_der(chain_pem)?,
					reason: Some(reason.as_acme()),
				})
				.await
				.map_err(|e| {
					Failure::from_acme("the authority would not revoke the certificate", e)
				}),
		}
	}

	#[allow(clippy::too_many_arguments)]
	async fn obtain_real(
		account: &Account,
		dns: &DnsProvider,
		zone: &ManagedZone,
		name: &str,
		csr_der: &[u8],
		profile: Option<&str>,
		replacing: Option<&str>,
	) -> AcmeResult<Issued> {
		let identifiers = [Identifier::Dns(name.to_string())];
		let mut new_order = NewOrder::new(&identifiers);
		if let Some(profile) = profile {
			new_order = new_order.profile(profile);
		}
		// Only where the previous chain still parses: a stored chain Canopy cannot
		// read is no reason to refuse the renewal that would replace it.
		if let Some(replaces) = replacing
			.and_then(|pem| leaf_der(pem).ok())
			.and_then(|der| CertificateIdentifier::try_from(&der).ok())
			.map(CertificateIdentifier::into_owned)
		{
			new_order = new_order.replaces(replaces);
		}

		let mut order = account.new_order(&new_order).await.map_err(|e| {
			Failure::from_acme(
				&format!("the authority would not open an order for {name}"),
				e,
			)
		})?;

		// Whatever happens next, take back down every record put up: a TXT left at
		// `_acme-challenge` would help authorise the next order for that name
		// without Canopy having proved anything for it.
		let mut published: Vec<RecordSet> = Vec::new();
		let authorised = Self::authorise(dns, zone, &mut order, &mut published).await;
		for set in &published {
			if let Err(err) = dns.delete(zone, set).await {
				// Not fatal: the certificate is what was wanted, and a stale
				// challenge record is housekeeping rather than a failed issuance.
				// Still worth saying loudly, because it does weaken the next order.
				warn!(name, record = %set.name, "could not remove the challenge record: {err}");
			}
		}
		authorised?;

		order
			.finalize_csr(csr_der)
			.await
			.map_err(|e| Failure::from_acme("the authority refused the request", e))?;
		let chain = order
			.poll_certificate(&RetryPolicy::default().timeout(FINALISE_TIMEOUT))
			.await
			.map_err(|e| Failure::from_acme("the authority did not produce a certificate", e))?;

		let not_after = leaf_expiry(&chain)?;
		let renew_after = renewal_window(account, &chain).await;
		debug!(name, %not_after, ?renew_after, "certificate obtained");

		Ok(Issued {
			chain,
			not_after,
			profile: profile.map(str::to_string),
			renew_after,
		})
	}

	/// Satisfy every DNS-01 authorisation the order carries, recording what was
	/// published so the caller can take it back down.
	///
	/// Canopy orders one name at a time, so this is one authorisation in practice;
	/// the loop is here because the authority decides how many to ask for, not
	/// because a multi-name order is expected.
	async fn authorise(
		dns: &DnsProvider,
		zone: &ManagedZone,
		order: &mut instant_acme::Order,
		published: &mut Vec<RecordSet>,
	) -> AcmeResult<()> {
		let mut authorizations = order.authorizations();
		while let Some(handle) = authorizations.next().await {
			let mut handle =
				handle.map_err(|e| Failure::from_acme("could not read an authorisation", e))?;
			// Already proved, within the authority's reuse window: nothing to
			// publish and nothing to ask it to look at.
			if handle.status == AuthorizationStatus::Valid {
				continue;
			}

			let mut challenge = handle.challenge(ChallengeType::Dns01).ok_or_else(|| {
				Failure::order(
					"the authority offered no DNS-01 challenge, which is the only kind Canopy can \
					 answer",
				)
			})?;
			let subject = match challenge.identifier().identifier {
				Identifier::Dns(name) => name.clone(),
				other => {
					return Err(Failure::order(format!(
						"the authority asked Canopy to prove control of {other:?}, which is not a \
						 name it can publish a record for"
					)));
				}
			};

			let dns_value = challenge.key_authorization().dns_value();
			let set = RecordSet::challenge(&subject, &dns_value);
			let record_name = set.name.clone();
			dns.upsert(zone, &set).await?;
			// Recorded before the authority is told to look, so a failure between
			// the two still gets cleaned up.
			published.push(set);

			// The authority validates promptly once asked and will not keep
			// checking back on Canopy's behalf, so confirm the record is
			// actually visible — to resolvers that never asked Route 53 and so
			// cannot simply be agreeing with itself — before asking it to look.
			Self::wait_for_propagation(&record_name, &dns_value).await?;

			challenge
				.set_ready()
				.await
				.map_err(|e| Failure::from_acme("the authority would not check the record", e))?;
		}
		// Ends the borrow of `order`, which `poll_ready` needs mutably.
		let _ = authorizations;

		match order
			.poll_ready(&RetryPolicy::default().timeout(AUTHORISATION_TIMEOUT))
			.await
			.map_err(|e| Failure::from_acme("the authority did not validate the name", e))?
		{
			OrderStatus::Ready | OrderStatus::Valid => Ok(()),
			other => Err(Failure::order(format!(
				"the authority left the order {other:?} rather than ready to sign"
			))),
		}
	}

	/// A resolver for one public DNS service, bypassing whatever resolver Canopy's
	/// own network would otherwise use — the point of the check below is to ask
	/// someone who never saw the Route 53 write, so Route 53 agreeing with itself
	/// proves nothing.
	fn public_resolver(group: &ServerGroup<'_>) -> AcmeResult<TokioResolver> {
		// `ResolverOpts` is `#[non_exhaustive]`, so built from the default and
		// adjusted rather than as a struct literal.
		let mut options = ResolverOpts::default();
		options.timeout = PROPAGATION_QUERY_TIMEOUT;
		options.attempts = 1;
		Resolver::builder_with_config(
			ResolverConfig::udp_and_tcp(group),
			TokioRuntimeProvider::default(),
		)
		.with_options(options)
		.build()
		.map_err(|e| Failure::order(format!("could not build a propagation-check resolver: {e}")))
	}

	/// Whether `resolver` currently answers `name`'s TXT records with `expected`
	/// among them. A lookup error — commonly no record yet, which is exactly the
	/// state being waited out — counts as not yet seen rather than a hard failure.
	async fn resolver_sees(resolver: &TokioResolver, name: &str, expected: &str) -> bool {
		let Ok(lookup) = resolver.lookup(name, RecordType::TXT).await else {
			return false;
		};
		lookup.answers().iter().any(|record| match &record.data {
			RData::TXT(txt) => txt
				.txt_data
				.iter()
				.any(|segment| segment.as_ref() == expected.as_bytes()),
			_ => false,
		})
	}

	/// Wait until `name` resolves to `expected` for at least one resolver
	/// independent of Canopy's own zone, or give up after [`PROPAGATION_TIMEOUT`]
	/// and let the caller's ordinary order-level retry handle it.
	///
	/// Asking just one public resolver would only move the race rather than
	/// closing it, so this asks two unrelated services and accepts either: the
	/// point is evidence the write has left Route 53's own edge, not a survey of
	/// the whole internet's resolvers (the authority's own validation servers
	/// query independently and are not necessarily either of these).
	async fn wait_for_propagation(name: &str, expected: &str) -> AcmeResult<()> {
		let cloudflare = Self::public_resolver(&CLOUDFLARE)?;
		let google = Self::public_resolver(&GOOGLE)?;
		let deadline = Instant::now() + PROPAGATION_TIMEOUT;
		loop {
			let (seen_cloudflare, seen_google) = tokio::join!(
				Self::resolver_sees(&cloudflare, name, expected),
				Self::resolver_sees(&google, name, expected),
			);
			if seen_cloudflare || seen_google {
				return Ok(());
			}
			if Instant::now() >= deadline {
				return Err(Failure::order(format!(
					"the challenge record at {name} had not propagated to public resolvers within \
					 {}s",
					PROPAGATION_TIMEOUT.as_secs()
				)));
			}
			tokio::time::sleep(PROPAGATION_POLL_INTERVAL).await;
		}
	}

	/// The fake authority: publish the challenge record the real path would, sign
	/// with a throwaway root, and clean up after itself. What it exercises is the
	/// worker's handling, not the ACME protocol.
	async fn obtain_fake(
		state: &Arc<Mutex<FakeCa>>,
		dns: &DnsProvider,
		zone: &ManagedZone,
		name: &str,
		profile: Option<&str>,
	) -> AcmeResult<Issued> {
		let (fail, lifetime, renew_after) = {
			let state = state.lock().expect("fake ca lock");
			(state.fail_with.clone(), state.lifetime, state.renew_after)
		};

		// Published and removed the way a real order's would be, so a test reads
		// the same two changes in the same order — including when the order fails.
		let set = RecordSet::challenge(name, "fake-challenge-value");
		dns.upsert(zone, &set).await?;
		let result = match fail {
			Some((fault, message)) => Err(Failure {
				fault,
				message: format!("fake ca: {message}"),
			}),
			None => self_signed(name, lifetime).map_err(Failure::from),
		};
		dns.delete(zone, &set).await?;
		let (chain, not_after) = result?;

		state
			.lock()
			.expect("fake ca lock")
			.issued
			.push(name.to_string());
		Ok(Issued {
			chain,
			not_after,
			profile: profile.map(str::to_string),
			renew_after,
		})
	}
}

/// Ask the authority when it would like this certificate replaced. No answer is
/// not a failure: an authority that publishes no renewal information leaves
/// Canopy to judge from the certificate's own lifetime.
// spec: CRT#renewal
async fn renewal_window(account: &Account, chain: &str) -> Option<Timestamp> {
	let leaf = leaf_der(chain).ok()?;
	let id = CertificateIdentifier::try_from(&leaf).ok()?;
	match account.renewal_info(&id).await {
		// The start of the window: Canopy renews from the moment the authority
		// says it may, rather than waiting for the window to run out.
		Ok((info, _retry_after)) => {
			Timestamp::from_second(info.suggested_window.start.unix_timestamp()).ok()
		}
		Err(instant_acme::Error::Unsupported(_)) => None,
		Err(err) => {
			debug!("the authority published no renewal information: {err}");
			None
		}
	}
}

/// The leaf of a PEM chain, as DER. The leaf is first, and it is the leaf that
/// both revocation and renewal information are about.
fn leaf_der(chain_pem: &str) -> Result<CertificateDer<'static>> {
	CertificateDer::from_pem_slice(chain_pem.as_bytes())
		.map(|der| der.into_owned())
		.map_err(|e| AppError::BadRequest(format!("could not read the certificate chain: {e}")))
}

/// When the leaf of a chain expires, according to the certificate itself.
fn leaf_expiry(chain_pem: &str) -> Result<Timestamp> {
	let leaf = leaf_der(chain_pem)?;
	let (_, cert) = x509_parser::parse_x509_certificate(leaf.as_ref())
		.map_err(|e| AppError::Upstream(format!("could not parse the issued certificate: {e}")))?;
	Timestamp::from_second(cert.validity().not_after.timestamp())
		.map_err(|e| AppError::Upstream(format!("the certificate's expiry is not a time: {e}")))
}

/// Sign a certificate with a throwaway root, for [`Acme::Fake`]. The key is
/// discarded: nothing verifies this chain, and what a test reads from it is the
/// name and the expiry.
fn self_signed(name: &str, lifetime: Duration) -> Result<(String, Timestamp)> {
	use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, PKCS_ECDSA_P256_SHA256};

	let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
		.map_err(|e| AppError::Custom(format!("fake ca keygen: {e}")))?;
	let mut params = CertificateParams::new(vec![name.to_string()])
		.map_err(|e| AppError::Custom(format!("fake ca params: {e}")))?;
	let mut dn = DistinguishedName::new();
	dn.push(DnType::CommonName, name);
	params.distinguished_name = dn;
	let not_before = ::time::OffsetDateTime::now_utc();
	let not_after = not_before + lifetime;
	params.not_before = not_before;
	params.not_after = not_after;
	// The renewal path asks a certificate for its authority key identifier, so
	// the fake's carry one too.
	params.use_authority_key_identifier_extension = true;

	let cert = params
		.self_signed(&key)
		.map_err(|e| AppError::Custom(format!("fake ca sign: {e}")))?;
	let expiry = Timestamp::from_second(not_after.unix_timestamp())
		.map_err(|e| AppError::Custom(format!("fake ca expiry: {e}")))?;
	Ok((cert.pem(), expiry))
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::dns_provider::RecordChange;

	fn zone() -> ManagedZone {
		ManagedZone::parse_list("tamanu.app=Z1", None).expect("zones")[0].clone()
	}

	#[tokio::test]
	async fn the_fake_signs_for_the_name_it_was_asked_for() {
		let acme = Acme::fake();
		let dns = DnsProvider::fake();
		let issued = acme
			.obtain(&dns, &zone(), "a.tamanu.app", b"csr", Some("classic"), None)
			.await
			.expect("obtain");

		assert_eq!(acme.signed(), vec!["a.tamanu.app"]);
		assert_eq!(issued.profile.as_deref(), Some("classic"));
		assert!(issued.not_after > Timestamp::now());
		assert_eq!(
			leaf_expiry(&issued.chain).expect("expiry"),
			issued.not_after
		);
	}

	#[tokio::test]
	async fn the_challenge_record_is_published_and_then_removed() {
		let acme = Acme::fake();
		let dns = DnsProvider::fake();
		acme.obtain(&dns, &zone(), "a.tamanu.app", b"csr", None, None)
			.await
			.expect("obtain");

		let changes = dns.recorded();
		assert_eq!(changes.len(), 2, "published then removed: {changes:?}");
		assert!(matches!(
			&changes[0],
			RecordChange::Upsert { set, .. } if set.name == "_acme-challenge.a.tamanu.app"
		));
		assert!(matches!(
			&changes[1],
			RecordChange::Delete { set, .. } if set.name == "_acme-challenge.a.tamanu.app"
		));
	}

	#[tokio::test]
	async fn a_failing_authority_leaves_no_challenge_record_behind() {
		let acme = Acme::fake();
		acme.fail_with("service unavailable");
		let dns = DnsProvider::fake();
		acme.obtain(&dns, &zone(), "a.tamanu.app", b"csr", None, None)
			.await
			.expect_err("should fail");

		assert!(
			matches!(dns.recorded().last(), Some(RecordChange::Delete { .. })),
			"the last thing done is the cleanup: {:?}",
			dns.recorded()
		);
	}

	#[tokio::test]
	async fn a_short_lifetime_is_honoured_so_renewal_is_testable() {
		let acme = Acme::fake();
		acme.set_lifetime(Duration::from_secs(6 * 24 * 60 * 60));
		let issued = acme
			.obtain(
				&DnsProvider::fake(),
				&zone(),
				"a.tamanu.app",
				b"csr",
				None,
				None,
			)
			.await
			.expect("obtain");
		let a_week_out = Timestamp::now() + jiff::SignedDuration::from_hours(7 * 24);
		assert!(
			issued.not_after < a_week_out,
			"expires {}, which is not the six days asked for",
			issued.not_after
		);
	}

	#[tokio::test]
	async fn renewal_information_is_passed_through_when_the_authority_gives_any() {
		let acme = Acme::fake();
		let at = Timestamp::now() + jiff::SignedDuration::from_hours(24);
		acme.set_renew_after(at);
		let issued = acme
			.obtain(
				&DnsProvider::fake(),
				&zone(),
				"a.tamanu.app",
				b"csr",
				None,
				None,
			)
			.await
			.expect("obtain");
		assert_eq!(issued.renew_after, Some(at));
	}

	#[tokio::test]
	async fn revoking_records_the_chain_it_was_given() {
		let acme = Acme::fake();
		let issued = acme
			.obtain(
				&DnsProvider::fake(),
				&zone(),
				"a.tamanu.app",
				b"csr",
				None,
				None,
			)
			.await
			.expect("obtain");
		acme.revoke(&issued.chain, RevokeFor::KeyCompromise)
			.await
			.expect("revoke");
		assert_eq!(acme.revocations(), vec![issued.chain]);
	}

	#[test]
	fn a_stored_reason_maps_onto_the_authority_s() {
		assert_eq!(
			RevokeFor::from_stored("key_compromise"),
			RevokeFor::KeyCompromise
		);
		assert_eq!(
			RevokeFor::from_stored("cessation_of_operation"),
			RevokeFor::CessationOfOperation
		);
		// An unrecognised reason is not a reason to refuse the revocation.
		assert_eq!(
			RevokeFor::from_stored("something else"),
			RevokeFor::Unspecified
		);
	}
}
