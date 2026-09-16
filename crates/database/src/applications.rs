use commons_errors::{AppError, Result};
use commons_types::{
	geo::GeoPoint,
	server::{RESERVED_TAG_PREFIX, TagMap, app_type::ApplicationType, rank::ServerRank},
	status::ShortStatus,
};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::pg_duration::PgDuration;
use super::url_field::UrlField;

/// Recompute each distinct, present group id, deduping repeats and skipping
/// `None`. Used by the server write paths that can change a group's canonical
/// member (membership/rank/kind/delete).
async fn recompute_groups(
	db: &mut AsyncPgConnection,
	groups: impl IntoIterator<Item = Option<Uuid>>,
) -> Result<()> {
	let mut seen: Vec<Uuid> = Vec::new();
	for gid in groups.into_iter().flatten() {
		if !seen.contains(&gid) {
			seen.push(gid);
			crate::server_groups::ServerGroup::recompute_version(db, gid).await?;
		}
	}
	Ok(())
}

/// A single application in the fleet: the unit that reports status, files
/// issues, and is monitored for reachability. An application runs on a
/// machine and may belong to a server group. The identity that reports for
/// it belongs to the machine, not to the application.
#[derive(
	Debug, Clone, Serialize, Deserialize, Queryable, Selectable, Insertable, utoipa::ToSchema,
)]
#[diesel(table_name = crate::schema::applications)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Application {
	/// Unique identifier for this server.
	pub id: Uuid,
	/// The server's display name, scoped within its group. May not be
	/// globally unique or meaningful outside the group.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub name: Option<String>,

	/// The application's URL. Optional: an application may be identified by
	/// its machine's tailnet name rather than a URL. Not required to be
	/// unique. Responses that need a value to display even when this is
	/// unset fall back to another identifying field.
	#[serde(skip_serializing_if = "Option::is_none")]
	#[diesel(treat_none_as_default_value = false)]
	pub host: Option<UrlField>,

	/// What this application is: the software and the role it plays together,
	/// for example `tamanu-central`. Decides which of Canopy's per-application
	/// features apply to it at all.
	// spec: APP
	#[diesel(column_name = type_, deserialize_as = String, serialize_as = String)]
	pub r#type: ApplicationType,
	/// The server's environment tier, for example production, test, or dev.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub rank: Option<ServerRank>,
	/// The machine this application runs on. An application runs on exactly
	/// one; a machine hosts any number.
	// spec: FLT#cardinality
	pub machine_id: Uuid,
	/// The key the reporter that found this application named it by.
	///
	/// A reporter cannot know what Canopy calls the applications on a machine,
	/// so it chooses a key of its own and Canopy correlates on it. The key is
	/// the reporter's, so it is unique within a machine rather than across the
	/// fleet, and it is never disclosed the other way: Canopy's own identifier
	/// for an application stays internal.
	///
	/// `None` on an application Canopy created from a transitional unified
	/// push, which has no key to give. The first split-shape push that names
	/// such an application by type claims it.
	// spec: STA#identifying-an-application
	#[serde(skip_serializing_if = "Option::is_none")]
	#[diesel(treat_none_as_default_value = false)]
	pub reported_key: Option<String>,
	/// The server group this server belongs to, if any.
	///
	/// A denormalisation of the machine's group: an operator sets the group on
	/// the machine and the applications on it take it. Carried here so a group
	/// query reads one column rather than joining through the machine.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub group_id: Option<Uuid>,
	/// If set, the server is listed publicly under this name (used by
	/// end-user-facing clients). `None` means it is not listed publicly.
	/// Separate from `name` because that field is only meaningful within
	/// the server's group.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub public_name: Option<String>,
	/// Whether this server is hosted in the cloud, if known.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub cloud: Option<bool>,
	/// The server's physical location, if known.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub geolocation: Option<GeoPoint>,
	/// Whether this server is actively monitored. When `false`, reachability
	/// checks skip it entirely and its issues no longer contribute to
	/// incidents — useful for test environments and ad-hoc demos. The
	/// `alert_when_down_for` threshold is preserved while unmonitored, so
	/// turning monitoring back on doesn't lose the configured value.
	pub is_monitored: bool,
	/// How long, in seconds, a server's status may go without an update
	/// before it's considered unreachable and an issue is filed. Increase
	/// it for applications with flaky connectivity; decrease it for critical
	/// applications that should alert promptly. Only enforced while
	/// `is_monitored` is `true`. Must be a positive number of seconds;
	/// defaults to 600 (10 minutes) for newly-created applications.
	#[schema(value_type = i64)]
	pub alert_when_down_for: PgDuration,
	/// Free-form operator notes about this server.
	#[serde(default)]
	pub notes: String,
	/// Key/value tags for this server.
	#[serde(default)]
	pub tags: TagMap,
	/// When set, the application is archived: hidden from live listings and
	/// monitoring, but its history is retained. Retiring one workload says
	/// nothing about the box it ran on, so the machine keeps its identity;
	/// `Machine::archive` is what releases that.
	#[serde(skip_serializing_if = "Option::is_none")]
	#[diesel(
		deserialize_as = jiff_diesel::NullableTimestamp,
		serialize_as = jiff_diesel::NullableTimestamp,
		treat_none_as_default_value = false
	)]
	pub deleted_at: Option<Timestamp>,
	/// When this application first reported. Enrolment is the machine's, so
	/// this is set by the report that brings the application into being, not
	/// by anything the operator does. While `None`, the application is
	/// awaiting its first check-in and the UI shows setup instructions.
	#[serde(skip_serializing_if = "Option::is_none")]
	#[diesel(
		deserialize_as = jiff_diesel::NullableTimestamp,
		serialize_as = jiff_diesel::NullableTimestamp,
		treat_none_as_default_value = false
	)]
	pub registered_at: Option<Timestamp>,
	/// Whether this server may manage its own DNS records for names under its
	/// group's domains. Withheld by default: a server without it is
	/// authenticated and refused. Unlike the restore window this is a standing
	/// grant, records needing maintenance for as long as the server lives.
	// spec: DOM#permission-for-a-server-to-manage-its-own-names
	pub may_manage_dns: bool,
	/// Whether this server may obtain TLS certificates for names under its
	/// group's domains. Separate from `may_manage_dns`: an application whose
	/// records are managed elsewhere may still want its certificates here.
	// spec: DOM#permission-for-a-server-to-manage-its-own-names
	pub may_manage_tls: bool,
	/// The certificate profile — the authority's name for a lifetime — this
	/// server's certificates are requested under. `None` means the longest the
	/// authority offers, which is every server until an operator says otherwise:
	/// a short lifetime is adopted deliberately rather than inherited.
	// spec: CRT#lifetime
	#[serde(skip_serializing_if = "Option::is_none")]
	#[diesel(treat_none_as_default_value = false)]
	pub certificate_profile: Option<String>,
	/// When this server's name management was paused. While set, Canopy makes no
	/// new changes on its behalf — nothing ordered, renewed, or republished —
	/// though nothing already in place is withdrawn.
	///
	/// Set automatically when one of the server's certificates is revoked, so
	/// revocation and re-issuance don't chase each other. Only an operator lifts
	/// it.
	// spec: CRT#pausing-a-server
	#[serde(skip_serializing_if = "Option::is_none")]
	#[diesel(
		deserialize_as = jiff_diesel::NullableTimestamp,
		serialize_as = jiff_diesel::NullableTimestamp,
		treat_none_as_default_value = false
	)]
	pub name_management_paused_at: Option<Timestamp>,
	/// Who paused it. `None` when Canopy paused it itself on a revocation.
	#[serde(skip_serializing_if = "Option::is_none")]
	#[diesel(treat_none_as_default_value = false)]
	pub name_management_paused_by: Option<String>,
	/// Why it was paused.
	#[serde(skip_serializing_if = "Option::is_none")]
	#[diesel(treat_none_as_default_value = false)]
	pub name_management_pause_reason: Option<String>,
}

impl Application {
	/// What this application is called.
	///
	/// A name is optional and an operator's alone to set, so an application
	/// nobody has named reads as its type. Every surface that has to flatten
	/// the name to a string goes through here, so an unnamed application reads
	/// the same wherever it appears.
	// spec: FLT#naming
	pub fn display_name(&self) -> String {
		self.name.clone().unwrap_or_else(|| self.r#type.label())
	}

	pub async fn get_all(
		db: &mut AsyncPgConnection,
		offset: u64,
		limit: Option<u64>,
	) -> Result<Vec<Self>> {
		use crate::schema::applications::dsl::*;
		let q = applications
			.select(Self::as_select())
			.filter(id.ne(Uuid::nil()))
			.filter(deleted_at.is_null())
			.order_by((
				name.is_not_null(),
				type_.asc(),
				name.asc(),
				created_at.desc(),
			))
			.offset(offset.try_into().unwrap_or(i64::MAX));

		if let Some(limit) = limit {
			q.limit(limit.try_into().unwrap_or(i64::MAX)).load(db).await
		} else {
			q.load(db).await
		}
		.map_err(AppError::from)
	}

	/// Archived (soft-deleted) applications, for the Archived view.
	pub async fn list_archived(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::applications::dsl::*;
		applications
			.select(Self::as_select())
			.filter(id.ne(Uuid::nil()))
			.filter(deleted_at.is_not_null())
			.order_by((type_.asc(), name.asc(), created_at.desc()))
			.load(db)
			.await
			.map_err(AppError::from)
	}

	pub async fn list_by_type(
		db: &mut AsyncPgConnection,
		k: ApplicationType,
		offset: u64,
		limit: Option<u64>,
	) -> Result<Vec<Self>> {
		use crate::schema::applications::dsl::*;
		let q = applications
			.select(Self::as_select())
			.filter(id.ne(Uuid::nil()).and(type_.eq(k)))
			.filter(deleted_at.is_null())
			.order_by((name.is_not_null(), name.asc(), created_at.desc()))
			.offset(offset.try_into().unwrap_or(i64::MAX));

		if let Some(limit) = limit {
			q.limit(limit.try_into().unwrap_or(i64::MAX)).load(db).await
		} else {
			q.load(db).await
		}
		.map_err(AppError::from)
	}

	pub async fn count_all(db: &mut AsyncPgConnection) -> Result<u64> {
		use crate::schema::applications::dsl::*;
		applications
			.count()
			.filter(id.ne(Uuid::nil()))
			.filter(deleted_at.is_null())
			.get_result(db)
			.await
			.map_err(AppError::from)
			.map(|n: i64| n.try_into().unwrap_or_default())
	}

	pub async fn count_by_type(db: &mut AsyncPgConnection, k: ApplicationType) -> Result<u64> {
		use crate::schema::applications::dsl::*;
		applications
			.count()
			.filter(id.ne(Uuid::nil()).and(type_.eq(k)))
			.filter(deleted_at.is_null())
			.get_result(db)
			.await
			.map_err(AppError::from)
			.map(|n: i64| n.try_into().unwrap_or_default())
	}

	pub async fn get_by_id(db: &mut AsyncPgConnection, id: Uuid) -> Result<Self> {
		crate::schema::applications::table
			.select(Self::as_select())
			.filter(crate::schema::applications::id.eq(id))
			.first(db)
			.await
			.map_err(AppError::from)
	}

	/// Like [`Application::get_by_id`] but takes a `FOR UPDATE` row lock. A caller
	/// inside a transaction uses this to serialise against concurrent archival
	/// (`soft_delete` locks the same row), closing the archive-vs-register
	/// TOCTOU at enrollment completion.
	pub async fn get_by_id_for_update(db: &mut AsyncPgConnection, id: Uuid) -> Result<Self> {
		crate::schema::applications::table
			.select(Self::as_select())
			.filter(crate::schema::applications::id.eq(id))
			.for_update()
			.first(db)
			.await
			.map_err(AppError::from)
	}

	pub async fn get_by_host(db: &mut AsyncPgConnection, host: String) -> Result<Self> {
		crate::schema::applications::table
			.select(Self::as_select())
			.filter(crate::schema::applications::host.eq(host))
			.first(db)
			.await
			.map_err(AppError::from)
	}

	/// Operator-driven insert. The caller pre-builds the row (id, defaults,
	/// optional URL). URLs are no longer unique, so there is no collision
	/// check.
	pub async fn create(db: &mut AsyncPgConnection, server: Application) -> Result<Self> {
		use crate::schema::applications;

		crate::tags::reject_reserved_keys(&server.tags)?;

		let created = diesel::insert_into(applications::table)
			.values(server)
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)?;
		// A new member can change the group's canonical version source.
		recompute_groups(db, [created.group_id]).await?;
		Ok(created)
	}

	/// The application a report describes, created if Canopy does not hold it.
	///
	/// A report is the only thing that creates an application, so a type
	/// reported on a machine that has none of it is adopted without ceremony
	/// rather than refused. Everything else about the new record is left for
	/// an operator: it takes its machine's group, because which group a
	/// box belongs to is the one fact the box cannot know, and nothing else.
	///
	/// Concurrent reports for one box are serialised by the caller on the
	/// machine row (see [`crate::machines::Machine::get_by_id_for_update`]),
	/// so two arriving together cannot both create.
	// spec: FLT#applications-come-from-reports
	pub async fn from_report(
		db: &mut AsyncPgConnection,
		machine: &crate::machines::Machine,
		r#type: &ApplicationType,
	) -> Result<Self> {
		use crate::schema::applications::dsl;

		let existing = dsl::applications
			.select(Self::as_select())
			.filter(dsl::machine_id.eq(machine.id))
			.filter(dsl::type_.eq(r#type.to_string()))
			.filter(dsl::deleted_at.is_null())
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)?;
		if let Some(existing) = existing {
			return Ok(existing);
		}

		Self::adopt(db, machine, r#type, None).await
	}

	/// The application a reporter's key names, created if Canopy does not hold
	/// it.
	///
	/// Correlation is on the machine, the key and the type together. A key
	/// Canopy already holds for this machine answers, as long as it still
	/// names an application of the reported type: a reporter that reports a
	/// different type under a key it was already using has stopped reporting
	/// one application and started reporting another, so the record it used to
	/// name gives the key up and stays as the application it is.
	///
	/// A key Canopy does not hold claims an unclaimed application of that type
	/// on the machine before creating one. That is what carries a box across
	/// the cutover: its applications came from unified pushes, which had no key
	/// to give them, and the first split-shape push naming one by type takes it
	/// over rather than standing up a duplicate beside it.
	///
	/// Concurrent reports for one box are serialised by the caller on the
	/// machine row (see [`crate::machines::Machine::get_by_id_for_update`]),
	/// so two arriving together cannot both create.
	///
	/// `create` is false for a source Canopy is ignoring, which reads what the
	/// key names without creating, claiming, or releasing anything. It answers
	/// `None` where a recording push would have created, an ignored reporter
	/// being told about the applications Canopy holds and no more.
	// spec: STA#identifying-an-application
	pub async fn from_report_key(
		db: &mut AsyncPgConnection,
		machine: &crate::machines::Machine,
		key: &str,
		r#type: &ApplicationType,
		create: bool,
	) -> Result<Option<Self>> {
		use crate::schema::applications::dsl;

		let live = dsl::applications
			.filter(dsl::machine_id.eq(machine.id))
			.filter(dsl::deleted_at.is_null());

		if let Some(held) = live
			.clone()
			.select(Self::as_select())
			.filter(dsl::reported_key.eq(key))
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)?
		{
			if &held.r#type == r#type {
				return Ok(Some(held));
			}
			if !create {
				return Ok(None);
			}
			diesel::update(dsl::applications.filter(dsl::id.eq(held.id)))
				.set(dsl::reported_key.eq(None::<String>))
				.execute(db)
				.await
				.map_err(AppError::from)?;
		}

		if let Some(unclaimed) = live
			.select(Self::as_select())
			.filter(dsl::type_.eq(r#type.to_string()))
			.filter(dsl::reported_key.is_null())
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)?
		{
			if !create {
				return Ok(Some(unclaimed));
			}
			return diesel::update(dsl::applications.filter(dsl::id.eq(unclaimed.id)))
				.set(dsl::reported_key.eq(key))
				.returning(Self::as_select())
				.get_result(db)
				.await
				.map(Some)
				.map_err(AppError::from);
		}

		if !create {
			return Ok(None);
		}

		Self::adopt(db, machine, r#type, Some(key.to_owned()))
			.await
			.map(Some)
	}

	/// Stand up the application a report describes. Everything about the new
	/// record beyond what the report said is left for an operator: it takes
	/// its machine's group, because which group a box belongs to is the
	/// one fact the box cannot know, and nothing else.
	async fn adopt(
		db: &mut AsyncPgConnection,
		machine: &crate::machines::Machine,
		r#type: &ApplicationType,
		key: Option<String>,
	) -> Result<Self> {
		Self::create(
			db,
			Self {
				id: Uuid::new_v4(),
				name: None,
				host: None,
				r#type: r#type.clone(),
				rank: None,
				machine_id: machine.id,
				reported_key: key,
				group_id: machine.group_id,
				public_name: None,
				cloud: machine.cloud,
				geolocation: machine.geolocation,
				is_monitored: true,
				alert_when_down_for: machine.alert_when_down_for,
				notes: String::new(),
				tags: TagMap::default(),
				deleted_at: None,
				registered_at: Some(Timestamp::now()),
				may_manage_dns: false,
				may_manage_tls: false,
				certificate_profile: None,
				name_management_paused_at: None,
				name_management_paused_by: None,
				name_management_pause_reason: None,
			},
		)
		.await
	}

	/// Archive an application: hide it from live listings and monitoring while
	/// retaining its history. Idempotent.
	///
	/// The box's identity is left alone. An identity speaks for the machine,
	/// and retiring one workload says nothing about the others on it or about
	/// the box itself; [`crate::machines::Machine::archive`] is what revokes.
	pub async fn soft_delete(db: &mut AsyncPgConnection, server_id: Uuid) -> Result<()> {
		use crate::schema::applications::dsl;
		use diesel_async::AsyncConnection;

		db.transaction::<_, AppError, _>(async |conn| {
			let server: Application = dsl::applications
				.select(Self::as_select())
				.filter(dsl::id.eq(server_id))
				.for_update()
				.first(conn)
				.await
				.map_err(AppError::from)?;

			if server.deleted_at.is_some() {
				return Ok(());
			}

			diesel::update(dsl::applications.filter(dsl::id.eq(server_id)))
				.set((
					dsl::deleted_at
						.eq(jiff_diesel::NullableTimestamp::from(Some(Timestamp::now()))),
					dsl::registered_at.eq(None::<jiff_diesel::Timestamp>),
				))
				.execute(conn)
				.await
				.map_err(AppError::from)?;

			// The server just dropped out of its group's live set, so the
			// group's cached headline version may now belong to someone else.
			recompute_groups(conn, [server.group_id]).await?;
			Ok(())
		})
		.await
	}

	/// Un-archive an application. Says nothing about its machine's identity,
	/// which archiving the application did not touch.
	pub async fn restore(db: &mut AsyncPgConnection, server_id: Uuid) -> Result<Self> {
		use crate::schema::applications::dsl;

		diesel::update(dsl::applications.filter(dsl::id.eq(server_id)))
			.set(dsl::deleted_at.eq(None::<jiff_diesel::Timestamp>))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		let restored = Self::get_by_id(db, server_id).await?;
		// Back in the live set: the group's canonical member may change.
		recompute_groups(db, [restored.group_id]).await?;
		Ok(restored)
	}

	/// Canonicalise a user-entered URL. A bare host (no scheme) defaults to
	/// `https://`, so operators can type `foo.example.com` and get
	/// `https://foo.example.com`.
	pub fn canonicalize_host(url: &str) -> Result<UrlField> {
		let url = url.trim();
		let candidate = if url.contains("://") {
			url.to_string()
		} else {
			format!("https://{url}")
		};
		Ok(UrlField(candidate.parse().map_err(|e| {
			AppError::BadRequest(format!("Invalid URL: {e}"))
		})?))
	}

	/// Map a hosting hint to the `cloud` flag.
	pub fn detect_cloud(hosting: &str) -> bool {
		matches!(
			hosting,
			"ec2" | "azure" | "gce" | "gcp" | "digitalocean" | "oracle" | "cloudstack"
		)
	}

	/// Live (non-archived) applications this identity speaks for.
	///
	/// An identity belongs to a machine, so the answer is what runs on that
	/// machine. A box with two workloads answers with both, which is the
	/// point: one agent reports for everything on its box.
	// spec: FLT#identities
	pub async fn live_by_device_id(db: &mut AsyncPgConnection, dev_id: Uuid) -> Result<Vec<Self>> {
		use crate::schema::{applications, machines};
		applications::table
			.inner_join(machines::table.on(machines::id.eq(applications::machine_id)))
			.select(Self::as_select())
			.filter(machines::device_id.eq(dev_id))
			.filter(machines::deleted_at.is_null())
			.filter(applications::deleted_at.is_null())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// This application's reachability, from when it last reported and its own
	/// down threshold.
	///
	/// The threshold lives here rather than at each call site so the indicator
	/// and the `reachability` check cannot be graded on different clocks, which
	/// is exactly how they came to disagree.
	///
	/// `last_reported_at` is when any source last reported on this
	/// application, from `ReportedDetail::last_reported_ats`, and is `None`
	/// only for one that has never reported. Reading it from status history
	/// instead capped the question at a lookback window, so an application
	/// quiet for longer than the window read as never heard from — grey, and
	/// outranking every other state — when it was the most thoroughly
	/// unreachable thing in the fleet.
	// spec: CHK#reachability
	pub fn reachability(&self, last_reported_at: Option<Timestamp>) -> ShortStatus {
		ShortStatus::grade(last_reported_at, self.alert_when_down_for.0)
	}

	// spec: CRT#pausing-a-server
	/// Whether Canopy is currently making no new changes on this server's behalf.
	pub fn name_management_paused(&self) -> bool {
		self.name_management_paused_at.is_some()
	}

	pub async fn pause_name_management(
		db: &mut AsyncPgConnection,
		server_id: Uuid,
		paused_by: Option<&str>,
		reason: &str,
	) -> Result<()> {
		use crate::schema::applications::dsl;
		diesel::update(dsl::applications.filter(dsl::id.eq(server_id)))
			.filter(dsl::name_management_paused_at.is_null())
			.set((
				dsl::name_management_paused_at
					.eq(jiff_diesel::NullableTimestamp::from(Some(Timestamp::now()))),
				dsl::name_management_paused_by.eq(paused_by),
				dsl::name_management_pause_reason.eq(Some(reason)),
			))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}

	/// Lift a pause, so the work resumes where it left off.
	///
	/// Only ever called for an operator: Canopy pauses itself on a revocation but
	/// never un-pauses itself, however long the pause has stood and however much
	/// is expiring under it. Deciding it is safe to start again is a judgement
	/// Canopy is not in a position to make.
	// spec: CRT#pausing-a-server
	pub async fn resume_name_management(db: &mut AsyncPgConnection, server_id: Uuid) -> Result<()> {
		use crate::schema::applications::dsl;
		diesel::update(dsl::applications.filter(dsl::id.eq(server_id)))
			.set((
				dsl::name_management_paused_at.eq(jiff_diesel::NullableTimestamp::from(None)),
				dsl::name_management_paused_by.eq::<Option<String>>(None),
				dsl::name_management_pause_reason.eq::<Option<String>>(None),
			))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}

	/// Set (or clear) the profile this server's certificates are requested under.
	///
	/// `None` means the authority's own default, which is its longest-lived: a
	/// short lifetime is something adopted deliberately for a server rather than a
	/// default anyone inherits. Takes effect on the next issuance or renewal; a
	/// certificate already held keeps the lifetime it was issued with.
	// spec: CRT#lifetime
	pub async fn set_certificate_profile(
		db: &mut AsyncPgConnection,
		server_id: Uuid,
		profile: Option<&str>,
	) -> Result<()> {
		use crate::schema::applications::dsl;
		let updated = diesel::update(dsl::applications.filter(dsl::id.eq(server_id)))
			.filter(dsl::deleted_at.is_null())
			.set(dsl::certificate_profile.eq(profile))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		if updated == 0 {
			return Err(AppError::DatabaseQuery(diesel::result::Error::NotFound));
		}
		Ok(())
	}

	/// Every application type present on a live application, so a surface
	/// listing types can offer the ones the fleet actually uses as well as the
	/// ones Canopy has handling for. The set is open, so the catalogue cannot
	/// be a constant.
	// spec: APP#where-a-type-comes-from
	pub async fn distinct_types(db: &mut AsyncPgConnection) -> Result<Vec<ApplicationType>> {
		use crate::schema::applications::dsl;
		let rows: Vec<String> = dsl::applications
			.select(dsl::type_)
			.filter(dsl::deleted_at.is_null())
			.distinct()
			.load(db)
			.await
			.map_err(AppError::from)?;
		// A stored value that does not parse is not a type this build can say
		// anything about; leaving it out of the catalogue is the same as any
		// other unknown.
		Ok(rows.into_iter().filter_map(|t| t.parse().ok()).collect())
	}

	/// Every application this identity speaks for, archived ones included.
	// spec: FLT#identities
	pub async fn get_by_device_id(db: &mut AsyncPgConnection, dev_id: Uuid) -> Result<Vec<Self>> {
		use crate::schema::{applications, machines};
		applications::table
			.inner_join(machines::table.on(machines::id.eq(applications::machine_id)))
			.select(Self::as_select())
			.filter(machines::device_id.eq(dev_id))
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// The type of each of these applications, for resolving the namespace of a
	/// check filed against them. Applications whose stored type does not parse
	/// are absent, the same as any other unknown type.
	pub async fn types_by_id(
		db: &mut AsyncPgConnection,
		ids: &[Uuid],
	) -> Result<std::collections::HashMap<Uuid, ApplicationType>> {
		use crate::schema::applications::dsl;
		if ids.is_empty() {
			return Ok(Default::default());
		}
		let rows: Vec<(Uuid, String)> = dsl::applications
			.select((dsl::id, dsl::type_))
			.filter(dsl::id.eq_any(ids))
			.load(db)
			.await
			.map_err(AppError::from)?;
		Ok(rows
			.into_iter()
			.filter_map(|(id, t)| Some((id, t.parse().ok()?)))
			.collect())
	}

	/// Which box each of these applications runs on.
	///
	/// The application's rollup takes in its machine's checks, so every read of
	/// a set of applications needs their boxes too; asking once beats a lookup
	/// per application on the fleet list.
	// spec: CHK#a-machines-checks-present-on-its-applications
	pub async fn machines_by_id(
		db: &mut AsyncPgConnection,
		ids: &[Uuid],
	) -> Result<std::collections::HashMap<Uuid, Uuid>> {
		use crate::schema::applications::dsl;
		if ids.is_empty() {
			return Ok(Default::default());
		}
		let rows: Vec<(Uuid, Uuid)> = dsl::applications
			.select((dsl::id, dsl::machine_id))
			.filter(dsl::id.eq_any(ids))
			.load(db)
			.await
			.map_err(AppError::from)?;
		Ok(rows.into_iter().collect())
	}

	pub async fn get_by_ids(db: &mut AsyncPgConnection, ids: &[Uuid]) -> Result<Vec<Self>> {
		use crate::schema::applications::dsl::*;
		applications
			.select(Self::as_select())
			.filter(id.eq_any(ids))
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// All applications in the same group as `self`, excluding `self`. If the
	/// server is ungrouped, returns an empty Vec.
	pub async fn siblings(&self, db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::applications::dsl::*;
		let Some(gid) = self.group_id else {
			return Ok(Vec::new());
		};
		applications
			.select(Self::as_select())
			.filter(group_id.eq(gid))
			.filter(id.ne(self.id))
			.filter(deleted_at.is_null())
			.order(name.asc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// All live (non-archived) applications in a group, ordered by name. Used to
	/// expand a group-wide restore-replica declaration into per-server entries.
	pub async fn list_live_in_group(
		db: &mut AsyncPgConnection,
		group_id_: Uuid,
	) -> Result<Vec<Self>> {
		use crate::schema::applications::dsl::*;
		applications
			.select(Self::as_select())
			.filter(group_id.eq(group_id_))
			.filter(id.ne(Uuid::nil()))
			.filter(deleted_at.is_null())
			.order(name.asc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// All applications without a group, ordered by name. The fleet is browsed by
	/// group and has no ungrouped listing; this is for the recovery snapshot, which
	/// sweeps up everything so nothing is left out of the record.
	pub async fn list_ungrouped(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::applications::dsl::*;
		applications
			.select(Self::as_select())
			.filter(group_id.is_null())
			.filter(id.ne(Uuid::nil()))
			.filter(deleted_at.is_null())
			.order(name.asc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Bulk-fetch `(name, host)` for a set of server ids — used by the
	/// issues/incidents APIs to embed display info into each row so the UI
	/// doesn't have to fetch every server independently.
	pub async fn names_by_ids(
		db: &mut AsyncPgConnection,
		ids: &[Uuid],
	) -> Result<std::collections::HashMap<Uuid, (Option<String>, Option<String>)>> {
		use crate::schema::applications::dsl;

		if ids.is_empty() {
			return Ok(std::collections::HashMap::new());
		}
		let rows: Vec<(Uuid, Option<String>, Option<String>)> = dsl::applications
			.select((dsl::id, dsl::name, dsl::host))
			.filter(dsl::id.eq_any(ids))
			.load(db)
			.await
			.map_err(AppError::from)?;
		Ok(rows.into_iter().map(|(i, n, h)| (i, (n, h))).collect())
	}

	/// Bulk-fetch the group name for each given server. Servers that are
	/// ungrouped (or don't exist) get `None`.
	pub async fn group_names_by_server_ids(
		db: &mut AsyncPgConnection,
		ids: &[Uuid],
	) -> Result<std::collections::HashMap<Uuid, Option<String>>> {
		use crate::schema::{applications, server_groups};
		use std::collections::HashMap;

		if ids.is_empty() {
			return Ok(HashMap::new());
		}
		let rows: Vec<(Uuid, Option<String>)> = applications::table
			.left_join(
				server_groups::table.on(server_groups::id.nullable().eq(applications::group_id)),
			)
			.select((applications::id, server_groups::name.nullable()))
			.filter(applications::id.eq_any(ids))
			.load(db)
			.await
			.map_err(AppError::from)?;
		Ok(rows.into_iter().collect())
	}

	/// Bulk-fetch `(group_id, group_name)` for each given server. Servers
	/// that are ungrouped (or don't exist) get `(None, None)`.
	pub async fn group_refs_by_server_ids(
		db: &mut AsyncPgConnection,
		ids: &[Uuid],
	) -> Result<std::collections::HashMap<Uuid, (Option<Uuid>, Option<String>)>> {
		use crate::schema::{applications, server_groups};
		use std::collections::HashMap;

		if ids.is_empty() {
			return Ok(HashMap::new());
		}
		let rows: Vec<(Uuid, Option<Uuid>, Option<String>)> = applications::table
			.left_join(
				server_groups::table.on(server_groups::id.nullable().eq(applications::group_id)),
			)
			.select((
				applications::id,
				applications::group_id,
				server_groups::name.nullable(),
			))
			.filter(applications::id.eq_any(ids))
			.load(db)
			.await
			.map_err(AppError::from)?;
		Ok(rows
			.into_iter()
			.map(|(id, gid, gn)| (id, (gid, gn)))
			.collect())
	}

	pub async fn search_central(
		db: &mut AsyncPgConnection,
		query: &str,
		limit: i64,
	) -> Result<Vec<Self>> {
		use crate::schema::applications::dsl::*;
		let search_pattern = format!("%{}%", query);

		// One filter, because eligibility is one fact about the type. It used
		// to take two — a product that lists publicly, and a central — which
		// between them said `tamanu-central` without being able to name it.
		// spec: APP#capabilities
		let mut query_builder = applications
			.select(Self::as_select())
			.filter(type_.eq_any(ApplicationType::stored_values_where(|t| {
				t.caps().public_listing
			})))
			.filter(public_name.is_not_null())
			.filter(deleted_at.is_null())
			.into_boxed();

		if let Ok(query_uuid) = query.parse::<Uuid>() {
			query_builder = query_builder.filter(
				name.ilike(&search_pattern)
					.or(host.ilike(&search_pattern))
					.or(id.eq(query_uuid)),
			);
		} else {
			query_builder =
				query_builder.filter(name.ilike(&search_pattern).or(host.ilike(&search_pattern)));
		}

		query_builder
			.limit(limit)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	pub async fn update(
		db: &mut AsyncPgConnection,
		server_id: Uuid,
		updates: PartialServer,
	) -> Result<Self> {
		use crate::schema::applications::dsl;

		if let Some(tags) = &updates.tags {
			crate::tags::reject_reserved_keys(tags)?;
		}

		// Capture the old group before the update: rank/kind/group_id may all
		// change, so both the old and new group's canonical member can shift.
		// Non-fatal: a missing server (or read error) just means "no old group
		// to recompute" and leaves the update's own error handling — e.g. the
		// empty-changeset path — to set the response, unchanged by us.
		let old_group_id = Self::get_by_id(db, server_id)
			.await
			.ok()
			.and_then(|s| s.group_id);

		// An empty changeset is a no-op, not an error. It became reachable when
		// the group moved to the machine: an edit that changes only the group
		// leaves nothing here to write, and diesel refuses to build that query.
		match diesel::update(dsl::applications.filter(dsl::id.eq(server_id)))
			.set(updates)
			.execute(db)
			.await
		{
			Ok(_) => {}
			Err(diesel::result::Error::QueryBuilderError(_)) => {}
			Err(err) => return Err(AppError::from(err)),
		}

		let after = Self::get_by_id(db, server_id).await?;
		recompute_groups(db, [old_group_id, after.group_id]).await?;
		Ok(after)
	}

	/// Set or clear the server's group. On a `None → Some(group)` transition,
	/// the server's currently-open issues get re-evaluated against the new
	/// group so any that warrant promotion to an incident do so. The clear
	/// case is the simpler direction: the server's open issues stay, but
	/// they no longer contribute to a group-level incident (the existing
	/// incident's other-server contributors keep it alive on their own).
	pub async fn assign_to_group(
		db: &mut AsyncPgConnection,
		server_id: Uuid,
		new_group_id: Option<Uuid>,
	) -> Result<Self> {
		use crate::schema::applications::dsl;

		let before = Self::get_by_id(db, server_id).await?;
		diesel::update(dsl::applications.filter(dsl::id.eq(server_id)))
			.set(dsl::group_id.eq(new_group_id))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		let after = Self::get_by_id(db, server_id).await?;

		if before.group_id.is_none() && new_group_id.is_some() {
			// Promote currently-open issues now that this server has somewhere
			// to attach an incident to.
			crate::issues::reevaluate_open_issues_for_server(db, server_id).await?;
		}

		recompute_groups(db, [before.group_id, new_group_id]).await?;

		Ok(after)
	}

	/// Tags as seen by the public-server tags endpoint: the group's tags
	/// with the server's own tags overlaid (server wins on key collision).
	/// If the server is ungrouped, returns just the server's tags.
	pub async fn tags_merged_with_group(&self, db: &mut AsyncPgConnection) -> Result<TagMap> {
		let Some(gid) = self.group_id else {
			return Ok(self.tags.clone());
		};
		let group = crate::server_groups::ServerGroup::get_by_id(db, gid).await?;
		Ok(self.tags.merged_with(&group.tags))
	}

	/// Tags served to the device by the public `/tags` endpoint: the merged
	/// server+group tags (see [`Self::tags_merged_with_group`]) plus synthetic
	/// read-only tags describing the server itself, under the reserved
	/// [`RESERVED_TAG_PREFIX`] namespace:
	///
	/// - `canopy:type` — the application's [`ApplicationType`] (always present).
	/// - `canopy:product` / `canopy:kind` — the type split back into the pair
	///   these carried before, so a rule written against either keeps
	///   matching. Deprecated in favour of `canopy:type`.
	/// - `canopy:rank` — the server's [`ServerRank`], only when one is set.
	/// - `canopy:group-id` / `canopy:group-name` — only when grouped.
	///
	/// Operator-set tags can't use the `canopy:` prefix (rejected on write),
	/// so these never collide with stored tags.
	pub async fn tags_for_device(&self, db: &mut AsyncPgConnection) -> Result<TagMap> {
		let group = match self.group_id {
			Some(gid) => Some(crate::server_groups::ServerGroup::get_by_id(db, gid).await?),
			None => None,
		};

		let mut tags = match &group {
			Some(group) => self.tags.merged_with(&group.tags),
			None => self.tags.clone(),
		};

		// The type is what an application is, and what a rule or silence
		// written against the classification should match on.
		// spec: APP#where-a-type-comes-from
		tags.0.insert(
			format!("{RESERVED_TAG_PREFIX}type"),
			self.r#type.to_string(),
		);
		// Both halves of the old pair stay emitted, derived from the type. An
		// agent or an operator rule reading `canopy:product` or `canopy:kind`
		// keeps working across the transition; dropping either would break it
		// silently rather than loudly. Deprecated: `canopy:type` is the one to
		// read.
		// spec: API#surfaces-the-definition-does-not-reach
		tags.0.insert(
			format!("{RESERVED_TAG_PREFIX}product"),
			self.r#type.software().to_string(),
		);
		tags.0.insert(
			format!("{RESERVED_TAG_PREFIX}kind"),
			self.r#type.role().to_string(),
		);
		if let Some(rank) = self.rank {
			tags.0
				.insert(format!("{RESERVED_TAG_PREFIX}rank"), rank.to_string());
		}
		if let Some(group) = &group {
			tags.0.insert(
				format!("{RESERVED_TAG_PREFIX}group-id"),
				group.id.to_string(),
			);
			tags.0.insert(
				format!("{RESERVED_TAG_PREFIX}group-name"),
				group.name.clone(),
			);
		}

		Ok(tags)
	}
}

#[test]
fn canonicalize_host_defaults_to_https() {
	let h = |s: &str| Application::canonicalize_host(s).unwrap().0.to_string();
	assert_eq!(h("foo.example.com"), "https://foo.example.com/");
	assert_eq!(h("  bar.example.com  "), "https://bar.example.com/");
	assert_eq!(h("http://insecure.example"), "http://insecure.example/");
	assert_eq!(h("https://full.example/path"), "https://full.example/path");
}

#[test]
fn test_server_serialization() {
	use jiff::SignedDuration;

	let server = Application {
		id: Uuid::nil(),
		name: Some("Test Application".to_string()),
		r#type: ApplicationType::TamanuCentral,
		rank: Some(ServerRank::Production),
		host: Some(UrlField("https://example.com/".parse().unwrap())),
		machine_id: Uuid::nil(),
		reported_key: None,
		group_id: None,
		public_name: Some("Test Application".to_string()),
		cloud: None,
		geolocation: None,
		is_monitored: true,
		alert_when_down_for: PgDuration(SignedDuration::from_secs(600)),
		notes: String::new(),
		tags: TagMap::default(),
		deleted_at: None,
		registered_at: None,
		may_manage_dns: false,
		may_manage_tls: false,
		certificate_profile: None,
		name_management_paused_at: None,
		name_management_paused_by: None,
		name_management_pause_reason: None,
	};

	let serialized = serde_json::to_string_pretty(&server).unwrap();
	assert_eq!(
		serialized,
		r#"{
  "id": "00000000-0000-0000-0000-000000000000",
  "name": "Test Application",
  "host": "https://example.com",
  "type": "tamanu-central",
  "rank": "production",
  "machine_id": "00000000-0000-0000-0000-000000000000",
  "public_name": "Test Application",
  "is_monitored": true,
  "alert_when_down_for": 600,
  "notes": "",
  "tags": {},
  "may_manage_dns": false,
  "may_manage_tls": false
}"#
	);
}

/// Fields to update on an existing server. Only the fields present are
/// changed; omitted fields are left as-is. For the fields that are
/// themselves optional (`host`, `group_id`, `public_name`,
/// `cloud`, `geolocation`), sending an explicit `null` clears the value,
/// while omitting the field entirely leaves it unchanged.
#[derive(Debug, Deserialize, AsChangeset, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::applications)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct PartialServer {
	/// The server to update.
	pub id: Uuid,
	/// New display name for the server.
	pub name: Option<String>,
	/// New environment tier for the server, for example production, test,
	/// or dev.
	#[diesel(deserialize_as = String, serialize_as = String)]
	pub rank: Option<ServerRank>,
	/// New URL for the server, or `null` to clear it.
	pub host: Option<Option<UrlField>>,
	/// New server group for the server, or `null` to remove it from its
	/// current group.
	pub group_id: Option<Option<Uuid>>,
	/// New public listing name for the server, or `null` to unlist it.
	pub public_name: Option<Option<String>>,
	/// New value for whether the server is cloud-hosted, or `null` to clear
	/// it.
	pub cloud: Option<Option<bool>>,
	/// New physical location for the server, or `null` to clear it.
	pub geolocation: Option<Option<GeoPoint>>,
	/// New monitored state for the server.
	pub is_monitored: Option<bool>,
	/// New downtime threshold for the server, in seconds.
	#[schema(value_type = Option<i64>)]
	#[diesel(serialize_as = PgDuration)]
	pub alert_when_down_for: Option<PgDuration>,
	/// New free-form operator notes for the server.
	pub notes: Option<String>,
	/// New set of key/value tags for the server. This replaces the whole
	/// tag set.
	pub tags: Option<TagMap>,
	/// Whether the server may manage its own DNS records under its group's
	/// domains.
	pub may_manage_dns: Option<bool>,
	/// Whether the server may obtain TLS certificates for names under its
	/// group's domains.
	pub may_manage_tls: Option<bool>,
}
