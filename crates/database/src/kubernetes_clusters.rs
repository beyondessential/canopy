//! Kubernetes clusters canopy monitors, each through a relay running inside it.
//!
//! A registered cluster is a relay identity and a name, and nothing else:
//! canopy holds no connection credential for a cluster, so there is nothing
//! here to encrypt, rotate, or persist beyond what identifies the relay (spec
//! `K8S`, "Cluster registry").
//!
//! A registration that has not yet been confirmed is a **draft**: a row whose
//! `registered_at` is null, carrying the cluster's name and its relay's
//! identity. The draft is what accounts for the minted identity while the
//! operator installs the credential and the relay dials in, so an abandoned
//! registration leaves a record of what the identity was for rather than an
//! identity alone. Only a registered cluster hosts applications, is offered as a
//! host, or carries checks.

use commons_errors::{AppError, Result};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A Kubernetes cluster in the registry (spec `K8S`).
///
/// The row is a relay identity and a name. A draft has `registered_at` null; it
/// becomes a registered cluster the moment its relay first connects and answers.
// spec: K8S
#[derive(
	Debug, Clone, Serialize, Deserialize, Queryable, Selectable, Insertable, utoipa::ToSchema,
)]
#[diesel(table_name = crate::schema::kubernetes_clusters)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct KubernetesCluster {
	/// Unique identifier for this cluster.
	pub id: Uuid,
	/// What an operator sees in the host picker.
	pub name: String,
	/// The relay's identity. One relay per cluster, so this is unique; a
	/// cluster keeps this identity even as the relay serving it is re-issued a
	/// credential, so an application's `kubernetes_cluster_id` never moves.
	pub relay_identity_id: Uuid,
	/// Null while a draft; set the moment the relay first connects and answers,
	/// which is what turns a draft into a registered cluster.
	#[serde(skip_serializing_if = "Option::is_none")]
	#[diesel(
		deserialize_as = jiff_diesel::NullableTimestamp,
		serialize_as = jiff_diesel::NullableTimestamp,
		treat_none_as_default_value = false
	)]
	pub registered_at: Option<Timestamp>,
	/// When canopy last had a `Ping` answered by this cluster's relay, written
	/// by the relay hub. Read by registration to confirm the relay is
	/// answering, and shown to an operator. Never cleared on disconnect: it is
	/// the "when did we last hear from this" an operator wants when a cluster
	/// goes quiet.
	#[serde(skip_serializing_if = "Option::is_none")]
	#[diesel(
		deserialize_as = jiff_diesel::NullableTimestamp,
		serialize_as = jiff_diesel::NullableTimestamp,
		treat_none_as_default_value = false
	)]
	pub last_answered_at: Option<Timestamp>,
	#[serde(skip)]
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	#[serde(skip)]
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub updated_at: Timestamp,
}

impl KubernetesCluster {
	/// Whether this cluster's registration has been confirmed.
	pub fn is_registered(&self) -> bool {
		self.registered_at.is_some()
	}

	/// Create a draft: the row that accounts for a freshly minted relay
	/// identity while its registration is still in progress. `registered_at`
	/// stays null until [`Self::register`] confirms the relay is answering.
	pub async fn create_draft(
		db: &mut AsyncPgConnection,
		name: &str,
		relay_identity_id: Uuid,
	) -> Result<Self> {
		use crate::schema::kubernetes_clusters::dsl;
		diesel::insert_into(dsl::kubernetes_clusters)
			.values((
				dsl::name.eq(name),
				dsl::relay_identity_id.eq(relay_identity_id),
			))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)
	}

	pub async fn get_by_id(db: &mut AsyncPgConnection, id: Uuid) -> Result<Self> {
		use crate::schema::kubernetes_clusters::dsl;
		dsl::kubernetes_clusters
			.select(Self::as_select())
			.filter(dsl::id.eq(id))
			.first(db)
			.await
			.map_err(AppError::from)
	}

	/// The cluster a relay identity serves, if any. A relay whose identity
	/// resolves to no cluster row — the operator removed the draft, or never
	/// finished one — yields `None`, which callers treat as nothing to do
	/// rather than an error.
	pub async fn get_by_relay_identity(
		db: &mut AsyncPgConnection,
		relay_identity_id: Uuid,
	) -> Result<Option<Self>> {
		use crate::schema::kubernetes_clusters::dsl;
		dsl::kubernetes_clusters
			.select(Self::as_select())
			.filter(dsl::relay_identity_id.eq(relay_identity_id))
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// The registered cluster a relay identity serves, if any. Everything
	/// downstream — the host picker, an application's host, a filing's scope —
	/// reads through here, so a draft is invisible to all of it.
	pub async fn get_registered_by_relay_identity(
		db: &mut AsyncPgConnection,
		relay_identity_id: Uuid,
	) -> Result<Option<Self>> {
		use crate::schema::kubernetes_clusters::dsl;
		dsl::kubernetes_clusters
			.select(Self::as_select())
			.filter(dsl::relay_identity_id.eq(relay_identity_id))
			.filter(dsl::registered_at.is_not_null())
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// Every registered cluster, newest first. Drafts are excluded: they are
	/// registrations in progress, not clusters in the registry.
	pub async fn list_registered(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::kubernetes_clusters::dsl;
		dsl::kubernetes_clusters
			.select(Self::as_select())
			.filter(dsl::registered_at.is_not_null())
			.order(dsl::name.asc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Every draft, newest first: the registrations an operator has begun but
	/// not finished.
	pub async fn list_drafts(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::kubernetes_clusters::dsl;
		dsl::kubernetes_clusters
			.select(Self::as_select())
			.filter(dsl::registered_at.is_null())
			.order(dsl::created_at.desc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Confirm a registration: set `registered_at`, turning a draft into a
	/// registered cluster. Idempotent — a cluster already registered keeps its
	/// original timestamp — so a relay reconnecting never rewrites when it was
	/// first confirmed.
	pub async fn register(db: &mut AsyncPgConnection, id: Uuid) -> Result<Self> {
		use crate::schema::kubernetes_clusters::dsl;
		diesel::update(dsl::kubernetes_clusters.filter(dsl::id.eq(id)))
			.filter(dsl::registered_at.is_null())
			.set(dsl::registered_at.eq(jiff_diesel::Timestamp::from(Timestamp::now())))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Self::get_by_id(db, id).await
	}

	/// Rename a cluster.
	pub async fn rename(db: &mut AsyncPgConnection, id: Uuid, name: &str) -> Result<Self> {
		use crate::schema::kubernetes_clusters::dsl;
		diesel::update(dsl::kubernetes_clusters.filter(dsl::id.eq(id)))
			.set(dsl::name.eq(name))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)
	}

	/// Remove a cluster (draft or registered). Its relay identity is left in
	/// place; retiring the identity is a separate operator action.
	pub async fn remove(db: &mut AsyncPgConnection, id: Uuid) -> Result<()> {
		use crate::schema::kubernetes_clusters::dsl;
		diesel::delete(dsl::kubernetes_clusters.filter(dsl::id.eq(id)))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}

	/// Bulk-fetch names for a set of cluster ids, for surfaces embedding a
	/// cluster's display name beside its id.
	pub async fn names_by_ids(
		db: &mut AsyncPgConnection,
		ids: &[Uuid],
	) -> Result<std::collections::HashMap<Uuid, String>> {
		use crate::schema::kubernetes_clusters::dsl;
		if ids.is_empty() {
			return Ok(std::collections::HashMap::new());
		}
		let rows: Vec<(Uuid, String)> = dsl::kubernetes_clusters
			.select((dsl::id, dsl::name))
			.filter(dsl::id.eq_any(ids))
			.load(db)
			.await
			.map_err(AppError::from)?;
		Ok(rows.into_iter().collect())
	}

	/// Stamp that this cluster's relay answered at `at`, keyed by the relay
	/// identity the connection authenticated as. Returns whether a row was
	/// updated: a relay whose identity resolves to no cluster row updates
	/// nothing, which the caller logs rather than treating as an error.
	///
	/// Never clears the column, only advances it: staleness is computed from
	/// the timestamp, so clearing on disconnect would discard the "when did we
	/// last hear from this" an operator wants when diagnosing a quiet cluster.
	pub async fn stamp_answered(
		db: &mut AsyncPgConnection,
		relay_identity_id: Uuid,
		at: Timestamp,
	) -> Result<bool> {
		use crate::schema::kubernetes_clusters::dsl;
		let updated = diesel::update(
			dsl::kubernetes_clusters.filter(dsl::relay_identity_id.eq(relay_identity_id)),
		)
		.set(dsl::last_answered_at.eq(jiff_diesel::Timestamp::from(at)))
		.execute(db)
		.await
		.map_err(AppError::from)?;
		Ok(updated > 0)
	}
}
