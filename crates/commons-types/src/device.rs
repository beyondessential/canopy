use diesel::{
	backend::Backend,
	deserialize::{self, FromSql},
	expression::AsExpression,
	serialize::{self, Output, ToSql},
	sql_types::Text,
};
use serde::{Deserialize, Serialize};

/// The role a device is trusted with, which determines what it may do.
#[derive(
	Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, AsExpression, utoipa::ToSchema,
)]
#[diesel(sql_type = Text)]
#[serde(rename_all = "lowercase")]
pub enum DeviceRole {
	/// Full administrative access, including everything the other roles can do.
	Admin,
	/// May publish release versions and register their artifacts.
	Releaser,
	/// Acts as a monitored box: submits statuses and events, runs backups.
	///
	/// An identity belongs to a machine rather than to the software on it, so
	/// this is a box's role. Accepted as `server` on input, which is what an
	/// agent deployed before the rename sends.
	// spec: DTR
	#[serde(alias = "server")]
	Machine,
	/// May run managed restores of backups onto replica servers.
	#[serde(rename = "backup-restore")]
	BackupRestore,
	/// A relay canopy runs in a Kubernetes cluster, monitoring that cluster on
	/// canopy's behalf. Belongs to no machine: it reports the checks of the
	/// applications in its cluster rather than of anything it runs on.
	// spec: DTR
	Relay,
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("invalid device role")]
pub struct DeviceRoleFromStringError;

impl std::str::FromStr for DeviceRole {
	type Err = DeviceRoleFromStringError;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		match s.to_ascii_lowercase().as_ref() {
			"admin" => Ok(Self::Admin),
			"releaser" => Ok(Self::Releaser),
			// `server` is the name this role had when a box and the software on
			// it were one record. Accepted so an agent deployed before the
			// rename keeps enrolling; what Canopy stores and presents is
			// `machine`, so the two do not both travel through the code.
			// spec: DTR
			"machine" | "server" => Ok(Self::Machine),
			"backup-restore" => Ok(Self::BackupRestore),
			// spec: DTR
			"relay" => Ok(Self::Relay),
			_ => Err(DeviceRoleFromStringError),
		}
	}
}

impl TryFrom<String> for DeviceRole {
	type Error = DeviceRoleFromStringError;

	fn try_from(value: String) -> Result<Self, Self::Error> {
		value.parse()
	}
}

impl std::fmt::Display for DeviceRole {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		let s = match self {
			DeviceRole::Admin => "admin",
			DeviceRole::Releaser => "releaser",
			DeviceRole::Machine => "machine",
			DeviceRole::BackupRestore => "backup-restore",
			DeviceRole::Relay => "relay",
		};
		write!(f, "{}", s)
	}
}

impl From<DeviceRole> for String {
	fn from(role: DeviceRole) -> Self {
		role.to_string()
	}
}

impl<DB> FromSql<Text, DB> for DeviceRole
where
	DB: Backend,
	String: FromSql<Text, DB>,
{
	fn from_sql(bytes: DB::RawValue<'_>) -> deserialize::Result<Self> {
		let s = String::from_sql(bytes)?;
		Ok(DeviceRole::try_from(s)?)
	}
}

impl ToSql<Text, diesel::pg::Pg> for DeviceRole
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

	/// `server` was this role's name when a box and the software on it were one
	/// record. It is accepted on input so an agent deployed before the rename
	/// keeps enrolling, and a row written before it still reads.
	// spec: DTR
	#[test]
	fn server_is_accepted_as_the_machine_role() {
		assert_eq!("server".parse::<DeviceRole>().unwrap(), DeviceRole::Machine);
		assert_eq!(
			"machine".parse::<DeviceRole>().unwrap(),
			DeviceRole::Machine
		);
		assert_eq!("SERVER".parse::<DeviceRole>().unwrap(), DeviceRole::Machine);
		assert_eq!(
			serde_json::from_str::<DeviceRole>("\"server\"").unwrap(),
			DeviceRole::Machine
		);
	}

	/// The alias is on the input only: what Canopy stores and presents is the
	/// machine role, so the two do not both travel through the code.
	// spec: DTR
	#[test]
	fn the_machine_role_is_only_ever_written_as_machine() {
		assert_eq!(DeviceRole::Machine.to_string(), "machine");
		assert_eq!(String::from(DeviceRole::Machine), "machine");
		assert_eq!(
			serde_json::to_string(&DeviceRole::Machine).unwrap(),
			"\"machine\""
		);
	}
}
