//! Operator-managed silences, stored as scoped check policies.
//!
//! A silence is a scoped transform with a `skipped` ceiling (see
//! [`crate::check_policies::ScopedCheckPolicy`]): the matching check
//! keeps recording its observed results, but its effective result is
//! skipped, so it raises nothing and counts nowhere —
//! [`crate::issues::re_evaluate_incident_membership`] treats it as a
//! "should leave" reason, and the health rollups drop it.
//!
//! This module keeps the historical (source, ref) surface the private
//! API and UI speak: refs carry the `health/` namespace prefix for
//! source-reported checks, while the scoped-policy storage is keyed by
//! bare check name. The mapping is applied on the way in and out.

use std::collections::BTreeSet;

use commons_errors::{AppError, Result};
use commons_types::namespace::{Namespace, NamespaceRef};
use commons_types::server::app_type::ApplicationType;
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::applications::Application;
use crate::check_policies::ScopedCheckPolicy;
use crate::issues::{
	MANUAL_SOURCE, Scope, reevaluate_open_issues_for_group_ref,
	reevaluate_open_issues_for_machine_ref, reevaluate_open_issues_for_server_ref,
};
use crate::statuses::CANOPY_SOURCE;

/// The ref prefix (with trailing separator) healthcheck issues use,
/// whichever source reports them. Mirrors the public-server's `HEALTH_REF`.
const HEALTH_REF_PREFIX: &str = "health/";

/// The check name a silence ref maps to: refs of source-reported checks
/// carry the `health/` namespace prefix, canopy/manual refs are already
/// bare check names.
fn ref_to_check(r#ref: &str) -> &str {
	r#ref.strip_prefix(HEALTH_REF_PREFIX).unwrap_or(r#ref)
}

/// The ref a silenced check name presents as: reserved sources file at
/// bare refs, everything else under the `health/` namespace.
fn check_to_ref(source: &str, check: &str) -> String {
	if source == CANOPY_SOURCE || source == MANUAL_SOURCE {
		check.to_string()
	} else {
		format!("{HEALTH_REF_PREFIX}{check}")
	}
}

/// The namespace a silence names, from the check's name and whatever the
/// target can say about the application type.
///
/// A curated source's names are flat, so nothing needs to be known. A
/// structured source's machine-subject check is in the machine namespace,
/// which no type bears on. Only a structured source's application-subject
/// check needs one, and where it comes from follows the scope: an
/// application-scoped silence reads it off the application, a group-scoped one
/// takes it from the operator (who silenced the check while looking at one),
/// and a machine-scoped one has none. A machine-scoped silence never reaches
/// here: a box Canopy holds no application for files everything as its own, so
/// every check at that scope is in the machine namespace whatever its name,
/// and [`Namespace::for_machine`] answers without needing a type.
fn namespace_for(
	source: &str,
	check: &str,
	application_type: Option<&ApplicationType>,
) -> Result<Namespace> {
	Namespace::of(source, check, application_type).ok_or_else(|| {
		AppError::Custom(format!(
			"{check} from {source} is an application check, so silencing it needs an application type"
		))
	})
}

/// The application type of the application a silence is scoped to, for
/// resolving the check's namespace.
async fn type_of(db: &mut AsyncPgConnection, application_id: Uuid) -> Result<ApplicationType> {
	Ok(Application::get_by_id(db, application_id).await?.r#type)
}

/// A silenced issue reference scoped to a single server: issues matching
/// this `(source, ref)` on this server are still recorded, but are excluded
/// from incidents and notifications.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ServerSilencedRef {
	/// The server this silence applies to.
	pub application_id: Uuid,
	/// The issue source this silence matches.
	pub source: String,
	/// The issue reference this silence matches.
	#[serde(rename = "ref")]
	pub r#ref: String,
	/// When this silence was created.
	pub created_at: Timestamp,
	/// The operator who created this silence. `None` if not recorded.
	pub created_by: Option<String>,
}

/// A silenced issue reference scoped to an entire server group: issues
/// matching this `(source, ref)` on any server in the group (or raised
/// directly against the group) are still recorded, but are excluded from
/// incidents and notifications.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ServerGroupSilencedRef {
	/// The server group this silence applies to.
	pub server_group_id: Uuid,
	/// The issue source this silence matches.
	pub source: String,
	/// The issue reference this silence matches.
	#[serde(rename = "ref")]
	pub r#ref: String,
	/// Which catalog entry this silence quiets. A group covers several
	/// application types, so two of them reporting one check name are two
	/// silences here, and the ref alone does not tell them apart.
	pub namespace: NamespaceRef,
	/// When this silence was created.
	pub created_at: Timestamp,
	/// The operator who created this silence. `None` if not recorded.
	pub created_by: Option<String>,
}

/// A silenced issue reference scoped to a single machine: issues matching this
/// `(source, ref)` on this box are still recorded, but are excluded from
/// incidents and notifications.
///
/// A box's own checks are the subject here — a full disk, a drifting clock —
/// not those of the applications running on it, which are silenced against
/// each application.
// spec: CHK#silences-follow-the-event
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct MachineSilencedRef {
	/// The machine this silence applies to.
	pub machine_id: Uuid,
	/// The issue source this silence matches.
	pub source: String,
	/// The issue reference this silence matches.
	#[serde(rename = "ref")]
	pub r#ref: String,
	/// When this silence was created.
	pub created_at: Timestamp,
	/// The operator who created this silence. `None` if not recorded.
	pub created_by: Option<String>,
}

/// Is a silence in force for `(source, ref)` on an event at this scope?
///
/// An event can be silenced at its own scope and at its group's. Which "its
/// own" is follows the event: a machine's checks are silenced against the
/// machine, an application's against the application. Silencing a check
/// everywhere is not a silence at all but the check's own ceiling, so no scope
/// above the group is consulted here.
// spec: CHK#silences-follow-the-event
pub async fn is_silenced(
	db: &mut AsyncPgConnection,
	scope: Scope,
	group_id: Option<Uuid>,
	source: &str,
	r#ref: &str,
) -> Result<bool> {
	let check = ref_to_check(r#ref);
	let is_silence =
		|p: Option<ScopedCheckPolicy>| p.is_some_and(|p| p.ceiling.as_deref() == Some("skipped"));
	// The event's own scope, when it has one below the group.
	let namespace = match scope {
		Scope::Application(id) => namespace_for(source, check, Some(&type_of(db, id).await?))?,
		Scope::Machine(_) => Namespace::for_machine(source, check),
		_ => namespace_for(source, check, None)?,
	};
	if matches!(scope, Scope::Application(_) | Scope::Machine(_))
		&& is_silence(ScopedCheckPolicy::get(db, scope, source, &namespace, check).await?)
	{
		return Ok(true);
	}
	let Some(gid) = group_id else {
		return Ok(false);
	};
	Ok(is_silence(
		ScopedCheckPolicy::get(db, Scope::Group(gid), source, &namespace, check).await?,
	))
}

/// Check names silenced for a server under one reporting source, at
/// either server or group scope. `group_id` is the server's current
/// group; pass `None` if ungrouped. A check's identity is the (source,
/// check) pair, so a silence on another source's same-named check never
/// applies.
///
/// This feeds the consolidated check readers and the device-facing
/// effective check map: a silenced check keeps recording results but is
/// presented as skipped and doesn't count toward the server's health
/// rollup.
pub async fn silenced_health_checks_for_server(
	db: &mut AsyncPgConnection,
	application_id: Option<Uuid>,
	machine_id: Option<Uuid>,
	group_id: Option<Uuid>,
	source: &str,
) -> Result<BTreeSet<String>> {
	use crate::schema::scoped_check_policies::dsl;

	// A reporter pushes both grains' checks and gets one answer back, so this
	// covers the machine as well. Without it a machine check silenced by an
	// operator would keep being run and reported: the silence would hold on
	// canopy's side and be invisible to the agent.
	// spec: STA
	// A group-scoped silence is shared by every namespace filing under that
	// group, so it is narrowed to the ones this reporter can file into: the
	// machine's, its own application type's, and the flat one a curated source
	// uses. Without that, silencing one application type's check would silence
	// its namesake on every other type in the group.
	//
	// A box Canopy holds no application for files everything as the machine's,
	// so there is no type to narrow by and no application scope to read: it
	// gets the machine's silences and its group's unqualified ones.
	let application_type = match application_id {
		Some(id) => Some(type_of(db, id).await?.to_string()),
		None => None,
	};
	let rows: Vec<String> = dsl::scoped_check_policies
		.select(dsl::check_name)
		.filter(dsl::ceiling.eq("skipped"))
		.filter(dsl::source.eq(source))
		.filter(
			dsl::subject
				.is_null()
				.or(dsl::subject.is_not_distinct_from(commons_types::namespace::SUBJECT_MACHINE))
				.or(dsl::subject
					.is_not_distinct_from(commons_types::namespace::SUBJECT_APPLICATION)
					.and(dsl::application_type.is_not_distinct_from(application_type))
					.and(dsl::application_type.is_not_null())),
		)
		.filter(
			dsl::application_id
				.is_not_distinct_from(application_id)
				.and(dsl::application_id.is_not_null())
				.or(dsl::machine_id
					.is_not_distinct_from(machine_id)
					.and(dsl::machine_id.is_not_null()))
				.or(dsl::server_group_id
					.is_not_distinct_from(group_id)
					.and(dsl::server_group_id.is_not_null())),
		)
		.load(db)
		.await?;
	Ok(rows.into_iter().collect())
}

impl ServerSilencedRef {
	fn from_policy(p: ScopedCheckPolicy) -> Option<Self> {
		Some(Self {
			application_id: p.application_id?,
			r#ref: check_to_ref(&p.source, &p.check_name),
			source: p.source,
			created_at: p.created_at,
			created_by: p.created_by,
		})
	}

	/// Add a server-scoped silence and re-evaluate any currently-open
	/// matching issues so they leave their incident. Idempotent.
	pub async fn add(
		db: &mut AsyncPgConnection,
		application_id: Uuid,
		source: &str,
		r#ref: &str,
		created_by: Option<&str>,
	) -> Result<Self> {
		let check = ref_to_check(r#ref);
		let namespace = namespace_for(source, check, Some(&type_of(db, application_id).await?))?;
		let policy = ScopedCheckPolicy::silence(
			db,
			Scope::Application(application_id),
			source,
			&namespace,
			check,
			created_by,
		)
		.await?;
		reevaluate_open_issues_for_server_ref(db, application_id, source, r#ref).await?;
		Ok(Self::from_policy(policy).expect("server-scoped silence has a application_id"))
	}

	/// Remove a server-scoped silence and re-evaluate any currently-open
	/// matching issues so they (re)join an incident if eligible.
	pub async fn remove(
		db: &mut AsyncPgConnection,
		application_id: Uuid,
		source: &str,
		r#ref: &str,
	) -> Result<()> {
		let check = ref_to_check(r#ref);
		let namespace = namespace_for(source, check, Some(&type_of(db, application_id).await?))?;
		ScopedCheckPolicy::unsilence(
			db,
			Scope::Application(application_id),
			source,
			&namespace,
			check,
		)
		.await?;
		reevaluate_open_issues_for_server_ref(db, application_id, source, r#ref).await?;
		Ok(())
	}

	pub async fn list_for_server(
		db: &mut AsyncPgConnection,
		application_id: Uuid,
	) -> Result<Vec<Self>> {
		Ok(
			ScopedCheckPolicy::list_silences(db, Scope::Application(application_id))
				.await?
				.into_iter()
				.filter_map(Self::from_policy)
				.collect(),
		)
	}

	/// Every application-scoped silence across these applications.
	///
	/// The plural of [`Self::list_for_server`], for the machine's edit form:
	/// one form holds a section per application on the box, and each section's
	/// unreachability switch is that application's own silence. Asking per
	/// section would put a round trip on every workload.
	// spec: FLT#navigating-the-two-grains
	pub async fn list_for_servers(
		db: &mut AsyncPgConnection,
		application_ids: &[Uuid],
	) -> Result<Vec<Self>> {
		use crate::schema::scoped_check_policies::dsl;
		use commons_types::status::CheckResult;
		if application_ids.is_empty() {
			return Ok(Vec::new());
		}
		let rows: Vec<ScopedCheckPolicy> = dsl::scoped_check_policies
			.select(ScopedCheckPolicy::as_select())
			.filter(dsl::application_id.eq_any(application_ids))
			.filter(dsl::ceiling.eq(CheckResult::Skipped.to_string()))
			.order(dsl::created_at.desc())
			.load(db)
			.await
			.map_err(AppError::from)?;
		Ok(rows.into_iter().filter_map(Self::from_policy).collect())
	}
}

impl ServerGroupSilencedRef {
	fn from_policy(p: ScopedCheckPolicy) -> Option<Self> {
		Some(Self {
			server_group_id: p.server_group_id?,
			namespace: (&p.namespace().ok()?).into(),
			r#ref: check_to_ref(&p.source, &p.check_name),
			source: p.source,
			created_at: p.created_at,
			created_by: p.created_by,
		})
	}

	/// Add a group-scoped silence. `application_type` names which type's check is
	/// meant, and is required for an application-subject check from a structured
	/// source: the operator silences group-wide from one server's check row, so
	/// the caller knows the type even though the group covers several.
	pub async fn add(
		db: &mut AsyncPgConnection,
		server_group_id: Uuid,
		source: &str,
		r#ref: &str,
		application_type: Option<&ApplicationType>,
		created_by: Option<&str>,
	) -> Result<Self> {
		let check = ref_to_check(r#ref);
		let namespace = namespace_for(source, check, application_type)?;
		let policy = ScopedCheckPolicy::silence(
			db,
			Scope::Group(server_group_id),
			source,
			&namespace,
			check,
			created_by,
		)
		.await?;
		reevaluate_open_issues_for_group_ref(db, server_group_id, source, r#ref).await?;
		Ok(Self::from_policy(policy).expect("group-scoped silence has a server_group_id"))
	}

	pub async fn remove(
		db: &mut AsyncPgConnection,
		server_group_id: Uuid,
		source: &str,
		r#ref: &str,
		application_type: Option<&ApplicationType>,
	) -> Result<()> {
		let check = ref_to_check(r#ref);
		let namespace = namespace_for(source, check, application_type)?;
		ScopedCheckPolicy::unsilence(db, Scope::Group(server_group_id), source, &namespace, check)
			.await?;
		reevaluate_open_issues_for_group_ref(db, server_group_id, source, r#ref).await?;
		Ok(())
	}

	pub async fn list_for_group(
		db: &mut AsyncPgConnection,
		server_group_id: Uuid,
	) -> Result<Vec<Self>> {
		Ok(
			ScopedCheckPolicy::list_silences(db, Scope::Group(server_group_id))
				.await?
				.into_iter()
				.filter_map(Self::from_policy)
				.collect(),
		)
	}
}

impl MachineSilencedRef {
	fn from_policy(p: ScopedCheckPolicy) -> Option<Self> {
		Some(Self {
			machine_id: p.machine_id?,
			r#ref: check_to_ref(&p.source, &p.check_name),
			source: p.source,
			created_at: p.created_at,
			created_by: p.created_by,
		})
	}

	/// Add a machine-scoped silence and re-evaluate any currently-open matching
	/// issues so they leave their incident. Idempotent.
	pub async fn add(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
		source: &str,
		r#ref: &str,
		created_by: Option<&str>,
	) -> Result<Self> {
		let check = ref_to_check(r#ref);
		let namespace = Namespace::for_machine(source, check);
		let policy = ScopedCheckPolicy::silence(
			db,
			Scope::Machine(machine_id),
			source,
			&namespace,
			check,
			created_by,
		)
		.await?;
		reevaluate_open_issues_for_machine_ref(db, machine_id, source, r#ref).await?;
		Ok(Self::from_policy(policy).expect("machine-scoped silence has a machine_id"))
	}

	/// Remove a machine-scoped silence and re-evaluate any currently-open
	/// matching issues so they (re)join an incident if eligible.
	pub async fn remove(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
		source: &str,
		r#ref: &str,
	) -> Result<()> {
		let check = ref_to_check(r#ref);
		let namespace = Namespace::for_machine(source, check);
		ScopedCheckPolicy::unsilence(db, Scope::Machine(machine_id), source, &namespace, check)
			.await?;
		reevaluate_open_issues_for_machine_ref(db, machine_id, source, r#ref).await?;
		Ok(())
	}

	pub async fn list_for_machine(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
	) -> Result<Vec<Self>> {
		Ok(
			ScopedCheckPolicy::list_silences(db, Scope::Machine(machine_id))
				.await?
				.into_iter()
				.filter_map(Self::from_policy)
				.collect(),
		)
	}
}
