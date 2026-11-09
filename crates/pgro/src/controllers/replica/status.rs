use jiff::Timestamp;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, Time};
use kube::{
	Api, Client, ResourceExt,
	api::{Patch, PatchParams},
};

use crate::{error::Result, types::*};

impl PostgresPhysicalReplica {
	/// Atomically update `nextScheduledRestore` and `scheduleInputHash` in a
	/// single status patch, preventing race conditions where a reconcile
	/// re-triggers between two separate field updates.
	pub async fn update_schedule_status(
		&self,
		client: &Client,
		next: Timestamp,
		input_hash: &str,
	) -> Result<()> {
		let replicas: Api<PostgresPhysicalReplica> = Api::namespaced(
			client.clone(),
			self.metadata
				.namespace
				.as_deref()
				.expect("PostgresPhysicalReplica is a namespaced resource"),
		);
		let patch = serde_json::json!({
			"status": {
				"nextScheduledRestore": Time(next),
				"scheduleInputHash": input_hash,
			}
		});
		replicas
			.patch_status(
				self.metadata
					.name
					.as_deref()
					.expect("cannot be called on new resource"),
				&PatchParams::apply("postgres-restore-operator"),
				&Patch::Merge(&patch),
			)
			.await?;
		Ok(())
	}

	pub async fn update_condition(
		&self,
		client: &Client,
		type_: &str,
		status: &str,
		reason: &str,
		message: &str,
	) -> Result<()> {
		let ns = self
			.metadata
			.namespace
			.as_deref()
			.expect("PostgresPhysicalReplica is a namespaced resource");
		let name = self
			.metadata
			.name
			.as_deref()
			.expect("cannot be called on new resource");
		let replicas: Api<PostgresPhysicalReplica> = Api::namespaced(client.clone(), ns);

		let current = replicas.get(name).await?;
		let mut conditions = current.status.map(|s| s.conditions).unwrap_or_default();
		let now = Time(Timestamp::now());

		if let Some(existing) = conditions.iter_mut().find(|c| c.type_ == type_) {
			if existing.status != status {
				existing.last_transition_time = now;
			}
			existing.status = status.to_string();
			existing.reason = reason.to_string();
			existing.message = message.to_string();
			existing.observed_generation = current.metadata.generation;
		} else {
			conditions.push(Condition {
				type_: type_.to_string(),
				status: status.to_string(),
				reason: reason.to_string(),
				message: message.to_string(),
				last_transition_time: now,
				observed_generation: current.metadata.generation,
			});
		}

		let patch = serde_json::json!({
			"status": {
				"conditions": conditions,
			}
		});
		replicas
			.patch_status(
				name,
				&PatchParams::apply("postgres-restore-operator"),
				&Patch::Merge(&patch),
			)
			.await?;
		Ok(())
	}

	pub async fn update_phase(&self, client: &Client, phase: ReplicaPhase) -> Result<()> {
		let replicas: Api<PostgresPhysicalReplica> = Api::namespaced(
			client.clone(),
			self.metadata
				.namespace
				.as_deref()
				.expect("PostgresPhysicalReplica is a namespaced resource"),
		);
		let patch = serde_json::json!({
			"status": {
				"phase": phase,
			}
		});
		replicas
			.patch_status(
				self.metadata
					.name
					.as_deref()
					.expect("cannot be called on new resource"),
				&PatchParams::apply("postgres-restore-operator"),
				&Patch::Merge(&patch),
			)
			.await?;
		Ok(())
	}

	pub async fn update_status_field(
		&self,
		client: &Client,
		field: &str,
		value: impl serde::Serialize,
	) -> Result<()> {
		let replicas: Api<PostgresPhysicalReplica> = Api::namespaced(
			client.clone(),
			self.metadata
				.namespace
				.as_deref()
				.expect("PostgresPhysicalReplica is a namespaced resource"),
		);
		let patch = serde_json::json!({
			"status": {
				(field): value,
			}
		});
		replicas
			.patch_status(
				self.metadata
					.name
					.as_deref()
					.expect("cannot be called on new resource"),
				&PatchParams::apply("postgres-restore-operator"),
				&Patch::Merge(&patch),
			)
			.await?;
		Ok(())
	}

	pub fn conn_info(&self) -> ConnectionInfo {
		ConnectionInfo {
			host: format!("{}.{}.svc.cluster.local", self.name_any(), self.ns()),
			port: 5432,
			database: "postgres".to_string(),
			username: self.spec.analytics_username.clone(),
			password_secret: self.creds_secret_name(),
		}
	}

	pub async fn update_connection_info(&self, client: &Client) -> Result<()> {
		let replicas: Api<PostgresPhysicalReplica> = Api::namespaced(client.clone(), &self.ns());
		let patch = serde_json::json!({
			"status": {
				"connectionInfo": self.conn_info(),
			}
		});
		replicas
			.patch_status(
				&self.name_any(),
				&PatchParams::apply("postgres-restore-operator"),
				&Patch::Merge(&patch),
			)
			.await?;
		Ok(())
	}
}

impl PostgresPhysicalRestore {
	pub async fn update_phase(&self, client: &Client, phase: RestorePhase) -> Result<()> {
		let restores: Api<Self> = Api::namespaced(
			client.clone(),
			self.metadata
				.namespace
				.as_deref()
				.expect("PostgresPhysicalRestore is a namespaced resource"),
		);
		let patch = serde_json::json!({
			"status": {
				"phase": phase,
			}
		});
		restores
			.patch_status(
				self.metadata
					.name
					.as_deref()
					.expect("cannot be called on new resource"),
				&PatchParams::apply("postgres-restore-operator"),
				&Patch::Merge(&patch),
			)
			.await?;
		Ok(())
	}

	pub async fn update_activated_at(&self, client: &Client, timestamp: Time) -> Result<()> {
		let restores: Api<Self> = Api::namespaced(
			client.clone(),
			self.metadata
				.namespace
				.as_deref()
				.expect("PostgresPhysicalRestore is a namespaced resource"),
		);
		let patch = serde_json::json!({
			"status": {
				"activatedAt": timestamp,
			}
		});
		restores
			.patch_status(
				self.metadata
					.name
					.as_deref()
					.expect("cannot be called on new resource"),
				&PatchParams::apply("postgres-restore-operator"),
				&Patch::Merge(&patch),
			)
			.await?;
		Ok(())
	}
}
