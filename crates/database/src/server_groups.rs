//! Application groups: a flat unit grouping several applications together for the
//! purposes of incident roll-up, shared tags, and shared operator notes.

use commons_errors::{AppError, Result};
use commons_types::server::{TagMap, app_type::ApplicationType, rank::ServerRank};
use commons_types::version::VersionStr;
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::applications::Application;
use crate::notes_log::{self, Author, NotesRecord};
use crate::pg_duration::PgDuration;
use crate::statuses::Status;

/// Ordering key for a server's rank — lower is higher priority. Used to pick a
/// group's canonical member (and to bucket groups on the status page). `None`
/// (unranked) sorts last.
pub fn rank_priority(rank: Option<ServerRank>) -> u8 {
	match rank {
		Some(ServerRank::Production) => 0,
		Some(ServerRank::Clone) => 1,
		Some(ServerRank::Demo) => 2,
		Some(ServerRank::Test) => 3,
		Some(ServerRank::Dev) => 4,
		None => 5,
	}
}

/// Ordering key for a server's kind — lower is higher priority. Central applications
/// are the headline of a group; facility ties below them, standalone last.
// spec: APP#versions
fn higher_rank(a: ServerRank, b: ServerRank) -> ServerRank {
	if rank_priority(Some(a)) <= rank_priority(Some(b)) {
		a
	} else {
		b
	}
}

/// A group's applications at one rank, and what the environment's own central
/// reports running.
// spec: GRP#environments
#[derive(Debug, Clone)]
pub struct Environment {
	pub group_id: Uuid,
	pub rank: ServerRank,
	/// The group's highest-ranked environment, which is the one the group's
	/// own checks belong to.
	pub headline: bool,
	pub version: Option<VersionStr>,
}

/// How an environment is named where it is read: the group's name, with the
/// rank after it unless it is production.
// spec: INC#notification
pub fn environment_name(group: &str, rank: ServerRank) -> String {
	match rank {
		ServerRank::Production => group.to_owned(),
		rank => format!("{group} {rank}"),
	}
}

/// A group of applications managed together: incidents roll up across the group,
/// members share tags, and the group carries its own notes and
/// notification settings.
#[derive(
	Debug, Clone, Serialize, Deserialize, Queryable, Selectable, Insertable, utoipa::ToSchema,
)]
#[diesel(table_name = crate::schema::server_groups)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct ServerGroup {
	/// Unique identifier for this group.
	pub id: Uuid,
	/// When this group was created.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	/// When this group was last modified.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub updated_at: Timestamp,
	/// The group's display name.
	pub name: String,
	/// Free-form operator notes about this group.
	#[serde(default)]
	pub notes: String,
	/// Key/value tags shared by every server in the group.
	#[serde(default)]
	pub tags: TagMap,
	/// How long, in seconds, an incident-opened notification waits before
	/// it's sent. If the incident resolves within this window, the
	/// notification is cancelled outright and never sent — useful for
	/// groups prone to brief flapping, so a transient blip doesn't spam
	/// notifications, without losing the underlying incident record.
	#[schema(value_type = i64, format = "int64")]
	pub slack_open_delay: PgDuration,
	/// The id of the group's canonical member server (the one whose version
	/// is reflected in `effective_version`), chosen by highest rank then
	/// highest kind. `None` when the group has no members.
	pub version_application_id: Option<Uuid>,
	/// The version reported by the group's canonical member server. `None`
	/// if the group has no members or that member hasn't reported a
	/// version yet.
	pub effective_version: Option<VersionStr>,
	/// When set, the group is archived: hidden from live listings but kept,
	/// along with its members, and can be restored.
	#[serde(skip_serializing_if = "Option::is_none")]
	#[diesel(
		deserialize_as = jiff_diesel::NullableTimestamp,
		serialize_as = jiff_diesel::NullableTimestamp,
		treat_none_as_default_value = false
	)]
	pub deleted_at: Option<Timestamp>,
	/// How long, in seconds, an incident lingers after its last failure
	/// recovers before it closes and the resolved notification is sent. A
	/// failure returning within this window continues the same incident
	/// instead of opening (and notifying about) a new one — the close-side
	/// mirror of `slack_open_delay`, for a red check that briefly blips
	/// green.
	#[schema(value_type = i64, format = "int64")]
	pub slack_close_delay: PgDuration,
	/// The project the group serves, as free text. Absent when not filled in.
	// spec: GRP#project-details
	pub project: Option<String>,
	/// The PM in charge of the group's project, by their tailnet login or an
	/// email address. Absent when not filled in.
	// spec: GRP#project-details
	pub pm_in_charge: Option<String>,
	/// The group's contact on the ground, as free text. Absent when not filled
	/// in.
	// spec: GRP#project-details
	pub ground_contact: Option<String>,
}

/// A group's three project details, each absent when not filled in.
// spec: GRP#project-details
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ProjectDetails {
	/// The project the group serves, as free text.
	#[serde(default)]
	pub project: Option<String>,
	/// The PM in charge, by their tailnet login or an email address: text
	/// either side of an `@`.
	#[serde(default)]
	pub pm_in_charge: Option<String>,
	/// The contact on the ground, as free text.
	#[serde(default)]
	pub ground_contact: Option<String>,
}

impl ProjectDetails {
	/// The details as stored: each trimmed, and an empty one absent.
	fn normalised(self) -> Self {
		fn one(value: Option<String>) -> Option<String> {
			value
				.map(|value| value.trim().to_owned())
				.filter(|value| !value.is_empty())
		}
		Self {
			project: one(self.project),
			pm_in_charge: one(self.pm_in_charge),
			ground_contact: one(self.ground_contact),
		}
	}

	/// Refuse a PM in charge that doesn't name a person by a login or an
	/// address, with text either side of an `@`.
	fn validate(&self) -> Result<()> {
		let Some(pm) = &self.pm_in_charge else {
			return Ok(());
		};
		match pm.split_once('@') {
			Some((local, domain)) if !local.is_empty() && !domain.is_empty() => Ok(()),
			_ => Err(AppError::BadRequest(format!(
				"the PM in charge must be a tailnet login or an email address, \
				 with text either side of an @; got {pm:?}"
			))),
		}
	}

	/// The details with the display name Canopy knows for the PM in charge,
	/// from `names` as [`crate::tailscale_users::TailscaleUser::names_ignoring_case`]
	/// returns them.
	pub fn named(self, names: &std::collections::HashMap<String, String>) -> NamedProjectDetails {
		let pm_in_charge_name = self
			.pm_in_charge
			.as_deref()
			.and_then(|login| names.get(&login.to_lowercase()))
			.cloned();
		NamedProjectDetails {
			project: self.project,
			pm_in_charge: self.pm_in_charge,
			pm_in_charge_name,
			ground_contact: self.ground_contact,
		}
	}
}

/// A group's project details as they are read, the PM in charge with the
/// display name Canopy knows for them.
// spec: GRP#project-details
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct NamedProjectDetails {
	/// The project the group serves, as free text. Absent when not filled in.
	pub project: Option<String>,
	/// The PM in charge, by their login or address as entered. Absent when
	/// not filled in.
	pub pm_in_charge: Option<String>,
	/// The display name Canopy knows for the PM in charge's login, compared
	/// ignoring case. Absent where it knows none, in which case the PM in
	/// charge is named as entered.
	pub pm_in_charge_name: Option<String>,
	/// The contact on the ground, as free text. Absent when not filled in.
	pub ground_contact: Option<String>,
}

/// One project detail's value before and after a change.
// spec: GRP#project-details
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetailChange {
	pub from: Option<String>,
	pub to: Option<String>,
}

impl DetailChange {
	fn between(from: &Option<String>, to: &Option<String>) -> Option<Self> {
		(from != to).then(|| Self {
			from: from.clone(),
			to: to.clone(),
		})
	}
}

/// What a change to a group's project details changed: each detail that
/// differs, with its value before and after, and nothing for one left as it
/// was.
// spec: GRP#project-details
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectDetailsChange {
	pub project: Option<DetailChange>,
	pub pm_in_charge: Option<DetailChange>,
	pub ground_contact: Option<DetailChange>,
}

impl ProjectDetailsChange {
	/// Whether the change left every detail as it was.
	pub fn is_empty(&self) -> bool {
		self.project.is_none() && self.pm_in_charge.is_none() && self.ground_contact.is_none()
	}
}

/// A production Tamanu central or facility, as the projects view lists it.
// spec: PRJ#what-each-group-shows
#[derive(Debug, Clone)]
pub struct ProductionTamanu {
	pub group_id: Uuid,
	pub application_id: Uuid,
	pub r#type: ApplicationType,
	/// The application's own name, where an operator gave it one.
	pub name: Option<String>,
	/// The application's configured URL.
	pub url: Option<crate::url_field::UrlField>,
	/// The machine it runs on: none for an application hosted on a cluster.
	pub machine_id: Option<Uuid>,
	pub machine_name: Option<String>,
	/// The tailnet name of the device bound to its machine, where there is one.
	pub tailnet_name: Option<String>,
}

/// Fields required to create a new server group.
#[derive(Debug, Clone, Deserialize, Insertable, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::server_groups)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct NewServerGroup {
	/// The group's display name.
	pub name: String,
	/// Free-form operator notes about this group. Defaults to empty. Held in
	/// the group's notes log from [`ServerGroup::create_by`], not inserted with
	/// the row.
	#[serde(default)]
	#[diesel(skip_insertion)]
	pub notes: String,
	/// Key/value tags shared by every server in the group. Defaults to
	/// empty.
	#[serde(default)]
	pub tags: TagMap,
	/// How long, in seconds, an incident-opened notification waits before
	/// it's sent, letting brief flaps resolve without notifying. Defaults
	/// to the system default delay if omitted.
	#[serde(default)]
	#[schema(value_type = Option<i64>, format = "int64")]
	pub slack_open_delay: Option<PgDuration>,
	/// How long, in seconds, an incident lingers after its last failure
	/// recovers before it closes and notifies as resolved. Defaults to the
	/// system default if omitted.
	#[serde(default)]
	#[schema(value_type = Option<i64>, format = "int64")]
	pub slack_close_delay: Option<PgDuration>,
}

/// Fields to update on an existing server group. Only the fields present
/// are changed; omitted fields are left as-is.
#[derive(Debug, Clone, Deserialize, AsChangeset, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::server_groups)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct PartialServerGroup {
	/// New display name for the group.
	pub name: Option<String>,
	/// New free-form operator notes for the group, applied through the group's
	/// notes log by [`ServerGroup::update_by`] rather than as a column update.
	#[diesel(skip_update)]
	pub notes: Option<String>,
	/// New set of key/value tags shared by every server in the group. This
	/// replaces the whole tag set.
	pub tags: Option<TagMap>,
	/// New incident-opened notification delay, in seconds.
	#[schema(value_type = Option<i64>, format = "int64")]
	pub slack_open_delay: Option<PgDuration>,
	/// New incident linger window, in seconds.
	#[schema(value_type = Option<i64>, format = "int64")]
	pub slack_close_delay: Option<PgDuration>,
}

/// What a group search matches its term against, beside the group's identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupMatch {
	/// The group's name.
	Name,
	/// The group's name and its project details.
	// spec: MCP#discovery
	NameOrProjectDetails,
}

impl ServerGroup {
	/// The group's project details as stored.
	// spec: GRP#project-details
	pub fn project_details(&self) -> ProjectDetails {
		ProjectDetails {
			project: self.project.clone(),
			pm_in_charge: self.pm_in_charge.clone(),
			ground_contact: self.ground_contact.clone(),
		}
	}

	/// Set a group's project details, returning what changed. Each is trimmed
	/// and an empty one stored as absent; a PM in charge without text either
	/// side of an `@` is refused. A save leaving them as they were writes
	/// nothing.
	// spec: GRP#project-details
	pub async fn update_project_details(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		details: ProjectDetails,
	) -> Result<ProjectDetailsChange> {
		use crate::schema::server_groups::dsl;
		use diesel_async::AsyncConnection;

		let details = details.normalised();
		details.validate()?;
		db.transaction::<_, AppError, _>(async |conn| {
			let (project, pm_in_charge, ground_contact): (
				Option<String>,
				Option<String>,
				Option<String>,
			) = dsl::server_groups
				.select((dsl::project, dsl::pm_in_charge, dsl::ground_contact))
				.filter(dsl::id.eq(group_id))
				.for_update()
				.first(conn)
				.await
				.map_err(AppError::from)?;
			let change = ProjectDetailsChange {
				project: DetailChange::between(&project, &details.project),
				pm_in_charge: DetailChange::between(&pm_in_charge, &details.pm_in_charge),
				ground_contact: DetailChange::between(&ground_contact, &details.ground_contact),
			};
			if change.is_empty() {
				return Ok(change);
			}
			diesel::update(dsl::server_groups.filter(dsl::id.eq(group_id)))
				.set((
					dsl::project.eq(&details.project),
					dsl::pm_in_charge.eq(&details.pm_in_charge),
					dsl::ground_contact.eq(&details.ground_contact),
				))
				.execute(conn)
				.await
				.map_err(AppError::from)?;
			Ok(change)
		})
		.await
	}

	/// Every live group with a live production environment, in order of name:
	/// a production application in it, or a production machine, which is in
	/// its environment whether or not anything on it has reported.
	// spec: PRJ#which-groups-it-lists
	// spec: GRP#environments
	pub async fn with_production_environment(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::{applications, machines, server_groups::dsl};
		use diesel::dsl::exists;

		let production_application = applications::table
			.filter(applications::group_id.eq(dsl::id.nullable()))
			.filter(applications::deleted_at.is_null())
			.filter(applications::rank.eq(ServerRank::Production));
		let production_machine = machines::table
			.filter(machines::group_id.eq(dsl::id.nullable()))
			.filter(machines::deleted_at.is_null())
			.filter(machines::rank.eq(ServerRank::Production));
		dsl::server_groups
			.select(Self::as_select())
			.filter(dsl::deleted_at.is_null())
			.filter(exists(production_application).or(exists(production_machine)))
			.order((dsl::name.asc(), dsl::id.asc()))
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// The live production Tamanu centrals and facilities of `group_ids`, with
	/// the machine each runs on and the tailnet name of the device bound to
	/// it. A pending application, or one at another rank, is in no production
	/// environment and is left out.
	// spec: PRJ#what-each-group-shows
	pub async fn production_tamanu(
		db: &mut AsyncPgConnection,
		group_ids: &[Uuid],
	) -> Result<Vec<ProductionTamanu>> {
		use crate::schema::{applications, devices, machines};

		if group_ids.is_empty() {
			return Ok(Vec::new());
		}
		let types = [
			ApplicationType::TamanuCentral.to_string(),
			ApplicationType::TamanuFacility.to_string(),
		];
		#[allow(clippy::type_complexity)]
		let rows: Vec<(
			Uuid,
			Uuid,
			String,
			Option<String>,
			Option<crate::url_field::UrlField>,
			Option<Uuid>,
			Option<String>,
			Option<String>,
		)> = applications::table
			.left_join(machines::table.left_join(devices::table))
			.select((
				applications::id,
				applications::group_id.assume_not_null(),
				applications::type_,
				applications::name,
				applications::host,
				applications::machine_id,
				machines::name.nullable(),
				devices::tailscale_node_name.nullable(),
			))
			.filter(applications::group_id.eq_any(group_ids))
			.filter(applications::deleted_at.is_null())
			.filter(applications::rank.eq(ServerRank::Production))
			.filter(applications::type_.eq_any(&types))
			.filter(machines::deleted_at.is_null())
			.load(db)
			.await
			.map_err(AppError::from)?;
		Ok(rows
			.into_iter()
			.filter_map(
				|(
					application_id,
					group_id,
					r#type,
					name,
					url,
					machine_id,
					machine_name,
					tailnet,
				)| {
					Some(ProductionTamanu {
						group_id,
						application_id,
						r#type: r#type.parse().ok()?,
						name,
						url,
						machine_id,
						machine_name,
						tailnet_name: tailnet,
					})
				},
			)
			.collect())
	}

	pub async fn create(db: &mut AsyncPgConnection, new: NewServerGroup) -> Result<Self> {
		Self::create_by(db, new, None).await
	}

	/// Create a group, its notes starting its notes log under `author`.
	// spec: FLT#notes
	pub async fn create_by(
		db: &mut AsyncPgConnection,
		new: NewServerGroup,
		author: Option<Author<'_>>,
	) -> Result<Self> {
		use crate::schema::server_groups;
		crate::tags::reject_reserved_keys(&new.tags)?;
		let notes = new.notes.clone();
		let group: Self = diesel::insert_into(server_groups::table)
			.values(new)
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)?;
		if notes.is_empty() {
			return Ok(group);
		}
		notes_log::set(db, NotesRecord::Group(group.id), &notes, author, None).await?;
		Self::get_by_id(db, group.id).await
	}

	pub async fn get_by_id(db: &mut AsyncPgConnection, group_id: Uuid) -> Result<Self> {
		use crate::schema::server_groups::dsl;
		dsl::server_groups
			.select(Self::as_select())
			.filter(dsl::id.eq(group_id))
			.first(db)
			.await
			.map_err(AppError::from)
	}

	/// The named groups' names by id, including archived ones, so a reference
	/// to a group can be shown by name whatever state the group is in.
	pub async fn names_by_ids(
		db: &mut AsyncPgConnection,
		ids: &[Uuid],
	) -> Result<std::collections::HashMap<Uuid, String>> {
		use crate::schema::server_groups::dsl;

		if ids.is_empty() {
			return Ok(std::collections::HashMap::new());
		}

		Ok(dsl::server_groups
			.select((dsl::id, dsl::name))
			.filter(dsl::id.eq_any(ids))
			.load::<(Uuid, String)>(db)
			.await
			.map_err(AppError::from)?
			.into_iter()
			.collect())
	}

	/// The group's canonical central application: the highest-ranked one, and
	/// the lowest id among equals so the choice is stable.
	///
	/// There is no fallback. A group with no central has none, because a
	/// group's version, and the database a reporting schema is built from, are
	/// things its central has and nothing else stands in for.
	// spec: APP#capabilities
	pub fn canonical_central(
		members: &[crate::applications::Application],
	) -> Option<&crate::applications::Application> {
		members
			.iter()
			.filter(|s| s.r#type == ApplicationType::TamanuCentral)
			.min_by_key(|s| (rank_priority(s.rank), s.id))
	}

	/// The central of one of a group's environments, the lowest id among
	/// several so the choice is stable.
	// spec: RPT#the-build-contract
	pub fn environment_central(
		members: &[crate::applications::Application],
		rank: ServerRank,
	) -> Option<&crate::applications::Application> {
		members
			.iter()
			.filter(|s| s.r#type == ApplicationType::TamanuCentral && s.rank == Some(rank))
			.min_by_key(|s| s.id)
	}

	/// Groups carrying this exact name, archived ones included, so a caller
	/// resolving a group by name can tell an archived group from one that does
	/// not exist. Names aren't unique, so this can return several.
	pub async fn find_by_name(db: &mut AsyncPgConnection, name: &str) -> Result<Vec<Self>> {
		use crate::schema::server_groups::dsl;
		dsl::server_groups
			.select(Self::as_select())
			.filter(dsl::name.eq(name))
			.load(db)
			.await
			.map_err(AppError::from)
	}

	pub async fn list_all(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::server_groups::dsl;
		dsl::server_groups
			.select(Self::as_select())
			.filter(dsl::deleted_at.is_null())
			.order(dsl::name.asc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Archived (soft-deleted) groups, newest-first. Powers the Archived view.
	pub async fn list_archived(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::server_groups::dsl;
		dsl::server_groups
			.select(Self::as_select())
			.filter(dsl::deleted_at.is_not_null())
			.order(dsl::name.asc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	pub async fn list_by_ids(db: &mut AsyncPgConnection, ids: &[Uuid]) -> Result<Vec<Self>> {
		use crate::schema::server_groups::dsl;
		if ids.is_empty() {
			return Ok(Vec::new());
		}
		dsl::server_groups
			.select(Self::as_select())
			.filter(dsl::id.eq_any(ids))
			.filter(dsl::deleted_at.is_null())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	pub async fn update(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		changes: PartialServerGroup,
	) -> Result<Self> {
		Self::update_by(db, group_id, changes, None).await
	}

	/// Update a group, a change to its notes going through its notes log under
	/// `author`.
	// spec: FLT#notes
	pub async fn update_by(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		changes: PartialServerGroup,
		author: Option<Author<'_>>,
	) -> Result<Self> {
		use crate::schema::server_groups::dsl;
		if let Some(tags) = &changes.tags {
			crate::tags::reject_reserved_keys(tags)?;
		}
		let notes = changes.notes.clone();
		diesel::update(dsl::server_groups.filter(dsl::id.eq(group_id)))
			.set(changes)
			.execute(db)
			.await
			.optional_empty_changeset()
			.map_err(AppError::from)?;
		if let Some(notes) = notes {
			notes_log::set(db, NotesRecord::Group(group_id), &notes, author, None).await?;
		}
		Self::get_by_id(db, group_id).await
	}

	/// Archive (soft-delete) a group, hiding it from live listings while keeping
	/// it (and its members) and allowing restore.
	///
	/// An empty group archives outright. A group with live members archives only
	/// if *every* live member is **gone** (no status in the last 7 days — the
	/// same notion the UI shows): in that case the archive **cascades**, also
	/// archiving those members. If any live member reported recently, it refuses
	/// (409) — you don't bulk-archive a group with active applications.
	pub async fn soft_delete(db: &mut AsyncPgConnection, group_id: Uuid) -> Result<()> {
		use crate::schema::{applications, server_groups::dsl};
		use diesel_async::AsyncConnection;

		let archived_at = Timestamp::now();
		db.transaction::<_, AppError, _>(async |conn| {
			let member_ids: Vec<Uuid> = applications::table
				.select(applications::id)
				.filter(applications::group_id.eq(group_id))
				.filter(applications::deleted_at.is_null())
				.load(conn)
				.await
				.map_err(AppError::from)?;

			if !member_ids.is_empty() {
				// A member is quiet iff it's absent from `latest_for_servers`
				// (no status in the last 7 days). Allow the cascade only when
				// every live member is quiet; any recent reporter blocks it.
				//
				// This is deliberately the windowed question and not
				// reachability: a member that last reported months ago is
				// unreachable rather than never heard from, and archiving it
				// is still the right call. The card carries the same fact as
				// `all_members_quiet` so the button offered matches what this
				// allows.
				let recent = Status::latest_for_servers(conn, &member_ids).await?;
				if !recent.is_empty() {
					return Err(AppError::Conflict(format!(
						"this group has {} server(s) that reported within the last week; \
						 only a group whose applications have all gone quiet can be archived",
						recent.len(),
					)));
				}
				for id in &member_ids {
					Application::soft_delete(conn, *id).await?;
				}

				// Stamp the whole cascade with the group's own archival time.
				// `member_ids` is exactly the members that were live a moment
				// ago, so every one of them was archived by *this* cascade —
				// and matching timestamps is what lets `restore` tell them
				// from a server an operator had already archived on its own.
				diesel::update(applications::table.filter(applications::id.eq_any(&member_ids)))
					.set(
						applications::deleted_at
							.eq(jiff_diesel::NullableTimestamp::from(Some(archived_at))),
					)
					.execute(conn)
					.await
					.map_err(AppError::from)?;
			}

			diesel::update(dsl::server_groups.filter(dsl::id.eq(group_id)))
				.set(dsl::deleted_at.eq(jiff_diesel::NullableTimestamp::from(Some(archived_at))))
				.execute(conn)
				.await
				.map_err(AppError::from)?;
			Ok(())
		})
		.await
	}

	/// Un-archive a group, cascading to restore the members its archival took
	/// down with it — the exact inverse of [`ServerGroup::soft_delete`]'s
	/// cascade.
	///
	/// Only those members. A server an operator archived deliberately *before*
	/// the group was archived is not part of the cascade and stays archived:
	/// resurrecting it would put a decommissioned box back into a group whose
	/// `is_monitored` survived archival, so it would rejoin monitoring and
	/// start filing "never reported" alerts nobody asked for. The cascade is
	/// identified by `deleted_at` matching the group's, which `soft_delete`
	/// stamps across the set it archives.
	pub async fn restore(db: &mut AsyncPgConnection, group_id: Uuid) -> Result<()> {
		use crate::schema::{applications, server_groups::dsl};
		use diesel_async::AsyncConnection;

		db.transaction::<_, AppError, _>(async |conn| {
			let archived_at: Option<Timestamp> = dsl::server_groups
				.select(dsl::deleted_at)
				.filter(dsl::id.eq(group_id))
				.first::<jiff_diesel::NullableTimestamp>(conn)
				.await
				.map_err(AppError::from)?
				.into();

			// A group that isn't archived has no cascade to undo. Restoring
			// its archived members would be the same resurrection by another
			// route.
			let archived_members: Vec<Uuid> = match archived_at {
				Some(at) => applications::table
					.select(applications::id)
					.filter(applications::group_id.eq(group_id))
					.filter(
						applications::deleted_at.eq(jiff_diesel::NullableTimestamp::from(Some(at))),
					)
					.load(conn)
					.await
					.map_err(AppError::from)?,
				None => Vec::new(),
			};
			for id in &archived_members {
				Application::restore(conn, *id).await?;
			}

			diesel::update(dsl::server_groups.filter(dsl::id.eq(group_id)))
				.set(dsl::deleted_at.eq(None::<jiff_diesel::Timestamp>))
				.execute(conn)
				.await
				.map_err(AppError::from)?;
			Ok(())
		})
		.await
	}

	pub async fn list_servers(&self, db: &mut AsyncPgConnection) -> Result<Vec<Application>> {
		use crate::schema::applications::dsl;
		dsl::applications
			.select(Application::as_select())
			.filter(dsl::group_id.eq(self.id))
			.filter(dsl::deleted_at.is_null())
			.order(dsl::name.asc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// For each group id, the software its live members all run, when they
	/// agree on one. A group whose members span two is absent from the map.
	///
	/// Attribution is by software rather than by type: a central and a facility
	/// of one group are both Tamanu, so a group holding the pair still has
	/// one product to attribute its shared cost to.
	// spec: APP#billing-attribution
	pub async fn sole_member_software(
		db: &mut AsyncPgConnection,
		group_ids: &[Uuid],
	) -> Result<std::collections::HashMap<Uuid, String>> {
		use crate::schema::applications::dsl;
		use std::collections::HashMap;

		if group_ids.is_empty() {
			return Ok(HashMap::new());
		}
		let rows: Vec<(Uuid, String)> = dsl::applications
			.select((dsl::group_id.assume_not_null(), dsl::type_))
			.filter(dsl::group_id.eq_any(group_ids))
			.filter(dsl::deleted_at.is_null())
			.load(db)
			.await?;

		// Two passes rather than one: a group has to be *removed* once a
		// second software shows up, which a running "first wins" insert can't
		// express.
		let mut seen: HashMap<Uuid, Vec<String>> = HashMap::new();
		for (gid, r#type) in rows {
			let Ok(r#type) = r#type.parse::<ApplicationType>() else {
				continue;
			};
			let software = seen.entry(gid).or_default();
			if !software.iter().any(|s| s == r#type.software()) {
				software.push(r#type.software().to_owned());
			}
		}
		Ok(seen
			.into_iter()
			.filter_map(|(gid, mut software)| match software.as_mut_slice() {
				[sole] => Some((gid, std::mem::take(sole))),
				_ => None,
			})
			.collect())
	}

	/// For each group id, the rank of its highest-ranked member. Used by the
	/// Status page to bucket groups by rank (Production beats Clone, Demo,
	/// Test, Dev). Groups without ranked members are absent from the map.
	pub async fn highest_member_ranks(
		db: &mut AsyncPgConnection,
		group_ids: &[Uuid],
	) -> Result<std::collections::HashMap<Uuid, commons_types::server::rank::ServerRank>> {
		use crate::schema::applications::dsl;
		use std::collections::HashMap;

		if group_ids.is_empty() {
			return Ok(HashMap::new());
		}
		let rows: Vec<(Uuid, Option<String>)> = dsl::applications
			.select((dsl::group_id.assume_not_null(), dsl::rank))
			.filter(dsl::group_id.eq_any(group_ids))
			.filter(dsl::deleted_at.is_null())
			.load(db)
			.await?;

		let mut out: HashMap<Uuid, commons_types::server::rank::ServerRank> = HashMap::new();
		for (gid, rank_str) in rows {
			let Some(rank): Option<commons_types::server::rank::ServerRank> =
				rank_str.and_then(|s| s.parse().ok())
			else {
				continue;
			};
			let cur = out.get(&gid).copied();
			let better = match cur {
				None => rank,
				Some(existing) => higher_rank(existing, rank),
			};
			out.insert(gid, better);
		}
		for (gid, rank) in Self::ranked_machines(db, group_ids).await? {
			let better = match out.get(&gid).copied() {
				None => rank,
				Some(existing) => higher_rank(existing, rank),
			};
			out.insert(gid, better);
		}
		Ok(out)
	}

	/// A group's headline rank, if it has one: `None` for no group or while
	/// nothing in it is ranked.
	// spec: GRP#a-groups-headline-rank
	pub async fn headline_rank(
		db: &mut AsyncPgConnection,
		group_id: Option<Uuid>,
	) -> Result<Option<ServerRank>> {
		let Some(group_id) = group_id else {
			return Ok(None);
		};
		Ok(Self::highest_member_ranks(db, &[group_id])
			.await?
			.get(&group_id)
			.copied())
	}

	/// Count of live (non-archived) applications in each group, keyed by group id.
	/// Groups with no live members are absent (callers default to 0).
	pub async fn live_server_counts(
		db: &mut AsyncPgConnection,
	) -> Result<std::collections::HashMap<Uuid, i64>> {
		use crate::schema::applications::dsl;
		use std::collections::HashMap;

		let group_ids: Vec<Uuid> = dsl::applications
			.select(dsl::group_id.assume_not_null())
			.filter(dsl::group_id.is_not_null())
			.filter(dsl::deleted_at.is_null())
			.load(db)
			.await?;

		let mut counts: HashMap<Uuid, i64> = HashMap::new();
		for gid in group_ids {
			*counts.entry(gid).or_insert(0) += 1;
		}
		Ok(counts)
	}

	/// Every environment across `group_ids`: each group's live members at one
	/// rank, in the order the groups were given and then by rank,
	/// production first. A pending application is in no environment, so a group
	/// with nothing ranked has none.
	///
	/// An environment's version is its own central's, which is the group's
	/// headline derivation confined to one rank, so an environment holding no
	/// central has no version.
	// spec: GRP#environments
	pub async fn environments(
		db: &mut AsyncPgConnection,
		group_ids: &[Uuid],
	) -> Result<Vec<Environment>> {
		Self::environments_inner(db, group_ids, true).await
	}

	/// The environments a group has, without the version each reports: for a
	/// caller that wants the ranks and would discard the rest.
	// spec: GRP#environments
	pub async fn environment_ranks(
		db: &mut AsyncPgConnection,
		group_ids: &[Uuid],
	) -> Result<Vec<Environment>> {
		Self::environments_inner(db, group_ids, false).await
	}

	/// The group and rank of each live ranked machine in `group_ids`: a machine
	/// is in the environment its rank names whether or not anything on it has
	/// reported, so a group's environments read its machines beside its
	/// applications.
	// spec: GRP#environments
	async fn ranked_machines(
		db: &mut AsyncPgConnection,
		group_ids: &[Uuid],
	) -> Result<Vec<(Uuid, ServerRank)>> {
		use crate::schema::machines::dsl;

		dsl::machines
			.select((dsl::group_id.assume_not_null(), dsl::rank.assume_not_null()))
			.filter(dsl::group_id.eq_any(group_ids))
			.filter(dsl::rank.is_not_null())
			.filter(dsl::deleted_at.is_null())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	async fn environments_inner(
		db: &mut AsyncPgConnection,
		group_ids: &[Uuid],
		with_versions: bool,
	) -> Result<Vec<Environment>> {
		use crate::schema::applications::dsl;
		use std::collections::{BTreeSet, HashMap};

		if group_ids.is_empty() {
			return Ok(Vec::new());
		}
		// `applications.rank` is unconstrained text, so an unknown spelling
		// leaves its application in no environment and the fleet still readable.
		let members: Vec<(Uuid, Option<Uuid>, Option<String>, String)> = dsl::applications
			.select((dsl::id, dsl::group_id, dsl::rank, dsl::type_))
			.filter(dsl::group_id.eq_any(group_ids))
			.filter(dsl::deleted_at.is_null())
			.load(db)
			.await?;

		let parsed: Vec<(Uuid, Option<Uuid>, Option<ServerRank>, String)> = members
			.into_iter()
			.map(|(id, group_id, rank, r#type)| {
				(
					id,
					group_id,
					rank.and_then(|rank| rank.parse().ok()),
					r#type,
				)
			})
			.collect();

		let tamanu_central = ApplicationType::TamanuCentral.to_string();
		let mut present: HashMap<Uuid, BTreeSet<ServerRank>> = HashMap::new();
		let mut central: HashMap<(Uuid, ServerRank), Uuid> = HashMap::new();
		for (id, group_id, rank, r#type) in parsed {
			// A pending application is in no environment until it is ranked.
			let (Some(group_id), Some(rank)) = (group_id, rank) else {
				continue;
			};
			present.entry(group_id).or_default().insert(rank);
			if !r#type.eq_ignore_ascii_case(&tamanu_central) {
				continue;
			}
			central
				.entry((group_id, rank))
				.and_modify(|held| {
					if id < *held {
						*held = id;
					}
				})
				.or_insert(id);
		}
		for (group_id, rank) in Self::ranked_machines(db, group_ids).await? {
			present.entry(group_id).or_default().insert(rank);
		}

		// A caller that only needs the ranks does not pay for the version each
		// environment reports.
		let versions = if with_versions {
			let ids: Vec<Uuid> = central.values().copied().collect();
			crate::reported_detail::ReportedDetail::last_versions(db, &ids).await?
		} else {
			HashMap::new()
		};

		let mut out = Vec::new();
		for group_id in group_ids {
			let mut ranks: Vec<ServerRank> = present
				.get(group_id)
				.map(|ranks| ranks.iter().copied().collect())
				.unwrap_or_default();
			ranks.sort_by_key(|rank| rank_priority(Some(*rank)));
			for (position, rank) in ranks.into_iter().enumerate() {
				out.push(Environment {
					group_id: *group_id,
					rank,
					headline: position == 0,
					version: central
						.get(&(*group_id, rank))
						.and_then(|id| versions.get(id))
						.cloned(),
				});
			}
		}
		Ok(out)
	}

	/// One of a group's environments, if the group has members at that rank.
	// spec: GRP#environments
	pub async fn environment(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		rank: ServerRank,
	) -> Result<Option<Environment>> {
		Ok(Self::environments(db, &[group_id])
			.await?
			.into_iter()
			.find(|env| env.rank == rank))
	}

	/// Recompute the cached canonical member and its version for `group_id`.
	///
	/// Loads every member of the group, picks the canonical one among those
	/// whose product canopy tracks versions for (lowest
	/// `(rank_priority, kind_priority)`, tie-broken by `id`), and caches that
	/// member's last reported version. No such members → both cache columns
	/// are cleared. The `statuses` trigger handles the hot path (canonical
	/// member reporting a new version) without touching this.
	pub async fn recompute_version(db: &mut AsyncPgConnection, group_id: Uuid) -> Result<()> {
		use crate::schema::server_groups::dsl;

		let members: Vec<Application> = {
			use crate::schema::applications::dsl as servers_dsl;
			servers_dsl::applications
				.select(Application::as_select())
				.filter(servers_dsl::group_id.eq(group_id))
				.filter(servers_dsl::deleted_at.is_null())
				.load(db)
				.await?
		};

		// A group's version is its central's version, so the headline
		// names that outright: the `tamanu-central` on the group's
		// highest-ranked machine. Saying it directly is clearer than a
		// precedence order over types that happens to put centrals first, and
		// it leaves types a flat set with no ordering to maintain as new ones
		// appear.
		//
		// There is no fallback. A group with no central has no headline
		// version, because a group's version is a thing its central has
		// and nothing else stands in for it.
		// spec: APP#capabilities
		let canonical = Self::canonical_central(&members).map(|s| s.id);

		let (version_application_id, effective_version) = match canonical {
			None => (None, None),
			Some(application_id) => {
				let version =
					crate::reported_detail::ReportedDetail::last_version(db, application_id)
						.await?;
				(Some(application_id), version)
			}
		};

		diesel::update(dsl::server_groups.filter(dsl::id.eq(group_id)))
			.set((
				dsl::version_application_id.eq(version_application_id),
				dsl::effective_version.eq(effective_version),
			))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}

	/// Live groups whose identifier is `term`, or whose text `matching` names
	/// contains it ignoring case, in order of name, and no more than `limit` of
	/// them where one is given. A wildcard in the term matches only itself.
	// spec: MCP#discovery
	pub async fn search(
		db: &mut AsyncPgConnection,
		term: &str,
		matching: GroupMatch,
		limit: Option<i64>,
	) -> Result<Vec<Self>> {
		use crate::schema::server_groups::dsl;

		if let Ok(id) = term.parse::<Uuid>()
			&& let Ok(group) = Self::get_by_id(db, id).await
			&& group.deleted_at.is_none()
		{
			return Ok(vec![group]);
		}

		let pattern = crate::audit::contains_pattern(term);
		let mut query = dsl::server_groups
			.select(Self::as_select())
			.filter(dsl::deleted_at.is_null())
			.order((dsl::name.asc(), dsl::id.asc()))
			.into_boxed();
		query = match matching {
			GroupMatch::Name => query.filter(dsl::name.ilike(pattern)),
			GroupMatch::NameOrProjectDetails => query.filter(
				dsl::name
					.ilike(pattern.clone())
					.nullable()
					.or(dsl::project.ilike(pattern.clone()))
					.or(dsl::pm_in_charge.ilike(pattern.clone()))
					.or(dsl::ground_contact.ilike(pattern)),
			),
		};
		if let Some(limit) = limit {
			query = query.limit(limit);
		}
		query.load(db).await.map_err(AppError::from)
	}
}
