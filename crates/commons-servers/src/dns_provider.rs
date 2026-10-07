//! Writing records into the DNS zones Canopy manages.
//!
//! Canopy holds the zone access no server holds, and uses it for two things: the
//! address records that make a group's names resolve, and the challenge records
//! that prove control of a name to a certificate authority.
//!
//! Zones are shared, so every write here is scoped to one record set at one
//! name: Canopy replaces and removes the sets it manages and never touches
//! anything else in the zone.
// spec: CRT

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use aws_config::{BehaviorVersion, SdkConfig, sts::AssumeRoleProvider};
use aws_sdk_route53::types::{
	Change, ChangeAction, ChangeBatch, ResourceRecord, ResourceRecordSet,
};
use commons_errors::{AppError, Result};
use commons_types::dns::ManagedZone;

/// How long a record Canopy publishes is cacheable. Short, because both kinds
/// change on Canopy's schedule rather than a human's: an address record follows
/// a server that may move, and a challenge record is wanted for one minute and
/// then never again.
const TTL_SECONDS: i64 = 60;

/// The record kinds Canopy publishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecordKind {
	A,
	Aaaa,
	Txt,
}

impl RecordKind {
	fn as_route53(self) -> aws_sdk_route53::types::RrType {
		match self {
			Self::A => aws_sdk_route53::types::RrType::A,
			Self::Aaaa => aws_sdk_route53::types::RrType::Aaaa,
			Self::Txt => aws_sdk_route53::types::RrType::Txt,
		}
	}

	pub fn as_str(self) -> &'static str {
		match self {
			Self::A => "A",
			Self::Aaaa => "AAAA",
			Self::Txt => "TXT",
		}
	}
}

/// One record set: every value Canopy publishes at a name for one kind. A write
/// replaces the set wholesale, which is how a server changing address ends up
/// with only its current addresses published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordSet {
	pub name: String,
	pub kind: RecordKind,
	pub values: Vec<String>,
}

impl RecordSet {
	/// The address records for a name, split by family. Returns no set for a
	/// family the server reported no address in, so an IPv4-only server
	/// publishes no AAAA.
	pub fn addresses(name: &str, addresses: &[IpAddr]) -> Vec<Self> {
		let mut sets = Vec::with_capacity(2);
		let v4: Vec<String> = addresses
			.iter()
			.filter(|a| a.is_ipv4())
			.map(|a| a.to_string())
			.collect();
		let v6: Vec<String> = addresses
			.iter()
			.filter(|a| a.is_ipv6())
			.map(|a| a.to_string())
			.collect();
		if !v4.is_empty() {
			sets.push(Self {
				name: name.to_string(),
				kind: RecordKind::A,
				values: v4,
			});
		}
		if !v6.is_empty() {
			sets.push(Self {
				name: name.to_string(),
				kind: RecordKind::Aaaa,
				values: v6,
			});
		}
		sets
	}

	/// The DNS-01 challenge record for a name: a TXT at the `_acme-challenge`
	/// label holding the authority's expected value.
	pub fn challenge(name: &str, value: &str) -> Self {
		Self {
			name: format!("_acme-challenge.{name}"),
			kind: RecordKind::Txt,
			// TXT values go on the wire quoted; Route 53 wants the quotes in the
			// value it is given.
			values: vec![format!("\"{value}\"")],
		}
	}
}

/// A change Canopy made, as recorded by [`DnsProvider::Fake`] for tests to
/// assert on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordChange {
	Upsert { zone: String, set: RecordSet },
	Delete { zone: String, set: RecordSet },
}

/// Where Canopy's record writes go: Route 53 in a real run, or an in-memory log
/// in tests and the e2e binary.
#[derive(Clone)]
pub enum DnsProvider {
	Aws(Box<SdkConfig>),
	/// Records every change instead of making it, and can be told to fail so the
	/// retry and alerting paths are exercisable.
	Fake(Arc<Mutex<FakeDns>>),
}

/// The state behind [`DnsProvider::Fake`].
#[derive(Debug, Default)]
pub struct FakeDns {
	pub changes: Vec<RecordChange>,
	/// When set, every write fails with this message.
	pub fail_with: Option<String>,
}

impl std::fmt::Debug for DnsProvider {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Aws(_) => f.write_str("DnsProvider::Aws"),
			Self::Fake(_) => f.write_str("DnsProvider::Fake"),
		}
	}
}

impl DnsProvider {
	/// Build from the ambient AWS identity (the pod's IRSA role). Per-zone roles
	/// are assumed from it as each write is made.
	pub async fn aws() -> Self {
		Self::Aws(Box::new(
			aws_config::load_defaults(BehaviorVersion::latest()).await,
		))
	}

	/// A provider that records changes rather than making them.
	pub fn fake() -> Self {
		Self::Fake(Arc::new(Mutex::new(FakeDns::default())))
	}

	/// The changes a [`DnsProvider::Fake`] has been asked to make, oldest first.
	/// Empty for the AWS provider, which does not keep a log.
	pub fn recorded(&self) -> Vec<RecordChange> {
		match self {
			Self::Aws(_) => Vec::new(),
			Self::Fake(state) => state.lock().expect("fake dns lock").changes.clone(),
		}
	}

	/// Make a [`DnsProvider::Fake`] fail every write, to exercise retry and
	/// alerting. No effect on the AWS provider.
	pub fn fail_with(&self, message: impl Into<String>) {
		if let Self::Fake(state) = self {
			state.lock().expect("fake dns lock").fail_with = Some(message.into());
		}
	}

	/// Publish `set` at its name, replacing whatever Canopy had there before.
	pub async fn upsert(&self, zone: &ManagedZone, set: &RecordSet) -> Result<()> {
		self.change(zone, set, ChangeAction::Upsert).await
	}

	/// Remove a set Canopy published. Removing one that isn't there succeeds:
	/// the caller wanted it gone, and it is.
	pub async fn delete(&self, zone: &ManagedZone, set: &RecordSet) -> Result<()> {
		self.change(zone, set, ChangeAction::Delete).await
	}

	async fn change(
		&self,
		zone: &ManagedZone,
		set: &RecordSet,
		action: ChangeAction,
	) -> Result<()> {
		if set.values.is_empty() {
			return Err(AppError::BadRequest(format!(
				"refusing to write an empty {} record set at {}",
				set.kind.as_str(),
				set.name
			)));
		}

		match self {
			Self::Fake(state) => {
				let mut state = state.lock().expect("fake dns lock");
				if let Some(message) = &state.fail_with {
					return Err(AppError::Upstream(format!("fake dns: {message}")));
				}
				let change = match action {
					ChangeAction::Delete => RecordChange::Delete {
						zone: zone.apex.clone(),
						set: set.clone(),
					},
					_ => RecordChange::Upsert {
						zone: zone.apex.clone(),
						set: set.clone(),
					},
				};
				state.changes.push(change);
				Ok(())
			}
			Self::Aws(config) => {
				let client = self.route53_for(zone, config).await?;
				let records: Vec<ResourceRecord> = set
					.values
					.iter()
					.map(|value| {
						ResourceRecord::builder()
							.value(value)
							.build()
							.map_err(|e| AppError::Upstream(format!("route53 record: {e}")))
					})
					.collect::<Result<Vec<_>>>()?;

				let record_set = ResourceRecordSet::builder()
					.name(&set.name)
					.r#type(set.kind.as_route53())
					.ttl(TTL_SECONDS)
					.set_resource_records(Some(records))
					.build()
					.map_err(|e| AppError::Upstream(format!("route53 record set: {e}")))?;

				let batch = ChangeBatch::builder()
					.changes(
						Change::builder()
							.action(action)
							.resource_record_set(record_set)
							.build()
							.map_err(|e| AppError::Upstream(format!("route53 change: {e}")))?,
					)
					.build()
					.map_err(|e| AppError::Upstream(format!("route53 change batch: {e}")))?;

				let result = client
					.change_resource_record_sets()
					.hosted_zone_id(&zone.provider_zone_id)
					.change_batch(batch)
					.send()
					.await;

				match result {
					Ok(_) => Ok(()),
					// Deleting a set that isn't there is the state the caller
					// asked for, so it isn't a failure.
					Err(e) if is_absent(&e) => Ok(()),
					Err(e) => Err(AppError::Upstream(format!(
						"route53 could not change {} {} in zone {}: {}",
						set.kind.as_str(),
						set.name,
						zone.apex,
						aws_error_message(&e),
					))),
				}
			}
		}
	}

	/// A Route 53 client for a zone, assuming the zone's role when it names one
	/// (the zone living in another account than Canopy's own).
	async fn route53_for(
		&self,
		zone: &ManagedZone,
		config: &SdkConfig,
	) -> Result<aws_sdk_route53::Client> {
		let Some(role_arn) = &zone.role_arn else {
			return Ok(aws_sdk_route53::Client::new(config));
		};

		let provider = AssumeRoleProvider::builder(role_arn)
			.session_name("canopy-domains")
			.configure(config)
			.build()
			.await;
		let scoped = config
			.to_builder()
			.credentials_provider(
				aws_credential_types::provider::SharedCredentialsProvider::new(provider),
			)
			.build();
		Ok(aws_sdk_route53::Client::new(&scoped))
	}
}

/// Whether a Route 53 error is "the thing you asked me to remove isn't here".
fn is_absent<E: std::fmt::Debug>(error: &E) -> bool {
	let rendered = format!("{error:?}");
	rendered.contains("InvalidChangeBatch") && rendered.contains("not found")
}

/// The most specific message an AWS SDK error carries, for an operator to read.
fn aws_error_message<E: std::fmt::Debug>(error: &E) -> String {
	format!("{error:?}")
}

/// Group record sets by the zone that covers their name, dropping any name no
/// configured zone covers — Canopy cannot write those, and says so elsewhere
/// rather than failing the whole batch here.
pub fn by_zone<'z>(
	sets: Vec<RecordSet>,
	zones: &'z [ManagedZone],
) -> HashMap<&'z str, Vec<RecordSet>> {
	let mut out: HashMap<&str, Vec<RecordSet>> = HashMap::new();
	for set in sets {
		// A challenge record sits under `_acme-challenge`, which is inside the
		// same zone as the name it proves, so match on the name it is for.
		let subject = set
			.name
			.strip_prefix("_acme-challenge.")
			.unwrap_or(&set.name)
			.to_string();
		if let Some(zone) = commons_types::dns::match_zone(&subject, zones) {
			out.entry(zone.apex.as_str()).or_default().push(set);
		}
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	fn zones() -> Vec<ManagedZone> {
		ManagedZone::parse_list("tamanu.app=Z1, demo.tamanu.app=Z2", None).expect("zones")
	}

	#[test]
	fn addresses_split_by_family() {
		let sets = RecordSet::addresses(
			"a.tamanu.app",
			&[
				"192.0.2.1".parse().unwrap(),
				"2001:db8::1".parse().unwrap(),
				"192.0.2.2".parse().unwrap(),
			],
		);
		assert_eq!(sets.len(), 2);
		assert_eq!(sets[0].kind, RecordKind::A);
		assert_eq!(sets[0].values, vec!["192.0.2.1", "192.0.2.2"]);
		assert_eq!(sets[1].kind, RecordKind::Aaaa);
		assert_eq!(sets[1].values, vec!["2001:db8::1"]);
	}

	#[test]
	fn a_family_with_no_address_gets_no_set() {
		let sets = RecordSet::addresses("a.tamanu.app", &["192.0.2.1".parse().unwrap()]);
		assert_eq!(sets.len(), 1);
		assert_eq!(sets[0].kind, RecordKind::A);
	}

	#[test]
	fn a_challenge_sits_under_the_name_it_proves() {
		let set = RecordSet::challenge("a.tamanu.app", "token-value");
		assert_eq!(set.name, "_acme-challenge.a.tamanu.app");
		assert_eq!(set.kind, RecordKind::Txt);
		assert_eq!(set.values, vec!["\"token-value\""]);
	}

	#[test]
	fn challenges_route_to_the_zone_of_the_name_they_prove() {
		let zones = zones();
		let grouped = by_zone(
			vec![
				RecordSet::challenge("a.demo.tamanu.app", "one"),
				RecordSet::challenge("a.prod.tamanu.app", "two"),
			],
			&zones,
		);
		assert_eq!(grouped["demo.tamanu.app"].len(), 1);
		assert_eq!(grouped["tamanu.app"].len(), 1);
	}

	#[test]
	fn a_name_no_zone_covers_is_dropped() {
		let zones = zones();
		let grouped = by_zone(
			vec![RecordSet {
				name: "a.senaite.app".into(),
				kind: RecordKind::A,
				values: vec!["192.0.2.1".into()],
			}],
			&zones,
		);
		assert!(grouped.is_empty());
	}

	#[tokio::test]
	async fn the_fake_records_what_it_was_asked_to_do() {
		let dns = DnsProvider::fake();
		let zone = &zones()[0];
		let set = RecordSet::challenge("a.tamanu.app", "v");
		dns.upsert(zone, &set).await.expect("upsert");
		dns.delete(zone, &set).await.expect("delete");

		assert_eq!(
			dns.recorded(),
			vec![
				RecordChange::Upsert {
					zone: "tamanu.app".into(),
					set: set.clone()
				},
				RecordChange::Delete {
					zone: "tamanu.app".into(),
					set
				},
			]
		);
	}

	#[tokio::test]
	async fn the_fake_can_be_made_to_fail() {
		let dns = DnsProvider::fake();
		dns.fail_with("route53 is having a day");
		let err = dns
			.upsert(&zones()[0], &RecordSet::challenge("a.tamanu.app", "v"))
			.await
			.expect_err("should fail");
		assert!(matches!(err, AppError::Upstream(_)), "got {err:?}");
	}

	#[tokio::test]
	async fn an_empty_set_is_refused_rather_than_written() {
		let dns = DnsProvider::fake();
		dns.upsert(
			&zones()[0],
			&RecordSet {
				name: "a.tamanu.app".into(),
				kind: RecordKind::A,
				values: vec![],
			},
		)
		.await
		.expect_err("an empty set would mean something different to every provider");
	}
}
