//! What addresses and certificates share about a DNS name (DNS).
//!
//! They are separate features: each has its own declarations (`application_names`
//! for addresses, `application_certificate_names` for certificates), its own
//! undeclared records, and its own denials. What they have in common is the DNS
//! name as the unit both act on, and that one application holds it across the
//! whole fleet whichever kinds it is declared for.
// spec: DNS#declared-dns-names

use commons_errors::AppError;
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use serde::Serialize;
use uuid::Uuid;

use commons_errors::Result;

/// A kind of request about a DNS name: for address records, or for a TLS
/// certificate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DnsNameKind {
	Addresses,
	Certificate,
}

impl DnsNameKind {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Addresses => "addresses",
			Self::Certificate => "certificate",
		}
	}

	/// The kind as a noun phrase, for messages to operators.
	pub fn noun(self) -> &'static str {
		match self {
			Self::Addresses => "addresses",
			Self::Certificate => "certificates",
		}
	}
}

impl TryFrom<String> for DnsNameKind {
	type Error = String;

	fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
		match value.as_str() {
			"addresses" => Ok(Self::Addresses),
			"certificate" => Ok(Self::Certificate),
			_ => Err(format!("unknown DNS name kind {value:?}")),
		}
	}
}

/// The application holding `name` for either kind, if any.
///
/// The two declaration tables never disagree about the holder: a database
/// trigger refuses a row in one for a name the other has for a different
/// application. So both are asked at once and whichever answers is the holder.
pub async fn holder(db: &mut AsyncPgConnection, name: &str) -> Result<Option<Uuid>> {
	use crate::schema::{application_certificate_names, application_names};

	let holders: Vec<Uuid> = application_names::table
		.filter(application_names::name.eq(name))
		.select(application_names::application_id)
		.union_all(
			application_certificate_names::table
				.filter(application_certificate_names::name.eq(name))
				.select(application_certificate_names::application_id),
		)
		.load(db)
		.await
		.map_err(AppError::from)?;
	Ok(holders.into_iter().next())
}

/// The operator-facing refusal for a name another application holds.
///
/// Names the holder — safe here, and not on the device-facing path, because an
/// operator already sees the whole fleet and needs to know what to release
/// first.
pub async fn held_elsewhere(db: &mut AsyncPgConnection, name: &str, holder: Uuid) -> AppError {
	let described = match crate::applications::Application::get_by_id(db, holder).await {
		Ok(app) => match app.name {
			Some(name) => format!("{name} ({holder})"),
			None => holder.to_string(),
		},
		Err(_) => holder.to_string(),
	};
	AppError::Conflict(format!(
		"{name} is already declared by {described}; release it there before declaring it here"
	))
}

/// The refusal for a declaration that lost a race to another application, once
/// the winner is known.
pub async fn lost_race(db: &mut AsyncPgConnection, name: &str) -> AppError {
	match holder(db, name).await {
		Ok(Some(id)) => held_elsewhere(db, name, id).await,
		_ => AppError::Conflict(format!("{name} was declared elsewhere just now")),
	}
}
