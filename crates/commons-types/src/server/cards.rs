use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
	server::{app_type::ApplicationType, rank::ServerRank},
	status::{HealthState, OperatorPresence, ShortStatus},
	version::VersionStr,
};

/// Current status of a single server within a group, as shown on the status
/// dashboard.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FacilityServerStatus {
	/// Unique identifier of the server.
	pub id: Uuid,
	/// Name of the server.
	pub name: String,
	/// Reachability of the server, based on how recently it last reported a
	/// status update.
	pub up: ShortStatus,
	/// The server's self-reported health.
	pub health: HealthState,
	/// Whether canopy alerts on this server's checks. An unmonitored
	/// server's reachability and health are determined and presented as
	/// normal, but raise nothing — so consumers mark it, rather than
	/// showing a failure that nobody is being paged about.
	// spec: CHK#monitoring-gate
	pub is_monitored: bool,
	/// People currently connected to this server, from its latest status
	/// update. Always empty unless the server is actively reporting (`up`
	/// or `blip`) — a stale report can't be trusted to reflect who is
	/// connected right now.
	pub operators: Vec<OperatorPresence>,
	/// The server's rank, when set. Lets consumers group or order servers by
	/// rank (e.g. production before clone, demo, and so on) in a consistent
	/// order.
	pub rank: Option<ServerRank>,
	/// The application the server runs, presented alongside its role.
	// spec: APP
	pub r#type: ApplicationType,
	/// The box this application runs on. Members sharing one are presented
	/// together, since a box carrying two workloads is the case the machine
	/// grain exists for.
	// spec: FLT
	pub machine_id: Uuid,
	/// The box's name, where an operator gave it one.
	pub machine_name: Option<String>,
	/// The box's own reachability, which is not this application's: a machine
	/// that has gone quiet takes everything on it with it, and one that is fine
	/// says nothing about whether the software on it is.
	pub machine_up: ShortStatus,
	/// The box's own health, from the checks filed against it.
	pub machine_health: HealthState,
	/// Whether a maintenance window suspends this application, by naming it or
	/// by covering the box it runs on.
	// spec: MNT#presentation
	pub maintained: bool,
	/// Whether the window suspending it was declared over this application in
	/// particular. A window reaching it through its box is marked at that
	/// grain, so the dot stays plain.
	// spec: MNT#presentation
	pub own_window: bool,
	/// Whether every window covering this application has ended and it is
	/// serving out the settle period.
	// spec: MNT#settling
	pub maintenance_settling: bool,
	/// Whether a maintenance window suspends the box this runs on, its own or
	/// one reaching it through its environment or its group.
	// spec: MNT#presentation
	pub machine_maintained: bool,
	/// Whether the window covering this box was declared over the box itself,
	/// as against one it falls under through its environment or its group. A
	/// reader marks at the grain the operator declared at, so an environment's
	/// window is not drawn as every box in it having its own.
	// spec: MNT#presentation
	pub machine_own_window: bool,
	/// Whether every window over the box has ended and it is serving out the
	/// settle period, so a lift reads as taken effect rather than as a window
	/// that is still holding.
	// spec: MNT#presentation
	pub machine_maintenance_settling: bool,
}

/// A status-dashboard card summarising one group of equivalent servers, with
/// a status entry per member. The group's version is taken from whichever
/// member reported most recently.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ServerGroupCard {
	/// Unique identifier of the group.
	pub id: Uuid,
	/// Name of the group.
	pub name: String,
	/// Free-text notes about the group.
	pub notes: String,
	/// Version reported by the most recently-reporting member of the group,
	/// when known.
	pub version: Option<VersionStr>,
	/// How far the reported `version` lags behind the latest known release,
	/// when both are known. Combines the major and minor version gaps into a
	/// single number that grows the further behind the version is (major
	/// gaps count for more than minor ones); `0` means the version is
	/// current or newer than the latest known release.
	pub version_distance: Option<u64>,
	/// Status of each server belonging to this group.
	pub members: Vec<FacilityServerStatus>,
	/// Whether every member of the group has been quiet for long enough that
	/// archiving the group cascades to them.
	///
	/// This is the archive rule, not a reachability reading: a member that
	/// last reported months ago is thoroughly unreachable but has still
	/// reported, so `up` alone cannot answer it. Both this and the rule the
	/// archive itself enforces ask the same question of the same window, so
	/// the button offered and the outcome cannot disagree.
	pub all_members_quiet: bool,
	/// The group's environments under a window of their own, so a reader marks
	/// the environment's row rather than each box in it.
	// spec: MNT#presentation
	pub maintained_ranks: Vec<ServerRank>,
	/// Of those, the ones serving out the settle period.
	// spec: MNT#settling
	pub settling_ranks: Vec<ServerRank>,
	/// Whether a window is declared over the group itself, covering every box
	/// in it whatever its rank.
	// spec: MNT#presentation
	pub maintained: bool,
	/// Whether the group's own window has ended and it is serving out the
	/// settle period.
	// spec: MNT#settling
	pub maintenance_settling: bool,
}
