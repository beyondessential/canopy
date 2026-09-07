//! Shared vocabulary for the backup-credentials system.
//!
//! These enums are stored as `TEXT` in Postgres and flow through
//! public-server, the `jobs` crate, and the private-web wire types (via the
//! generated `api-types.ts`), so they live here in `commons-types` rather than
//! being re-declared per component. The closed sets (`BackupPurpose`,
//! `RunOutcome`, `MaintenanceKind`, `BackupConfigStatus`) mirror the
//! `Severity`/`ServerKind` pattern and back DB `CHECK (… IN …)` constraints.
//! `BackupType` is deliberately open — bestool registers arbitrary type names
//! and unknown ones land in `Custom`.

use std::{fmt::Display, str::FromStr};

use diesel::{
	backend::Backend,
	deserialize::{self, FromSql, FromSqlRow},
	expression::AsExpression,
	serialize::{self, Output, ToSql},
	sql_types::Text,
};
use serde::{Deserialize, Serialize};

/// Generate a closed string-valued enum stored as Postgres `TEXT`, with the
/// `Display`/`FromStr`/`FromSql`/`ToSql` plumbing the `Severity` pattern uses.
/// The string literal per variant is the single source of truth for the wire
/// form, the `Display` form, and the DB value (which the `CHECK` constraint
/// must match).
macro_rules! text_enum {
	(
		$(#[$meta:meta])*
		$vis:vis enum $name:ident {
			$( $(#[doc = $doc:literal])* $variant:ident = $str:literal ),+ $(,)?
		}
		default = $default:ident;
		error $err:ident = $errmsg:literal;
	) => {
		$(#[$meta])*
		#[derive(
			Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, AsExpression,
			FromSqlRow, utoipa::ToSchema,
		)]
		#[diesel(sql_type = Text)]
		$vis enum $name {
			// `serde(rename)` sets the wire string; `schema(rename)` mirrors it
			// into the utoipa-generated OpenAPI schema, since utoipa v5 doesn't
			// pick up per-variant serde renames on its own (it only honours a
			// container-level `rename_all`). Without the schema rename the
			// generated TS union would carry the PascalCase variant names while
			// the actual JSON is the lowercase string — a wire/type mismatch.
			$( $(#[doc = $doc])* #[serde(rename = $str)] #[schema(rename = $str)] $variant ),+
		}

		#[derive(Debug, Clone, Copy, thiserror::Error)]
		#[error($errmsg)]
		$vis struct $err;

		impl Default for $name {
			fn default() -> Self { Self::$default }
		}

		impl Display for $name {
			fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
				let s = match self { $( Self::$variant => $str ),+ };
				write!(f, "{s}")
			}
		}

		impl FromStr for $name {
			type Err = $err;
			fn from_str(s: &str) -> Result<Self, Self::Err> {
				match s { $( $str => Ok(Self::$variant), )+ _ => Err($err) }
			}
		}

		impl TryFrom<String> for $name {
			type Error = $err;
			fn try_from(value: String) -> Result<Self, Self::Error> { value.parse() }
		}

		impl From<$name> for String {
			fn from(v: $name) -> Self { v.to_string() }
		}

		impl<DB> FromSql<Text, DB> for $name
		where
			DB: Backend,
			String: FromSql<Text, DB>,
		{
			fn from_sql(bytes: DB::RawValue<'_>) -> deserialize::Result<Self> {
				Ok(Self::try_from(String::from_sql(bytes)?)?)
			}
		}

		impl ToSql<Text, diesel::pg::Pg> for $name
		where
			String: ToSql<Text, diesel::pg::Pg>,
		{
			fn to_sql<'b>(&'b self, out: &mut Output<'b, '_, diesel::pg::Pg>) -> serialize::Result {
				let v = String::from(*self);
				<String as ToSql<Text, diesel::pg::Pg>>::to_sql(&v, &mut out.reborrow())
			}
		}
	};
}

text_enum! {
	/// Why a backup credential was issued, or what a reported run was for.
	/// Determines the access the credential grants: a `backup` credential can
	/// write new data but not delete existing data, while a `restore`
	/// credential is read-only.
	pub enum BackupPurpose {
		/// The credential is for writing a new backup, or the run wrote one.
		/// Access is write-only — existing data can't be deleted with it.
		Backup = "backup",
		/// The credential is for reading existing data, or the run read it.
		/// Access is read-only.
		Restore = "restore",
	}
	default = Backup;
	error BackupPurposeFromStringError = "invalid backup purpose; expected one of: backup, restore";
}

text_enum! {
	/// Outcome of a reported backup or restore run.
	pub enum RunOutcome {
		/// The run completed successfully.
		Success = "success",
		/// The run failed.
		Failure = "failure",
	}
	default = Success;
	error RunOutcomeFromStringError = "invalid run outcome; expected one of: success, failure";
}

text_enum! {
	/// How far a replica's masking manifest got before the replica was
	/// served.
	///
	/// A partial redaction is live: the manifest applied, but some of its
	/// columns did not, leaving a mostly-masked replica with an
	/// unidentified remainder in the clear. A failed one is not live at
	/// all — the consumer holds the replica on the data it was already
	/// serving.
	pub enum RedactionOutcome {
		/// Every column the manifest named was masked.
		Complete = "complete",
		/// The manifest applied, but some of its columns did not.
		Partial = "partial",
		/// No masking took effect.
		Failed = "failed",
	}
	default = Failed;
	error RedactionOutcomeFromStringError = "invalid redaction outcome; expected one of: complete, partial, failed";
}

text_enum! {
	/// Which maintenance cycle a reported backup-repository maintenance run
	/// performed.
	pub enum MaintenanceKind {
		/// A lightweight maintenance pass, run frequently.
		Quick = "quick",
		/// A more thorough maintenance pass, run less frequently.
		Full = "full",
	}
	default = Quick;
	error MaintenanceKindFromStringError = "invalid maintenance kind; expected one of: quick, full";
}

text_enum! {
	/// How a group's backup-repository passphrase originated. Either way, the
	/// passphrase is subsequently owned and rotated by Canopy — nobody,
	/// including the operator, keeps a copy of the current passphrase.
	pub enum BackupRepoMode {
		/// Canopy generated the passphrase itself when creating a new
		/// repository.
		FromBirth = "from_birth",
		/// The operator supplied the passphrase of an existing repository to
		/// connect it. Canopy rotates it to a freshly generated passphrase
		/// immediately after connecting.
		Passphrase = "passphrase",
	}
	default = FromBirth;
	error BackupRepoModeFromStringError = "invalid backup repo mode; expected one of: from_birth, passphrase";
}

text_enum! {
	/// Where a group's backup storage lives and who provisioned it. This
	/// distinction has no effect on how backups and restores are performed.
	pub enum BackupPlacement {
		/// The storage bucket and its access roles were provisioned ahead of
		/// time in the group's own cloud account; Canopy only connects
		/// to it. The default.
		External = "external",
		/// Canopy automatically created the storage bucket in a shared
		/// account and issues short-lived credentials scoped to the group
		/// for isolation.
		Shared = "shared",
	}
	default = External;
	error BackupPlacementFromStringError = "invalid backup placement; expected one of: external, shared";
}

text_enum! {
	/// Lifecycle status of a group's backup repository configuration. No
	/// backup or restore operations can run until this reaches `ready`.
	pub enum BackupConfigStatus {
		/// The repository is being created; not yet usable.
		Provisioning = "provisioning",
		/// The repository has been created and is ready for backups and
		/// restores. There is no separate approval step — the passphrase is
		/// owned and rotated by Canopy, and nobody holds a copy of it.
		Ready = "ready",
	}
	default = Provisioning;
	error BackupConfigStatusFromStringError = "invalid backup config status; expected one of: provisioning, ready";
}

/// A named, client-side backup procedure that ends in a type-tagged kopia
/// snapshot (e.g. `tamanu-postgres`). Open by design: bestool advertises
/// whatever types its server can run, and Canopy is type-agnostic for
/// execution, so any unrecognised name is preserved verbatim in `Custom`
/// rather than rejected. Stored as `TEXT`; serializes as a plain string (no
/// DB `CHECK`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, AsExpression, FromSqlRow)]
#[diesel(sql_type = Text)]
pub enum BackupType {
	/// The first well-known type — a quiesced Postgres snapshot (also what
	/// PGRO restores).
	TamanuPostgres,
	/// Any other type name, preserved as advertised.
	Custom(String),
}

impl BackupType {
	const TAMANU_POSTGRES: &'static str = "tamanu-postgres";

	/// The wire/DB string for this type.
	pub fn as_str(&self) -> &str {
		match self {
			Self::TamanuPostgres => Self::TAMANU_POSTGRES,
			Self::Custom(s) => s,
		}
	}
}

impl Display for BackupType {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(self.as_str())
	}
}

impl FromStr for BackupType {
	type Err = std::convert::Infallible;
	fn from_str(s: &str) -> Result<Self, Self::Err> {
		Ok(match s {
			Self::TAMANU_POSTGRES => Self::TamanuPostgres,
			other => Self::Custom(other.to_owned()),
		})
	}
}

impl From<&str> for BackupType {
	fn from(s: &str) -> Self {
		// FromStr is infallible.
		s.parse().unwrap()
	}
}

impl From<String> for BackupType {
	fn from(s: String) -> Self {
		match s.as_str() {
			Self::TAMANU_POSTGRES => Self::TamanuPostgres,
			_ => Self::Custom(s),
		}
	}
}

impl From<BackupType> for String {
	fn from(v: BackupType) -> Self {
		match v {
			BackupType::TamanuPostgres => BackupType::TAMANU_POSTGRES.to_owned(),
			BackupType::Custom(s) => s,
		}
	}
}

impl Serialize for BackupType {
	fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
		s.serialize_str(self.as_str())
	}
}

impl<'de> Deserialize<'de> for BackupType {
	fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
		Ok(Self::from(String::deserialize(d)?))
	}
}

impl<DB> FromSql<Text, DB> for BackupType
where
	DB: Backend,
	String: FromSql<Text, DB>,
{
	fn from_sql(bytes: DB::RawValue<'_>) -> deserialize::Result<Self> {
		Ok(Self::from(String::from_sql(bytes)?))
	}
}

impl ToSql<Text, diesel::pg::Pg> for BackupType
where
	String: ToSql<Text, diesel::pg::Pg>,
{
	fn to_sql<'b>(&'b self, out: &mut Output<'b, '_, diesel::pg::Pg>) -> serialize::Result {
		<str as ToSql<Text, diesel::pg::Pg>>::to_sql(self.as_str(), &mut out.reborrow())
	}
}

/// How a managed restore replica is handled, as defined by the consumer.
/// Fully open: a restore consumer advertises the intents it can satisfy as
/// arbitrary identifiers and Canopy stores and dispatches them verbatim,
/// never branching on any particular value. Stored as `TEXT`; serializes as
/// a plain string (no DB `CHECK`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, AsExpression, FromSqlRow)]
#[diesel(sql_type = Text)]
pub struct RestoreIntent(pub String);

impl RestoreIntent {
	/// The wire/DB string for this intent.
	pub fn as_str(&self) -> &str {
		&self.0
	}
}

impl Display for RestoreIntent {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(self.as_str())
	}
}

impl From<String> for RestoreIntent {
	fn from(s: String) -> Self {
		Self(s)
	}
}

impl From<&str> for RestoreIntent {
	fn from(s: &str) -> Self {
		Self::from(s.to_owned())
	}
}

impl FromStr for RestoreIntent {
	type Err = std::convert::Infallible;
	fn from_str(s: &str) -> Result<Self, Self::Err> {
		Ok(Self::from(s))
	}
}

impl From<RestoreIntent> for String {
	fn from(v: RestoreIntent) -> Self {
		v.0
	}
}

impl Serialize for RestoreIntent {
	fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
		s.serialize_str(self.as_str())
	}
}

impl<'de> Deserialize<'de> for RestoreIntent {
	fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
		Ok(Self::from(String::deserialize(d)?))
	}
}

impl<DB> FromSql<Text, DB> for RestoreIntent
where
	DB: Backend,
	String: FromSql<Text, DB>,
{
	fn from_sql(bytes: DB::RawValue<'_>) -> deserialize::Result<Self> {
		Ok(Self::from(String::from_sql(bytes)?))
	}
}

impl ToSql<Text, diesel::pg::Pg> for RestoreIntent
where
	String: ToSql<Text, diesel::pg::Pg>,
{
	fn to_sql<'b>(&'b self, out: &mut Output<'b, '_, diesel::pg::Pg>) -> serialize::Result {
		<str as ToSql<Text, diesel::pg::Pg>>::to_sql(self.as_str(), &mut out.reborrow())
	}
}

/// Canopy-defined restore semantics: behaviours a consumer opts an intent into.
/// Semantics are carried as plain strings so a consumer may advertise ahead of
/// Canopy support; these are the ones Canopy acts on. Unrecognised semantics are
/// stored and preserved but change no behaviour.
pub mod semantics {
	/// The intent produces restore-health feedback: Canopy expects a report per
	/// replica and holds it to the overdue bound.
	pub const CHECK: &str = "check";
	/// A given snapshot is dispatched to the intent at most once: Canopy omits
	/// the worklist entry once the current snapshot has a healthy report, and
	/// measures overdue against the latest snapshot rather than the clock.
	pub const ONCE: &str = "once";
	/// The intent's health report carries a link to the running replica within
	/// its health data, which Canopy surfaces to operators.
	pub const URL: &str = "url";
	/// The intent applies a Tamanu version's schema migrations to the replica it
	/// restores: Canopy names a target version on the worklist entry, withholds
	/// an entry from a server with no candidate version, and keys `once` to the
	/// snapshot and target version together.
	pub const MIGRATE: &str = "migrate";
	/// The intent can de-identify the restored data before serving it: Canopy
	/// resolves the masking manifest for the server's product into the
	/// worklist entry, withholds an entry from a server whose product has no
	/// manifest, and holds the replica to the redaction outcome reported back.
	pub const REDACT: &str = "redact";
	/// The intent builds a Tamanu reporting schema from the replica it restores
	/// and registers it as a group-scoped artifact: Canopy names the pair's
	/// version on the worklist entry, restores a machine of the group running a
	/// central Tamanu application, and keys `once` to the group and the version
	/// rather than the snapshot.
	// spec: RPT
	pub const REPORTING_SCHEMA: &str = "reporting-schema";
}

/// The parameters Canopy owns on behalf of the `redact` semantic.
///
/// An operator declaring a redacting replica says only that it redacts;
/// where its masking comes from is a property of the server's product, so
/// Canopy resolves these into the worklist itself and never takes an
/// operator's value for them.
// spec: RST#the-masking-manifest
pub mod redaction_params {
	/// `text` — where the masking manifest for a version lives, with
	/// `{version}` substituted by the consumer against the data it restored.
	/// A value here is what turns redaction on for the consumer.
	pub const MANIFEST_URL: &str = "redaction_manifest_url";
	/// `text` — SQL reading the version out of the restored
	/// database, to substitute into the manifest URL.
	pub const VERSION_QUERY: &str = "redaction_version_query";
	/// `boolean` — whether to retry at the `major.minor.0` base version when
	/// the versioned manifest URL 404s.
	pub const VERSION_FALLBACK_TO_BASE: &str = "redaction_version_fallback_to_base";

	/// Every parameter Canopy owns for `redact`, for stripping operator
	/// values and for resolving the worklist's.
	pub const ALL: &[&str] = &[MANIFEST_URL, VERSION_QUERY, VERSION_FALLBACK_TO_BASE];
}

/// The data type of a restore-replica configuration parameter, which
/// determines how its value is validated. `duration` and `bytes` values must
/// be non-negative integers (a count of seconds and of bytes, respectively);
/// `integer` accepts any whole number, positive or negative; `boolean` is a
/// JSON boolean; `text` is a JSON string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ParamType {
	/// A non-negative whole number of seconds.
	Duration,
	/// A non-negative size in bytes.
	Bytes,
	/// A JSON boolean value.
	Boolean,
	/// A JSON whole-number value, positive or negative.
	Integer,
	/// A JSON string value.
	Text,
}

/// Describes one configurable parameter that a replica of a restore intent
/// accepts.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ParamSpec {
	/// The parameter's data type, which determines how its value is
	/// validated.
	#[serde(rename = "type")]
	pub r#type: ParamType,
	/// The value used when the parameter is left unset. `None` means an
	/// unset parameter is sent as JSON `null` rather than a default value.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub default: Option<serde_json::Value>,
}

/// A consumer's parameter schema for one intent: parameter name → spec, ordered
/// for stable output.
pub type ParamSchema = std::collections::BTreeMap<String, ParamSpec>;

/// Operator-supplied parameter values for one replica: parameter name → value.
pub type ParamValues = std::collections::BTreeMap<String, serde_json::Value>;

/// One restore purpose a consumer advertises support for: the behaviours it
/// opts into and the settings it accepts per replica.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct IntentDescriptor {
	/// Name of the intent: an arbitrary identifier chosen by the consumer
	/// (e.g. `verify`); any name may be advertised.
	#[schema(value_type = String)]
	pub intent: RestoreIntent,
	/// Human-readable description of the intent, if provided.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub description: Option<String>,
	/// Behaviours this intent opts into; see [`semantics`] for what each one
	/// grants. Unrecognised values are stored but have no effect, so a consumer
	/// may advertise ahead of Canopy support.
	#[serde(default)]
	pub semantics: Vec<String>,
	/// Configurable parameters this intent accepts per replica, keyed by
	/// parameter name.
	#[serde(default)]
	pub params: ParamSchema,
}

impl IntentDescriptor {
	/// Whether the intent opts into a semantic (see [`semantics`]).
	pub fn has_semantic(&self, semantic: &str) -> bool {
		self.semantics.iter().any(|s| s == semantic)
	}
}

/// A parameter value failed validation against its intent's schema.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ParamValidationError {
	#[error("unknown parameter {0:?}")]
	Unknown(String),
	#[error("parameter {name:?} expects a {expected} value")]
	WrongType {
		name: String,
		expected: &'static str,
	},
	#[error("parameter {name:?}: {error}")]
	Unit {
		name: String,
		#[source]
		error: crate::units::UnitParseError,
	},
}

/// Validate operator-supplied values against a schema. Every value must name a
/// parameter in the schema and match its type; `null` is always allowed. Missing
/// parameters are legal — every parameter is optional.
pub fn validate_params(
	schema: &ParamSchema,
	values: &ParamValues,
) -> Result<(), ParamValidationError> {
	for (name, value) in values {
		let Some(spec) = schema.get(name) else {
			return Err(ParamValidationError::Unknown(name.clone()));
		};
		if value.is_null() {
			continue;
		}
		let (ok, expected) = match spec.r#type {
			ParamType::Boolean => (value.is_boolean(), "boolean"),
			ParamType::Text => (value.is_string(), "text"),
			ParamType::Integer => (value.is_i64() || value.is_u64(), "integer"),
			ParamType::Duration => (
				value.as_i64().is_some_and(|n| n >= 0) || value.is_u64(),
				"duration in whole seconds",
			),
			ParamType::Bytes => (
				value.as_i64().is_some_and(|n| n >= 0) || value.is_u64(),
				"size in bytes",
			),
		};
		if !ok {
			return Err(ParamValidationError::WrongType {
				name: name.clone(),
				expected,
			});
		}
	}
	Ok(())
}

/// Resolve human-unit string values into their raw stored form, per the
/// schema: a string value for a `duration` parameter parses in jiff's
/// "friendly" format (e.g. `2h 30m`) to whole seconds, and one for a `bytes`
/// parameter parses as a 1024-based size (e.g. `20Gi`) to a byte count. Raw
/// integer values pass through unchanged, as do values of other types and
/// values for parameters the schema doesn't describe ([`validate_params`]
/// deals with those separately).
pub fn normalize_params(
	schema: &ParamSchema,
	values: &ParamValues,
) -> Result<ParamValues, ParamValidationError> {
	values
		.iter()
		.map(|(name, value)| {
			let unit_err = |error| ParamValidationError::Unit {
				name: name.clone(),
				error,
			};
			let normalized = match (schema.get(name).map(|s| s.r#type), value.as_str()) {
				(Some(ParamType::Duration), Some(text)) => {
					crate::units::parse_duration_seconds(text)
						.map_err(unit_err)?
						.into()
				}
				(Some(ParamType::Bytes), Some(text)) => {
					crate::units::parse_bytes(text).map_err(unit_err)?.into()
				}
				_ => value.clone(),
			};
			Ok((name.clone(), normalized))
		})
		.collect()
}

/// Format raw stored values as human-unit display strings, per the schema:
/// a non-negative integer value of a `duration` parameter becomes a friendly
/// duration string (e.g. `2h 30m`), and one of a `bytes` parameter becomes a
/// Kubernetes-notation size (e.g. `20Gi`). Everything else passes through
/// unchanged.
pub fn display_params(schema: &ParamSchema, values: &ParamValues) -> ParamValues {
	values
		.iter()
		.map(|(name, value)| {
			let displayed = match (schema.get(name).map(|s| s.r#type), value.as_i64()) {
				(Some(ParamType::Duration), Some(n)) if n >= 0 => {
					crate::units::format_duration_seconds(n).into()
				}
				(Some(ParamType::Bytes), Some(n)) if n >= 0 => crate::units::format_bytes(n).into(),
				_ => value.clone(),
			};
			(name.clone(), displayed)
		})
		.collect()
}

/// Format a schema's `duration`/`bytes` defaults as human-unit display
/// strings, for operator-facing output; everything else passes through
/// unchanged.
pub fn display_param_defaults(schema: &ParamSchema) -> ParamSchema {
	schema
		.iter()
		.map(|(name, spec)| {
			let raw = spec.default.as_ref().and_then(|d| d.as_i64());
			let default = match (spec.r#type, raw) {
				(ParamType::Duration, Some(n)) if n >= 0 => {
					Some(crate::units::format_duration_seconds(n).into())
				}
				(ParamType::Bytes, Some(n)) if n >= 0 => Some(crate::units::format_bytes(n).into()),
				_ => spec.default.clone(),
			};
			(
				name.clone(),
				ParamSpec {
					r#type: spec.r#type,
					default,
				},
			)
		})
		.collect()
}

/// Resolve the values to send in the worklist: one entry per parameter the
/// intent advertises. A set (non-null) value wins; otherwise the parameter's
/// default, or JSON `null` when it has none. Stored values for parameters the
/// intent no longer advertises are dropped.
pub fn resolve_params(schema: &ParamSchema, values: &ParamValues) -> ParamValues {
	schema
		.iter()
		.map(|(name, spec)| {
			let resolved = values
				.get(name)
				.filter(|v| !v.is_null())
				.cloned()
				.or_else(|| spec.default.clone())
				.unwrap_or(serde_json::Value::Null);
			(name.clone(), resolved)
		})
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn closed_enum_roundtrips_match_db_strings() {
		assert_eq!(BackupPurpose::Backup.to_string(), "backup");
		assert_eq!(
			"restore".parse::<BackupPurpose>().unwrap(),
			BackupPurpose::Restore
		);
		assert_eq!(BackupConfigStatus::Ready.to_string(), "ready");
		assert!("nope".parse::<RunOutcome>().is_err());
	}

	#[test]
	fn closed_enum_serde_is_the_db_string() {
		assert_eq!(
			serde_json::to_string(&MaintenanceKind::Full).unwrap(),
			"\"full\""
		);
		assert_eq!(
			serde_json::from_str::<BackupConfigStatus>("\"provisioning\"").unwrap(),
			BackupConfigStatus::Provisioning,
		);
	}

	#[test]
	fn backup_type_known_and_custom() {
		assert_eq!(
			BackupType::from("tamanu-postgres"),
			BackupType::TamanuPostgres
		);
		assert_eq!(
			BackupType::from("weird"),
			BackupType::Custom("weird".into())
		);
		assert_eq!(BackupType::TamanuPostgres.to_string(), "tamanu-postgres");
		// Round-trips through the wire as a plain string.
		assert_eq!(
			serde_json::to_string(&BackupType::TamanuPostgres).unwrap(),
			"\"tamanu-postgres\""
		);
		assert_eq!(
			serde_json::from_str::<BackupType>("\"custom-x\"").unwrap(),
			BackupType::Custom("custom-x".into()),
		);
	}

	#[test]
	fn restore_intent_is_an_open_string() {
		assert_eq!(RestoreIntent::from("verify").to_string(), "verify");
		assert_eq!(
			RestoreIntent::from("anything-goes").as_str(),
			"anything-goes"
		);
		// Round-trips through the wire as a plain string.
		assert_eq!(
			serde_json::to_string(&RestoreIntent::from("verify")).unwrap(),
			"\"verify\""
		);
		assert_eq!(
			serde_json::from_str::<RestoreIntent>("\"custom-x\"").unwrap(),
			RestoreIntent::from("custom-x"),
		);
	}

	fn schema() -> ParamSchema {
		serde_json::from_value(serde_json::json!({
			"minimum_uptime": {"type": "duration", "default": 7200},
			"max_size": {"type": "bytes"},
			"anonymisation": {"type": "boolean", "default": true},
		}))
		.unwrap()
	}

	#[test]
	fn param_type_serializes_snake_case() {
		assert_eq!(
			serde_json::to_string(&ParamType::Duration).unwrap(),
			"\"duration\""
		);
		assert_eq!(
			serde_json::from_str::<ParamType>("\"boolean\"").unwrap(),
			ParamType::Boolean
		);
	}

	#[test]
	fn normalize_params_resolves_unit_strings_to_raw_values() {
		let values: ParamValues = serde_json::from_value(serde_json::json!({
			"minimum_uptime": "2h 30m",
			"max_size": "20G",
			"anonymisation": true,
		}))
		.unwrap();
		let normalized = normalize_params(&schema(), &values).unwrap();
		assert_eq!(normalized["minimum_uptime"], serde_json::json!(9000));
		assert_eq!(normalized["max_size"], serde_json::json!(20i64 << 30));
		assert_eq!(normalized["anonymisation"], serde_json::json!(true));
		assert!(validate_params(&schema(), &normalized).is_ok());
	}

	#[test]
	fn normalize_params_passes_raw_integers_and_unknown_names_through() {
		let values: ParamValues = serde_json::from_value(serde_json::json!({
			"minimum_uptime": 3600,
			"not_in_schema": "20G",
		}))
		.unwrap();
		let normalized = normalize_params(&schema(), &values).unwrap();
		assert_eq!(normalized["minimum_uptime"], serde_json::json!(3600));
		assert_eq!(normalized["not_in_schema"], serde_json::json!("20G"));
	}

	#[test]
	fn normalize_params_rejects_bad_unit_strings() {
		for (name, value) in [
			("minimum_uptime", "banana"),
			("minimum_uptime", "1mo"),
			("minimum_uptime", "-1h"),
			("max_size", "20T"),
			("max_size", "lots"),
		] {
			let values: ParamValues =
				serde_json::from_value(serde_json::json!({ name: value })).unwrap();
			let err = normalize_params(&schema(), &values).unwrap_err();
			assert!(
				matches!(err, ParamValidationError::Unit { .. }),
				"{name}={value:?} gave {err:?}"
			);
		}
	}

	#[test]
	fn display_params_formats_raw_values_as_unit_strings() {
		let values: ParamValues = serde_json::from_value(serde_json::json!({
			"minimum_uptime": 9000,
			"max_size": 20i64 << 30,
			"anonymisation": true,
		}))
		.unwrap();
		let displayed = display_params(&schema(), &values);
		assert_eq!(displayed["minimum_uptime"], serde_json::json!("2h 30m"));
		assert_eq!(displayed["max_size"], serde_json::json!("20Gi"));
		assert_eq!(displayed["anonymisation"], serde_json::json!(true));
		// Displayed values normalize straight back to the raw ones.
		let normalized = normalize_params(&schema(), &displayed).unwrap();
		assert_eq!(normalized["minimum_uptime"], serde_json::json!(9000));
		assert_eq!(normalized["max_size"], serde_json::json!(20i64 << 30));
	}

	#[test]
	fn display_param_defaults_formats_duration_and_bytes_defaults() {
		let displayed = display_param_defaults(&schema());
		assert_eq!(
			displayed["minimum_uptime"].default,
			Some(serde_json::json!("2h"))
		);
		assert_eq!(displayed["max_size"].default, None);
		assert_eq!(
			displayed["anonymisation"].default,
			Some(serde_json::json!(true))
		);
	}

	#[test]
	fn validate_params_accepts_matching_and_null() {
		let values: ParamValues = serde_json::from_value(serde_json::json!({
			"minimum_uptime": 3600,
			"max_size": serde_json::Value::Null,
			"anonymisation": false,
		}))
		.unwrap();
		assert!(validate_params(&schema(), &values).is_ok());
		// No values at all is legal — every parameter is optional.
		assert!(validate_params(&schema(), &ParamValues::new()).is_ok());
	}

	#[test]
	fn validate_params_rejects_unknown_and_wrong_type() {
		let unknown: ParamValues = serde_json::from_value(serde_json::json!({"nope": 1})).unwrap();
		assert!(matches!(
			validate_params(&schema(), &unknown),
			Err(ParamValidationError::Unknown(_))
		));
		let wrong: ParamValues =
			serde_json::from_value(serde_json::json!({"anonymisation": "yes"})).unwrap();
		assert!(matches!(
			validate_params(&schema(), &wrong),
			Err(ParamValidationError::WrongType { .. })
		));
		// A negative duration is not a valid whole-seconds value.
		let negative: ParamValues =
			serde_json::from_value(serde_json::json!({"minimum_uptime": -5})).unwrap();
		assert!(validate_params(&schema(), &negative).is_err());
	}

	#[test]
	fn resolve_params_fills_defaults_and_nulls() {
		let values: ParamValues =
			serde_json::from_value(serde_json::json!({"anonymisation": false})).unwrap();
		let resolved = resolve_params(&schema(), &values);
		// Set value wins.
		assert_eq!(resolved["anonymisation"], serde_json::json!(false));
		// Unset-with-default → default.
		assert_eq!(resolved["minimum_uptime"], serde_json::json!(7200));
		// Unset-without-default → null.
		assert_eq!(resolved["max_size"], serde_json::Value::Null);
		// Only advertised parameters appear.
		assert_eq!(resolved.len(), 3);
	}
}
