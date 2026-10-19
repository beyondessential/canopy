//! The audit log: who did what, to what.
//!
//! Spec: `.workhorse/specs/platform/audit-log.md` (id `AUD`).
//!
//! An entry records an action from one catalogue ([`Action`]), the records it
//! acted on, and the action's own detail. It is written as a condition of the
//! action: either in the transaction that carries the action's change
//! ([`record`]), or committed ahead of it as `pending` ([`commit_ahead`]) and
//! settled afterwards ([`settle`]).
//!
//! The table's trigger is the other half of the contract: nothing about an
//! entry changes except a pending one being settled, once, within an hour.

use std::collections::BTreeMap;

use commons_errors::{AppError, Result};
use commons_types::{device::DeviceRole, server::rank::ServerRank};
use diesel::{prelude::*, sql_types};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

/// How long a pending entry can still be settled. The table's trigger enforces
/// the same hour by the database's clock.
pub const SETTLE_WINDOW: SignedDuration = SignedDuration::from_hours(1);

/// How far back the log page and the fleet query interface reach.
pub const READ_WINDOW: SignedDuration = SignedDuration::from_hours(90 * 24);

/// How long an entry stays in the database before it can leave it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Retention {
	/// Ninety days.
	Standard,
	/// A year, for the few actions whose entries matter long after the fact.
	Year,
}

impl Retention {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Standard => "standard",
			Self::Year => "year",
		}
	}
}

/// How much an action is expected to be wanted when reading the log.
///
/// The log is read as the important entries unless the reader asks for more, so
/// an action that is taken at high volume and is not what anyone is looking for
/// by default is routine, and stays out of the way until asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
	/// Read by default.
	Important,
	/// Frequent, and left out unless the reader asks for routine actions or for
	/// the action by name.
	Routine,
}

/// The kinds of record an action can act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
	Machine,
	Group,
	Application,
	Device,
	MaintenanceWindow,
	UpgradePlan,
	RestoreReplica,
	ReportingSchema,
	Check,
	Cluster,
	Credential,
	Variable,
	Lease,
	Version,
	Artifact,
	Name,
	Certificate,
}

/// A record an action acted on: its identifier, and its name as it was when the
/// action was taken, so the entry still reads once the record is renamed or
/// gone. Held without a foreign key, so an entry never stops a record being
/// deleted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Record {
	/// What kind of record it is.
	pub kind: RecordKind,
	/// Its identifier.
	pub id: String,
	/// Its name when the action was taken.
	pub name: String,
}

impl Record {
	pub fn new(kind: RecordKind, id: impl ToString, name: impl Into<String>) -> Self {
		Self {
			kind,
			id: id.to_string(),
			name: name.into(),
		}
	}
}

impl Record {
	/// A machine, named as it is now.
	pub async fn machine(conn: &mut AsyncPgConnection, id: Uuid) -> Result<Self> {
		let machine = crate::Machine::get_by_id(conn, id).await?;
		Ok(Self::new(RecordKind::Machine, id, machine.name))
	}

	/// A group, named as it is now.
	pub async fn group(conn: &mut AsyncPgConnection, id: Uuid) -> Result<Self> {
		let group = crate::server_groups::ServerGroup::get_by_id(conn, id).await?;
		Ok(Self::new(RecordKind::Group, id, group.name))
	}

	/// An application, named as it is now.
	pub async fn application(conn: &mut AsyncPgConnection, id: Uuid) -> Result<Self> {
		let application = crate::applications::Application::get_by_id(conn, id).await?;
		Ok(Self::new(
			RecordKind::Application,
			id,
			application.display_name(),
		))
	}

	/// A cluster, named as it is now.
	pub async fn cluster(conn: &mut AsyncPgConnection, id: Uuid) -> Result<Self> {
		let cluster = crate::KubernetesCluster::get_by_id(conn, id).await?;
		Ok(Self::new(RecordKind::Cluster, id, cluster.name))
	}

	/// A device, which has no name of its own: it is named by the start of its
	/// identifier.
	pub fn device(id: Uuid) -> Self {
		Self::new(RecordKind::Device, id, id.to_string()[..8].to_owned())
	}

	/// An upgrade plan, named by the environment it moves and the version it
	/// moves to.
	pub async fn upgrade_plan(
		conn: &mut AsyncPgConnection,
		plan: &crate::upgrade_plans::UpgradePlan,
	) -> Result<Self> {
		let group = crate::server_groups::ServerGroup::get_by_id(conn, plan.group_id).await?;
		let version = crate::versions::Version::get_by_id(conn, plan.target_version_id).await?;
		Ok(Self::new(
			RecordKind::UpgradePlan,
			plan.id,
			format!(
				"{} {} to {}.{}.{}",
				group.name, plan.rank, version.major, version.minor, version.patch
			),
		))
	}

	/// What a scope is, as a record: the application, machine, group or cluster
	/// it names, and none for the whole of Canopy.
	pub async fn scope(
		conn: &mut AsyncPgConnection,
		scope: crate::issues::Scope,
	) -> Result<Option<Self>> {
		use crate::issues::Scope;
		Ok(Some(match scope {
			Scope::Application(id) => Self::application(conn, id).await?,
			Scope::Machine(id) => Self::machine(conn, id).await?,
			Scope::Group(id) => Self::group(conn, id).await?,
			Scope::Cluster(id) => Self::cluster(conn, id).await?,
			Scope::Global => return Ok(None),
		}))
	}
}

/// Something an [`Action`] holds as the records it acted on: one, none, or
/// several.
pub trait Records {
	fn extend_records(&self, out: &mut Vec<Record>);
}

impl Records for Record {
	fn extend_records(&self, out: &mut Vec<Record>) {
		out.push(self.clone());
	}
}

impl Records for Option<Record> {
	fn extend_records(&self, out: &mut Vec<Record>) {
		out.extend(self.iter().cloned());
	}
}

impl Records for Vec<Record> {
	fn extend_records(&self, out: &mut Vec<Record>) {
		out.extend(self.iter().cloned());
	}
}

/// Who took an action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Actor {
	/// An operator, by their tailnet login. The profile picture is not stored
	/// with the entry: it is filled in when entries are read, from the cache of
	/// what Tailscale last said.
	Operator {
		login: String,
		name: Option<String>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		profile_pic: Option<String>,
	},
	/// A device, by its role, and the application it acted for where it acted
	/// for one.
	Device {
		id: Uuid,
		name: Option<String>,
		role: DeviceRole,
		application: Option<Uuid>,
	},
	/// A fleet query access token.
	Token { id: Uuid, name: String },
	/// Canopy itself, by the job that acted.
	Canopy { job: String },
}

impl Actor {
	pub fn operator(login: impl Into<String>, name: Option<String>) -> Self {
		Self::Operator {
			login: login.into(),
			name,
			profile_pic: None,
		}
	}

	pub fn canopy(job: impl Into<String>) -> Self {
		Self::Canopy { job: job.into() }
	}

	fn kind(&self) -> &'static str {
		match self {
			Self::Operator { .. } => "operator",
			Self::Device { .. } => "device",
			Self::Token { .. } => "token",
			Self::Canopy { .. } => "canopy",
		}
	}

	fn id(&self) -> String {
		match self {
			Self::Operator { login, .. } => login.clone(),
			Self::Device { id, .. } => id.to_string(),
			Self::Token { id, .. } => id.to_string(),
			Self::Canopy { job } => job.clone(),
		}
	}

	fn name(&self) -> Option<String> {
		match self {
			Self::Operator { name, .. } | Self::Device { name, .. } => name.clone(),
			Self::Token { name, .. } => Some(name.clone()),
			Self::Canopy { .. } => None,
		}
	}

	fn role(&self) -> Option<String> {
		match self {
			Self::Device { role, .. } => Some(role.to_string()),
			_ => None,
		}
	}

	fn application(&self) -> Option<Uuid> {
		match self {
			Self::Device { application, .. } => *application,
			_ => None,
		}
	}

	fn from_columns(
		kind: &str,
		id: String,
		name: Option<String>,
		role: Option<String>,
		application: Option<Uuid>,
	) -> Self {
		match kind {
			"operator" => Self::Operator {
				login: id,
				name,
				profile_pic: None,
			},
			"device" => Self::Device {
				// A device identifier that is not a UUID is not something this
				// module writes, so it is shown rather than guessed at.
				id: id.parse().unwrap_or_default(),
				name,
				role: role
					.and_then(|role| role.parse().ok())
					.unwrap_or(DeviceRole::Machine),
				application,
			},
			"token" => Self::Token {
				id: id.parse().unwrap_or_default(),
				name: name.unwrap_or_default(),
			},
			_ => Self::Canopy { job: id },
		}
	}
}

macro_rules! actions {
	($(
		$(#[$meta:meta])*
		$variant:ident = $name:literal, $label:literal $(, retention = $retention:ident)? $(, tier = $tier:ident)? {
			records { $($rf:ident : $rt:ty),* $(,)? }
			detail { $($df:ident : $dt:ty),* $(,)? }
		}
	)*) => {
		/// Every action Canopy records, with what each acted on and holds.
		///
		/// A variant has no field for a secret, so keeping secrets out of the
		/// log is a property of this type rather than of what someone
		/// remembered to leave out.
		#[derive(Clone, Debug)]
		pub enum Action {
			$(
				$(#[$meta])*
				$variant { $($rf: $rt,)* $($df: $dt,)* },
			)*
		}

		impl Action {
			/// The action's stable name, which does not change when a route
			/// or an interface is reorganised.
			pub fn name(&self) -> &'static str {
				match self {
					$(Self::$variant { .. } => $name,)*
				}
			}

			/// How long entries of this action stay in the database.
			pub fn retention(&self) -> Retention {
				match self {
					$(Self::$variant { .. } => {
						#[allow(unused_mut, unused_assignments)]
						let mut retention = Retention::Standard;
						$(retention = Retention::$retention;)?
						retention
					})*
				}
			}

			/// Whether this action is read by default.
			pub fn tier(&self) -> Tier {
				match self {
					$(Self::$variant { .. } => {
						#[allow(unused_mut, unused_assignments)]
						let mut tier = Tier::Important;
						$(tier = Tier::$tier;)?
						tier
					})*
				}
			}

			/// The records this action acted on.
			pub fn records(&self) -> Vec<Record> {
				match self {
					$(Self::$variant { $($rf,)* .. } => {
						#[allow(unused_mut)]
						let mut out = Vec::new();
						$($rf.extend_records(&mut out);)*
						out
					})*
				}
			}

			/// What the action defines as worth knowing about it.
			pub fn detail(&self) -> Value {
				match self {
					$(Self::$variant { $($df,)* .. } => {
						#[allow(unused_mut)]
						let mut detail = serde_json::Map::new();
						$(detail.insert(
							stringify!($df).to_owned(),
							serde_json::to_value($df).unwrap_or(Value::Null),
						);)*
						Value::Object(detail)
					})*
				}
			}
		}

		/// What each action is called to an operator, and whether it is
		/// routine, by stable name.
		const LABELS: &[(&str, &str, Tier)] = &[$(($name, $label, {
			#[allow(unused_mut, unused_assignments)]
			let mut tier = Tier::Important;
			$(tier = Tier::$tier;)?
			tier
		})),*];

		/// The stable name of each action, by variant, for a route to declare
		/// the actions it takes.
		#[allow(non_upper_case_globals)]
		pub mod names {
			$(pub const $variant: &str = $name;)*
		}

		/// Every action name in the catalogue.
		pub const CATALOGUE: &[&str] = &[$($name),*];
	};
}

actions! {
	/// A maintenance window was declared over a target.
	MaintenanceDeclared = "maintenance_declared", "Declare maintenance" {
		records { window: Record, target: Record, upgrade_plan: Option<Record> }
		detail { rank: Option<ServerRank>, expected_end: Timestamp, note: Option<String> }
	}

	/// An open maintenance window was changed: its end, its note, or what it
	/// covers. Declaring over a target that already has a window amends it.
	MaintenanceAmended = "maintenance_amended", "Amend maintenance" {
		records { window: Record, target: Record, moved_from: Option<Record> }
		detail { rank: Option<ServerRank>, expected_end: Timestamp, note: Option<String> }
	}

	/// An open maintenance window was lifted before its expected end.
	MaintenanceLifted = "maintenance_lifted", "Lift maintenance" {
		records { window: Record, target: Record }
		detail { rank: Option<ServerRank>, already_ended: bool }
	}

	/// An inventory was read under a lease. Taken at the rate runs read, so it
	/// is routine.
	InventoryRead = "inventory_read", "Read inventory", tier = Routine {
		records { group: Record, lease: Record }
		detail { rank: ServerRank, intent: String }
	}

	/// A run lease on an environment was taken.
	InventoryLeaseTaken = "inventory_lease_taken", "Take inventory lease" {
		records { group: Record, lease: Record }
		detail { rank: ServerRank, intent: String, note: Option<String>, expires_at: Timestamp }
	}

	/// A run lease another operator held was taken over.
	InventoryLeaseTakenOver = "inventory_lease_taken_over", "Take over inventory lease" {
		records { group: Record, lease: Record }
		detail {
			rank: ServerRank, intent: String, note: Option<String>, expires_at: Timestamp,
			previous_holder: Option<String>,
		}
	}

	/// A held lease's expiry was pushed out. A run does it every few minutes,
	/// so it is routine.
	InventoryLeaseExtended = "inventory_lease_extended", "Extend inventory lease", tier = Routine {
		records { group: Record, lease: Record }
		detail { rank: ServerRank, expires_at: Timestamp }
	}

	/// A lease was released, by its holder or by someone taking the work over.
	InventoryLeaseReleased = "inventory_lease_released", "Release inventory lease" {
		records { group: Record, lease: Record }
		detail { rank: ServerRank, held_by: Option<String> }
	}

	/// An inventory variable was set. The value is not recorded.
	InventoryVariableSet = "inventory_variable_set", "Set inventory variable" {
		records { variable: Record, scope: Record }
		detail { rank: Option<ServerRank>, secret: bool }
	}

	/// An inventory variable was removed.
	InventoryVariableRemoved = "inventory_variable_removed", "Remove inventory variable" {
		records { variable: Record, scope: Record }
		detail { rank: Option<ServerRank>, secret: bool }
	}

	/// An upgrade plan was recorded for an environment.
	UpgradePlanRecorded = "upgrade_plan_recorded", "Record upgrade plan" {
		records { plan: Record, group: Record }
		detail {
			rank: ServerRank, planned_for: Option<String>, planned_time: Option<String>,
			note: Option<String>,
		}
	}

	/// An open upgrade plan's date or note was amended.
	UpgradePlanAmended = "upgrade_plan_amended", "Amend upgrade plan" {
		records { plan: Record, group: Record }
		detail {
			rank: ServerRank, planned_for: Option<String>, planned_time: Option<String>,
			note: Option<String>,
		}
	}

	/// An upgrade plan was withdrawn: the group is no longer going there. Kept
	/// a year, since who withdrew a plan matters long after.
	UpgradePlanWithdrawn = "upgrade_plan_withdrawn", "Withdraw upgrade plan", retention = Year {
		records { plan: Record, group: Record }
		detail { rank: ServerRank }
	}

	/// A restore replica declaration was created.
	RestoreReplicaDeclared = "restore_replica_declared", "Declare restore replica" {
		records { replica: Record, group: Record, machine: Option<Record>, consumer: Record }
		detail {
			backup_type: String, intent: String, enabled: bool, redacts: bool,
			publishes_schemas: bool,
		}
	}

	/// A restore replica declaration was changed, enabling and disabling it
	/// among the rest.
	RestoreReplicaUpdated = "restore_replica_updated", "Update restore replica" {
		records { replica: Record, group: Record, machine: Option<Record>, consumer: Record }
		detail {
			backup_type: String, intent: String, enabled: bool, redacts: bool,
			publishes_schemas: bool,
		}
	}

	/// A restore replica declaration was deleted.
	RestoreReplicaDeleted = "restore_replica_deleted", "Delete restore replica" {
		records { replica: Record, group: Record, consumer: Record }
		detail { backup_type: String, intent: String }
	}

	/// A query was run in the SQL playground. A read, recorded before it runs.
	SqlQueryRun = "sql_query_run", "Run SQL query" {
		records { }
		detail { query: String }
	}

	/// An operator minted a device credential.
	DeviceCredentialProvisioned = "device_credential_provisioned", "Provision device credential" {
		records { device: Record, key: Record }
		detail { role: String }
	}

	/// An operator added a key to a device.
	DeviceKeyAdded = "device_key_added", "Add device key" {
		records { device: Record }
		detail { key_name: Option<String> }
	}

	/// A machine enrolled. The device the enrolment created is the actor.
	MachineEnrolled = "machine_enrolled", "Enrol machine" {
		records { machine: Record }
		detail { }
	}

	/// A machine reported an application Canopy held no record of, and Canopy
	/// created it.
	ApplicationArrived = "application_arrived", "Application arrived" {
		records { application: Record, machine: Record }
		detail { application_type: String }
	}

	/// A check was reported that the catalogue did not hold, and registered.
	CheckRegistered = "check_registered", "Register check" {
		records { check: Record }
		detail { source: String, namespace: String }
	}

	/// Short-lived storage credentials were issued for a backup run. Issued per
	/// run across the fleet, so routine. Committed ahead of the issue and settled
	/// with the access key and expiry it produced, which are added to the detail
	/// when it is settled.
	BackupCredentialsIssued = "backup_credentials_issued", "Issue backup credentials", tier = Routine {
		records { group: Record, machine: Record }
		detail { backup_type: String, purpose: String, run_id: Option<String> }
	}

	/// Short-lived read-only credentials were issued for a restore. Settled with
	/// the access key and expiry it produced, as backup credentials are.
	RestoreCredentialsIssued = "restore_credentials_issued", "Issue restore credentials", tier = Routine {
		records { group: Record, device: Record }
		detail { backup_type: String }
	}

	/// A release version was published, or its record created, by a releaser.
	VersionPublished = "version_published", "Publish version" {
		records { version: Record, device: Record }
		detail { }
	}

	/// A release version was removed by a releaser.
	VersionRemoved = "version_removed", "Remove version" {
		records { version: Record, device: Record }
		detail { }
	}

	/// An artifact was registered against a version.
	ArtifactRegistered = "artifact_registered", "Register artifact" {
		records { artifact: Record, version: Option<Record>, group: Option<Record>, device: Record }
		detail { artifact_type: String, platform: String }
	}
}

/// What an action is called to an operator, from its stable name. An action
/// the catalogue no longer has, read back from an older entry, is its name
/// spelt out.
pub fn label(name: &str) -> String {
	LABELS
		.iter()
		.find(|(known, ..)| *known == name)
		.map(|(_, label, _)| (*label).to_owned())
		.unwrap_or_else(|| {
			let mut label = name.replace('_', " ");
			if let Some(first) = label.get_mut(0..1) {
				first.make_ascii_uppercase();
			}
			label
		})
}

/// The names of the actions that are routine, for leaving them out.
pub fn routine() -> Vec<&'static str> {
	LABELS
		.iter()
		.filter(|(_, _, tier)| *tier == Tier::Routine)
		.map(|(name, ..)| *name)
		.collect()
}

/// How an entry stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Status {
	/// Written with its action, or settled done.
	Done,
	/// Settled failed: the action knew its effect did not take place.
	Failed,
	/// Committed ahead and never settled, and past the point it can be: what it
	/// records may have taken place though the action did not complete.
	Unconfirmed,
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = crate::schema::audit_entries)]
#[diesel(check_for_backend(diesel::pg::Pg))]
struct EntryRow {
	id: Uuid,
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	recorded_at: Timestamp,
	retention: String,
	actor_kind: String,
	actor_id: String,
	actor_name: Option<String>,
	actor_role: Option<String>,
	actor_application_id: Option<Uuid>,
	action: String,
	records: Value,
	detail: Value,
	outcome: Option<String>,
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp)]
	settled_at: Option<Timestamp>,
}

/// One entry as the log presents it.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Entry {
	/// The entry's identifier.
	pub id: Uuid,
	/// When the entry was recorded.
	#[schema(value_type = String, format = DateTime)]
	pub recorded_at: Timestamp,
	/// How long the entry stays in the database.
	pub retention: Retention,
	/// Who took the action.
	pub actor: Actor,
	/// The action's stable name.
	pub action: String,
	/// What the action is called to an operator.
	pub action_label: String,
	/// The records the action acted on.
	pub records: Vec<Record>,
	/// What the action defines as worth knowing about it.
	#[schema(value_type = Object)]
	pub detail: Value,
	/// Whether the action was done, failed, or is unconfirmed.
	pub status: Status,
	/// When the entry was settled, for one committed ahead of its action.
	#[schema(value_type = Option<String>, format = DateTime)]
	pub settled_at: Option<Timestamp>,
}

impl EntryRow {
	fn into_entry(self, now: Timestamp) -> Entry {
		let status = match self.outcome.as_deref() {
			Some("failed") => Status::Failed,
			// Pending and past the window it can be settled in. One still inside
			// it is not presented, so does not reach here.
			Some("pending") => Status::Unconfirmed,
			_ => Status::Done,
		};
		debug_assert!(
			self.outcome.as_deref() != Some("pending")
				|| now.duration_since(self.recorded_at) >= SETTLE_WINDOW
		);
		Entry {
			id: self.id,
			recorded_at: self.recorded_at,
			retention: if self.retention == "year" {
				Retention::Year
			} else {
				Retention::Standard
			},
			actor: Actor::from_columns(
				&self.actor_kind,
				self.actor_id,
				self.actor_name,
				self.actor_role,
				self.actor_application_id,
			),
			action_label: label(&self.action),
			action: self.action,
			records: serde_json::from_value(self.records).unwrap_or_default(),
			detail: self.detail,
			status,
			settled_at: self.settled_at,
		}
	}
}

#[derive(Insertable)]
#[diesel(table_name = crate::schema::audit_entries)]
struct NewEntry {
	id: Uuid,
	retention: &'static str,
	actor_kind: &'static str,
	actor_id: String,
	actor_name: Option<String>,
	actor_role: Option<String>,
	actor_application_id: Option<Uuid>,
	action: &'static str,
	records: Value,
	detail: Value,
	outcome: Option<&'static str>,
}

impl NewEntry {
	fn new(actor: &Actor, action: &Action, outcome: Option<&'static str>) -> Self {
		Self {
			id: Uuid::new_v4(),
			retention: action.retention().as_str(),
			actor_kind: actor.kind(),
			actor_id: actor.id(),
			actor_name: actor.name(),
			actor_role: actor.role(),
			actor_application_id: actor.application(),
			action: action.name(),
			records: json!(action.records()),
			detail: action.detail(),
			outcome,
		}
	}
}

/// An entry committed ahead of its action, awaiting settling.
///
/// Carries the whole key of the row so settling goes straight to its partition.
#[derive(Clone, Debug)]
pub struct Pending {
	pub id: Uuid,
	pub recorded_at: Timestamp,
	pub retention: Retention,
}

/// Write an entry in the transaction that carries the action it records, so
/// that either both stand or neither does.
pub async fn record(conn: &mut AsyncPgConnection, actor: &Actor, action: &Action) -> Result<Uuid> {
	use crate::schema::audit_entries::dsl;

	let new = NewEntry::new(actor, action, None);
	let id = new.id;
	diesel::insert_into(dsl::audit_entries)
		.values(&new)
		.execute(conn)
		.await
		.map_err(AppError::from)?;
	Ok(id)
}

/// Write an entry ahead of its action, as pending. The connection should commit
/// on its own, so the entry stands whatever becomes of the action.
pub async fn commit_ahead(
	conn: &mut AsyncPgConnection,
	actor: &Actor,
	action: &Action,
) -> Result<Pending> {
	use crate::schema::audit_entries::dsl;

	let new = NewEntry::new(actor, action, Some("pending"));
	let (id, recorded_at): (Uuid, jiff_diesel::Timestamp) = diesel::insert_into(dsl::audit_entries)
		.values(&new)
		.returning((dsl::id, dsl::recorded_at))
		.get_result(conn)
		.await
		.map_err(AppError::from)?;
	Ok(Pending {
		id,
		recorded_at: recorded_at.to_jiff(),
		retention: action.retention(),
	})
}

/// How a pending entry is settled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
	Done,
	Failed,
}

impl Outcome {
	fn as_str(self) -> &'static str {
		match self {
			Self::Done => "done",
			Self::Failed => "failed",
		}
	}
}

/// Settle a pending entry, once. `extra` is merged into its detail, for what the
/// action has learned since, such as the identifier of a credential an outside
/// service issued.
///
/// The row is locked first, so two settlers cannot both succeed. Settled done
/// belongs in the transaction carrying the rest of the action's work; settled
/// failed, on a connection that commits by itself, since that work is rolled
/// back.
pub async fn settle(
	conn: &mut AsyncPgConnection,
	pending: &Pending,
	outcome: Outcome,
	extra: Option<Value>,
) -> Result<()> {
	use crate::schema::audit_entries::dsl;

	let key = dsl::id
		.eq(pending.id)
		.and(dsl::recorded_at.eq(jiff_diesel::Timestamp::from(pending.recorded_at)))
		.and(dsl::retention.eq(pending.retention.as_str()));

	let locked: Option<Uuid> = dsl::audit_entries
		.filter(key)
		.filter(dsl::outcome.eq("pending"))
		.select(dsl::id)
		.for_update()
		.first(conn)
		.await
		.optional()
		.map_err(AppError::from)?;
	if locked.is_none() {
		return Err(AppError::custom(
			"this audit entry is already settled or has lapsed",
		));
	}

	let extra = extra.unwrap_or_else(|| json!({}));
	diesel::update(dsl::audit_entries.filter(key))
		.set((
			dsl::outcome.eq(outcome.as_str()),
			dsl::detail.eq(dsl::detail.concat(extra)),
		))
		.execute(conn)
		.await
		.map_err(AppError::from)?;
	Ok(())
}

/// What to look for.
#[derive(Clone, Debug, Default)]
pub struct Filter {
	pub actor_kind: Option<String>,
	pub actor_id: Option<String>,
	pub action: Option<String>,
	/// A record acted on, by identifier.
	pub record: Option<String>,
	/// A record acted on, by part of the name it had.
	pub record_name: Option<String>,
	/// Entries whose detail contains this.
	pub detail: Option<Value>,
	/// Include routine actions. Without it they are left out, unless one is
	/// asked for by name with [`Self::action`].
	pub include_routine: bool,
	pub since: Option<Timestamp>,
	pub until: Option<Timestamp>,
	pub limit: i64,
}

/// The pattern matching a name that contains `name`, with the characters
/// `ILIKE` reads specially taken literally.
fn contains_pattern(name: &str) -> String {
	format!(
		"%{}%",
		name.replace('\\', "\\\\")
			.replace('%', "\\%")
			.replace('_', "\\_")
	)
}

/// Whether one of an entry's records has a name containing this.
macro_rules! name_matches {
	($name:expr) => {
		diesel::dsl::sql::<sql_types::Bool>(
			"EXISTS (SELECT 1 FROM jsonb_array_elements(records) AS acted WHERE acted ->> 'name' ILIKE ",
		)
		.bind::<sql_types::Text, _>(contains_pattern($name))
		.sql(")")
	};
}

/// Entries matching a filter, newest first.
pub async fn find(conn: &mut AsyncPgConnection, filter: &Filter) -> Result<Vec<Entry>> {
	use crate::schema::audit_entries::dsl;

	let mut query = dsl::audit_entries.into_boxed();
	query = query.filter(
		dsl::outcome
			.is_distinct_from("pending")
			.or(
				dsl::recorded_at.le(diesel::dsl::sql::<sql_types::Timestamptz>(
					"now() - interval '1 hour'",
				)),
			),
	);
	if let Some(kind) = &filter.actor_kind {
		query = query.filter(dsl::actor_kind.eq(kind));
	}
	if let Some(id) = &filter.actor_id {
		query = query.filter(dsl::actor_id.eq(id));
	}
	if let Some(action) = &filter.action {
		query = query.filter(dsl::action.eq(action));
	} else if !filter.include_routine {
		query = query.filter(dsl::action.ne_all(routine()));
	}
	match (&filter.record, &filter.record_name) {
		(Some(id), Some(name)) => {
			// One box offers both, so an entry matches by either.
			query = query.filter(
				dsl::records
					.contains(json!([{ "id": id }]))
					.or(name_matches!(name)),
			);
		}
		(Some(id), None) => query = query.filter(dsl::records.contains(json!([{ "id": id }]))),
		(None, Some(name)) => query = query.filter(name_matches!(name)),
		(None, None) => {}
	}
	if let Some(detail) = &filter.detail {
		query = query.filter(dsl::detail.contains(detail.clone()));
	}
	if let Some(since) = filter.since {
		query = query.filter(dsl::recorded_at.ge(jiff_diesel::Timestamp::from(since)));
	}
	if let Some(until) = filter.until {
		query = query.filter(dsl::recorded_at.lt(jiff_diesel::Timestamp::from(until)));
	}

	let rows: Vec<EntryRow> = query
		.order((dsl::recorded_at.desc(), dsl::id.desc()))
		.limit(filter.limit.clamp(1, 500))
		.select(EntryRow::as_select())
		.load(conn)
		.await
		.map_err(AppError::from)?;
	let now = Timestamp::now();
	let mut entries: Vec<Entry> = rows.into_iter().map(|row| row.into_entry(now)).collect();
	with_pictures(conn, entries.iter_mut().map(|entry| &mut entry.actor)).await?;
	Ok(entries)
}

/// Fill in each operator's profile picture from the cache of what Tailscale last
/// said, where it has one.
async fn with_pictures<'a>(
	conn: &mut AsyncPgConnection,
	actors: impl Iterator<Item = &'a mut Actor>,
) -> Result<()> {
	let mut actors: Vec<&mut Actor> = actors.collect();
	let logins: Vec<&str> = actors
		.iter()
		.filter_map(|actor| match &**actor {
			Actor::Operator { login, .. } => Some(login.as_str()),
			_ => None,
		})
		.collect();
	let known = crate::tailscale_users::TailscaleUser::by_logins(conn, &logins).await?;
	for actor in actors.iter_mut() {
		if let Actor::Operator {
			login, profile_pic, ..
		} = &mut **actor
		{
			*profile_pic = known
				.get(login.as_str())
				.and_then(|user| user.profile_pic.clone());
		}
	}
	Ok(())
}

/// How many entries of an action carry each value of one key of their detail,
/// over a window. This is how a count per signing key, or per anything else an
/// action puts in its detail, is read.
pub async fn count_by_detail(
	conn: &mut AsyncPgConnection,
	action: &str,
	key: &str,
	since: Timestamp,
	until: Timestamp,
) -> Result<BTreeMap<String, i64>> {
	#[derive(QueryableByName)]
	struct Row {
		#[diesel(sql_type = sql_types::Text)]
		value: String,
		#[diesel(sql_type = sql_types::BigInt)]
		entries: i64,
	}

	let rows: Vec<Row> = diesel::sql_query(
		"SELECT detail ->> $2 AS value, count(*) AS entries \
		 FROM audit_entries \
		 WHERE action = $1 AND recorded_at >= $3 AND recorded_at < $4 \
		   AND detail ? $2 \
		   AND (outcome IS DISTINCT FROM 'pending' OR recorded_at <= now() - interval '1 hour') \
		 GROUP BY 1",
	)
	.bind::<sql_types::Text, _>(action)
	.bind::<sql_types::Text, _>(key)
	.bind::<sql_types::Timestamptz, _>(jiff_diesel::Timestamp::from(since))
	.bind::<sql_types::Timestamptz, _>(jiff_diesel::Timestamp::from(until))
	.load(conn)
	.await
	.map_err(AppError::from)?;
	Ok(rows
		.into_iter()
		.map(|row| (row.value, row.entries))
		.collect())
}

/// Who has acted in a window, each actor once, as they last appeared: what an
/// actor filter offers.
pub async fn actors(conn: &mut AsyncPgConnection, since: Timestamp) -> Result<Vec<Actor>> {
	use crate::schema::audit_entries::dsl;

	type Row = (String, String, Option<String>, Option<String>, Option<Uuid>);
	let rows: Vec<Row> = dsl::audit_entries
		.filter(dsl::recorded_at.ge(jiff_diesel::Timestamp::from(since)))
		.select((
			dsl::actor_kind,
			dsl::actor_id,
			dsl::actor_name,
			dsl::actor_role,
			dsl::actor_application_id,
		))
		.distinct_on((dsl::actor_kind, dsl::actor_id))
		.order((dsl::actor_kind, dsl::actor_id, dsl::recorded_at.desc()))
		.load(conn)
		.await
		.map_err(AppError::from)?;
	let mut actors: Vec<Actor> = rows
		.into_iter()
		.map(|(kind, id, name, role, application)| {
			Actor::from_columns(&kind, id, name, role, application)
		})
		.collect();
	with_pictures(conn, actors.iter_mut()).await?;
	Ok(actors)
}

/// Every action in the catalogue with what it is called to an operator, and
/// whether it is routine.
pub fn catalogue() -> Vec<(&'static str, String, Tier)> {
	LABELS
		.iter()
		.map(|(name, label, tier)| (*name, (*label).to_owned(), *tier))
		.collect()
}
