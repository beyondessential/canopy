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
}

/// Fields required to create a new server group.
#[derive(Debug, Clone, Deserialize, Insertable, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::server_groups)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct NewServerGroup {
	/// The group's display name.
	pub name: String,
	/// Free-form operator notes about this group. Defaults to empty.
	#[serde(default)]
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
	/// New free-form operator notes for the group.
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

impl ServerGroup {
	pub async fn create(db: &mut AsyncPgConnection, new: NewServerGroup) -> Result<Self> {
		use crate::schema::server_groups;
		crate::tags::reject_reserved_keys(&new.tags)?;
		diesel::insert_into(server_groups::table)
			.values(new)
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)
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
		use crate::schema::server_groups::dsl;
		if let Some(tags) = &changes.tags {
			crate::tags::reject_reserved_keys(tags)?;
			crate::inventory_secret_variables::reject_secret_names(
				db,
				crate::inventory_secret_variables::TagScope::Group { group_id },
				tags,
			)
			.await?;
		}
		diesel::update(dsl::server_groups.filter(dsl::id.eq(group_id)))
			.set(changes)
			.execute(db)
			.await
			.map_err(AppError::from)?;
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
						"group {group_id} has {} server(s) that reported within the last \
						 week; only a group whose applications have all gone quiet can be \
						 archived",
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
		Ok(out)
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
		let canonical = members
			.iter()
			.filter(|s| s.r#type == ApplicationType::TamanuCentral)
			.min_by_key(|s| (rank_priority(s.rank), s.id))
			.map(|s| s.id);

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

	/// Loose search by UUID or name substring, capped at 50 results, used by
	/// the admin UI's group picker.
	pub async fn search(db: &mut AsyncPgConnection, query: &str) -> Result<Vec<Self>> {
		use crate::schema::server_groups::dsl;
		let pattern = format!("%{}%", query);

		if let Ok(qid) = query.parse::<Uuid>()
			&& let Ok(group) = Self::get_by_id(db, qid).await
			&& group.deleted_at.is_none()
		{
			return Ok(vec![group]);
		}

		dsl::server_groups
			.select(Self::as_select())
			.filter(dsl::name.ilike(&pattern))
			.filter(dsl::deleted_at.is_null())
			.order(dsl::name.asc())
			.limit(50)
			.load(db)
			.await
			.map_err(AppError::from)
	}
}
