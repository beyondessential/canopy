//! The safety-mode ladder shared between the session's current mode and the
//! grade a handler requires (see the SAFE spec).
//!
//! One type serves both roles: a session runs in a mode, a handler is graded to
//! the mode it requires, and a request is permitted when the session's mode is
//! at least the handler's grade. The variants are declared low to high so the
//! derived `Ord` *is* that "at least" comparison — `session_mode >= grade`.

use std::{fmt::Display, str::FromStr};

use diesel::{
	backend::Backend,
	deserialize::{self, FromSql, FromSqlRow},
	expression::AsExpression,
	serialize::{self, Output, ToSql},
	sql_types::Text,
};
use serde::{Deserialize, Serialize};

/// A rung of the safety-mode ladder: the mode a session is in, or the grade a
/// handler requires. Ordered read-only < write < danger, so `>=` answers
/// "does this session reach this grade?".
#[derive(
	Debug,
	Clone,
	Copy,
	Default,
	PartialOrd,
	Ord,
	PartialEq,
	Eq,
	Hash,
	Serialize,
	Deserialize,
	AsExpression,
	FromSqlRow,
	utoipa::ToSchema,
)]
#[diesel(sql_type = Text)]
#[serde(rename_all = "kebab-case")]
pub enum SafetyMode {
	/// Reads only. Every session begins here and returns here on its own; a
	/// read-only request needs no session at all.
	#[default]
	ReadOnly,
	/// Ordinary changes to Canopy's own records.
	Write,
	/// The most consequential operations, additionally gated by the danger
	/// permission (see the ADM spec).
	Danger,
}

impl SafetyMode {
	/// True when a session in this mode may make a request graded to `required`.
	/// The ladder holds downwards: danger reaches write and read-only work.
	pub fn permits(self, required: SafetyMode) -> bool {
		self >= required
	}
}

impl Display for SafetyMode {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(match self {
			SafetyMode::ReadOnly => "read-only",
			SafetyMode::Write => "write",
			SafetyMode::Danger => "danger",
		})
	}
}

#[derive(Debug, Clone, Copy)]
pub struct SafetyModeFromStringError;
impl std::error::Error for SafetyModeFromStringError {}
impl Display for SafetyModeFromStringError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "invalid safety mode")
	}
}

impl FromStr for SafetyMode {
	type Err = SafetyModeFromStringError;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		match s.trim().to_ascii_lowercase().as_str() {
			"read-only" | "readonly" | "read_only" => Ok(Self::ReadOnly),
			"write" => Ok(Self::Write),
			"danger" => Ok(Self::Danger),
			_ => Err(SafetyModeFromStringError),
		}
	}
}

impl TryFrom<String> for SafetyMode {
	type Error = SafetyModeFromStringError;

	fn try_from(value: String) -> Result<Self, Self::Error> {
		value.parse()
	}
}

impl From<SafetyMode> for String {
	fn from(mode: SafetyMode) -> Self {
		mode.to_string()
	}
}

impl<DB> FromSql<Text, DB> for SafetyMode
where
	DB: Backend,
	String: FromSql<Text, DB>,
{
	fn from_sql(bytes: DB::RawValue<'_>) -> deserialize::Result<Self> {
		let s = String::from_sql(bytes)?;
		SafetyMode::try_from(s.clone()).map_err(|_| format!("Unrecognized variant {s}").into())
	}
}

impl ToSql<Text, diesel::pg::Pg> for SafetyMode
where
	String: ToSql<Text, diesel::pg::Pg>,
{
	fn to_sql<'b>(&'b self, out: &mut Output<'b, '_, diesel::pg::Pg>) -> serialize::Result {
		let v = String::from(*self);
		<String as ToSql<Text, diesel::pg::Pg>>::to_sql(&v, &mut out.reborrow())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_ladder_holds_downwards() {
		assert!(SafetyMode::Danger.permits(SafetyMode::Write));
		assert!(SafetyMode::Danger.permits(SafetyMode::ReadOnly));
		assert!(SafetyMode::Write.permits(SafetyMode::ReadOnly));
		assert!(SafetyMode::Write.permits(SafetyMode::Write));
		assert!(!SafetyMode::Write.permits(SafetyMode::Danger));
		assert!(!SafetyMode::ReadOnly.permits(SafetyMode::Write));
		assert!(!SafetyMode::ReadOnly.permits(SafetyMode::Danger));
	}

	#[test]
	fn default_is_read_only() {
		assert_eq!(SafetyMode::default(), SafetyMode::ReadOnly);
	}

	#[test]
	fn wire_and_parse_round_trip() {
		for (mode, wire) in [
			(SafetyMode::ReadOnly, "read-only"),
			(SafetyMode::Write, "write"),
			(SafetyMode::Danger, "danger"),
		] {
			assert_eq!(mode.to_string(), wire);
			assert_eq!(wire.parse::<SafetyMode>().unwrap(), mode);
			assert_eq!(
				serde_json::to_value(mode).unwrap(),
				serde_json::Value::String(wire.to_string())
			);
		}
	}
}
