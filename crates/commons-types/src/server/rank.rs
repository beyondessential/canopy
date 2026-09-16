use std::{fmt::Display, str::FromStr};

use diesel::{
	backend::Backend,
	deserialize::{self, FromSql, FromSqlRow},
	expression::AsExpression,
	serialize::{self, Output, ToSql},
	sql_types::Text,
};
use serde::{Deserialize, Serialize};

/// The environment tier of a server, from `production` down to `dev`.
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
#[serde(rename_all = "lowercase")]
pub enum ServerRank {
	/// A live environment serving real users and real data.
	Production,
	/// A copy of a production environment, typically refreshed from it.
	Clone,
	/// A demonstration environment with sample data.
	Demo,
	/// A testing environment.
	Test,
	/// A development environment. The default when no rank is set.
	#[default]
	Dev,
}

impl Display for ServerRank {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			ServerRank::Production => write!(f, "production"),
			ServerRank::Clone => write!(f, "clone"),
			ServerRank::Demo => write!(f, "demo"),
			ServerRank::Test => write!(f, "test"),
			ServerRank::Dev => write!(f, "dev"),
		}
	}
}

#[derive(Debug, Clone, Copy)]
pub struct ServerRankFromStringError;
impl std::error::Error for ServerRankFromStringError {}
impl Display for ServerRankFromStringError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "invalid server rank")
	}
}

impl TryFrom<String> for ServerRank {
	type Error = ServerRankFromStringError;

	fn try_from(value: String) -> Result<Self, Self::Error> {
		value.parse()
	}
}

impl From<ServerRank> for String {
	fn from(rank: ServerRank) -> Self {
		rank.to_string()
	}
}

impl FromStr for ServerRank {
	type Err = ServerRankFromStringError;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		match s.to_ascii_lowercase().as_ref() {
			"live" | "prod" | "production" => Ok(Self::Production),
			"clone" | "staging" => Ok(Self::Clone),
			"demo" => Ok(Self::Demo),
			"test" => Ok(Self::Test),
			"dev" => Ok(Self::Dev),
			_ => Err(ServerRankFromStringError),
		}
	}
}

impl<DB> FromSql<Text, DB> for ServerRank
where
	DB: Backend,
	String: FromSql<Text, DB>,
{
	fn from_sql(bytes: DB::RawValue<'_>) -> deserialize::Result<Self> {
		let s = String::from_sql(bytes)?;
		ServerRank::try_from(s.clone()).map_err(|_| format!("Unrecognized variant {}", s).into())
	}
}

impl ToSql<Text, diesel::pg::Pg> for ServerRank
where
	String: ToSql<Text, diesel::pg::Pg>,
{
	fn to_sql<'b>(&'b self, out: &mut Output<'b, '_, diesel::pg::Pg>) -> serialize::Result {
		let v = String::from(*self);
		<String as ToSql<Text, diesel::pg::Pg>>::to_sql(&v, &mut out.reborrow())
	}
}
