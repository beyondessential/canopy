#![allow(
	dead_code,
	reason = "this file acts as its own crate, and so can't figure out dead_code properly"
)]

use std::{collections::BTreeMap, sync::Mutex, time::Duration};

use jiff::Span;
use uuid::Uuid;

use k8s_openapi::{
	ByteString,
	api::{
		batch::v1::Job,
		core::v1::{LocalObjectReference, Secret},
	},
	apimachinery::pkg::api::resource::Quantity,
};
use kube::{
	Api, Client, ResourceExt,
	api::{ListParams, ObjectMeta, Patch, PatchParams, PostParams},
};
use pgro::{
	controllers::canopy::{intent::IntentConfig, labels},
	kopia::parse_snapshot_list_output,
	types::{
		CanopySource, ExtraUserSpec, PostgresPhysicalReplica, PostgresPhysicalReplicaSpec,
		PostgresPhysicalRestore, PostgresPhysicalRestoreSpec, RedactionSpec, ReplicaPhase,
		RestorePhase,
	},
	util::TimeSpan,
};
use tokio::time::{sleep, timeout};

use crate::stub_canopy::{REPO_PASSWORD, stub};

pub const POLL_INTERVAL: Duration = Duration::from_secs(2);
pub const PHASE_TIMEOUT: Duration = Duration::from_secs(300);
pub const LONG_PHASE_TIMEOUT: Duration = Duration::from_secs(480);

pub async fn make_client() -> Client {
	Client::try_default()
		.await
		.expect("expected a valid kubeconfig (e.g. from kind)")
}

pub async fn setup_namespace(client: &Client, ns: &str) {
	let ns_api: Api<k8s_openapi::api::core::v1::Namespace> = Api::all(client.clone());
	let ns_obj = k8s_openapi::api::core::v1::Namespace {
		metadata: ObjectMeta {
			name: Some(ns.into()),
			..Default::default()
		},
		..Default::default()
	};
	let _ = ns_api
		.patch(
			ns,
			&PatchParams::apply("integration-test"),
			&Patch::Apply(ns_obj),
		)
		.await;
}

pub async fn cleanup_namespace(client: &Client, ns: &str, replica_names: &[&str]) {
	let replicas: Api<PostgresPhysicalReplica> = Api::namespaced(client.clone(), ns);
	for name in replica_names {
		let _ = replicas.delete(name, &Default::default()).await;
	}

	let restores: Api<PostgresPhysicalRestore> = Api::namespaced(client.clone(), ns);
	if let Ok(list) = restores.list(&ListParams::default()).await {
		for restore in &list.items {
			let _ = restores
				.delete(&restore.name_any(), &Default::default())
				.await;
		}
	}

	// Wait for cascading deletes from owner references
	sleep(Duration::from_secs(5)).await;

	let secrets: Api<Secret> = Api::namespaced(client.clone(), ns);
	if let Ok(list) = secrets.list(&ListParams::default()).await {
		for secret in &list.items {
			let _ = secrets
				.delete(&secret.name_any(), &Default::default())
				.await;
		}
	}

	let jobs: Api<Job> = Api::namespaced(client.clone(), ns);
	if let Ok(list) = jobs.list(&ListParams::default()).await {
		for job in &list.items {
			let _ = jobs.delete(&job.name_any(), &Default::default()).await;
		}
	}

	sleep(Duration::from_secs(3)).await;
}

/// The Secret Canopy's worklist syncer writes beside a replica it declares:
/// where the repository is and its password, with placeholder keys, since the
/// proxy sidecar signs every request itself with what Canopy issues.
///
/// Its name is derived from the replica's, so there is no other name to pass.
pub fn build_canopy_creds_secret(ns: &str, replica_name: &str, bucket: &str) -> Secret {
	let data = [
		("bucket", bucket),
		("region", "us-east-1"),
		("prefix", ""),
		("repositoryPassword", REPO_PASSWORD),
		("accessKeyId", "PROXY_DUMMY_ACCESS_KEY"),
		("secretAccessKey", "PROXY_DUMMY_SECRET_KEY"),
	];
	Secret {
		metadata: ObjectMeta {
			name: Some(IntentConfig::canopy_creds_secret_name(replica_name)),
			namespace: Some(ns.into()),
			..Default::default()
		},
		data: Some(
			data.into_iter()
				.map(|(key, value)| (key.to_owned(), ByteString(value.as_bytes().to_vec())))
				.collect(),
		),
		..Default::default()
	}
}

/// The id of the snapshot a fixture's setup Job made in `bucket`, read from the
/// listing the Job prints once it has made it.
pub async fn snapshot_id_for(bucket: &str) -> String {
	static KNOWN: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());
	if let Some(id) = KNOWN.lock().unwrap().get(bucket) {
		return id.clone();
	}

	let job = match bucket {
		"test-bucket" => "setup-kopia-repo",
		"test-bucket-pg18" => "setup-kopia-repo-pg18",
		"non-postgres-bucket" => "setup-non-postgres-snapshot",
		"locale-bucket" => "setup-locale-snapshot",
		other => panic!("no fixture makes a snapshot in bucket {other}"),
	};
	let output = tokio::process::Command::new("kubectl")
		.args([
			"logs",
			"-n",
			"default",
			&format!("job/{job}"),
			"--all-containers",
		])
		.output()
		.await
		.expect("failed to run kubectl logs");
	assert!(
		output.status.success(),
		"cannot read the logs of {job}: {}",
		String::from_utf8_lossy(&output.stderr)
	);

	let logs = String::from_utf8_lossy(&output.stdout);
	let listing = logs
		.split_once("Verifying snapshot...")
		.unwrap_or_else(|| panic!("{job} printed no snapshot listing"))
		.1;
	let id = parse_snapshot_list_output(listing)
		.expect("snapshot listing is not JSON")
		.into_iter()
		.max_by(|a, b| a.start_time.cmp(&b.start_time))
		.unwrap_or_else(|| panic!("{job} made no snapshot"))
		.id;

	KNOWN.lock().unwrap().insert(bucket.to_owned(), id.clone());
	id
}

pub struct ReplicaOpts {
	pub schedule: String,
	pub minimum_ttl: Option<TimeSpan>,
	pub schedule_jitter: Option<TimeSpan>,
	pub read_only: bool,
	pub ephemeral: bool,
	pub redaction: Option<RedactionSpec>,
	pub extra_users: Vec<ExtraUserSpec>,
	/// How long the outgoing restore survives after a switchover. The default
	/// is short so tests that don't care aren't slowed by it; raise it in tests
	/// that need to inspect the outgoing instance before the sweep removes it.
	pub switchover_grace_period: Option<TimeSpan>,
}

impl Default for ReplicaOpts {
	fn default() -> Self {
		Self {
			schedule: "0 */6 * * *".into(),
			minimum_ttl: None,
			schedule_jitter: None,
			read_only: true,
			ephemeral: false,
			redaction: None,
			switchover_grace_period: None,
			extra_users: vec![],
		}
	}
}

/// A replica the way Canopy's syncer declares one, to be started with
/// [`start_replica`]: tests set the spec directly, as the syncer's own pass is not
/// running over it.
pub fn build_replica(name: &str, opts: ReplicaOpts) -> PostgresPhysicalReplica {
	let id = Uuid::new_v5(&Uuid::NAMESPACE_OID, name.as_bytes()).to_string();
	let intent = if opts.ephemeral {
		"verify"
	} else {
		"analytics"
	};
	let mut replica = PostgresPhysicalReplica::new(
		name,
		PostgresPhysicalReplicaSpec {
			rank: None,
			pre_migrate_drop_schemas: None,
			pre_migrate_sql: None,
			migrate_to: None,
			builder_image: None,
			canopy_source: CanopySource {
				group: id.clone(),
				r#type: "postgres".into(),
			},
			schedule: opts.schedule,
			schedule_jitter: opts.schedule_jitter.unwrap_or_default(),
			minimum_ttl: opts.minimum_ttl,
			switchover_grace_period: opts
				.switchover_grace_period
				.unwrap_or(TimeSpan(Span::new().seconds(10))),
			analytics_username: "analytics".into(),
			extra_users: opts.extra_users,
			storage_class: None,
			storage_size_override: None,
			resources: None,
			resources_floor: None,
			resources_maximum: None,
			deployment_ready_timeout: None,
			shm_size_floor: None,
			service_annotations: None,
			pod_annotations: None,
			affinity: None,
			tolerations: vec![],
			read_only: opts.read_only,
			ephemeral: opts.ephemeral,
			postgres_extra_config: None,
			persistent_schemas: None,
			redaction: opts.redaction,
			storage_size_maximum: Quantity("2Ti".to_string()),
		},
	);
	replica.metadata.labels = Some(BTreeMap::from([
		(labels::DECLARATION_ID.into(), id.clone()),
		(labels::GROUP.into(), id.clone()),
		(labels::SERVER.into(), id),
		(labels::TYPE.into(), "postgres".into()),
		(labels::INTENT.into(), intent.into()),
	]));
	replica
}

/// Create `replica` in `ns` with the Secret the syncer would write for `bucket`,
/// and point it at the snapshot the fixture made there.
pub async fn start_replica(
	client: &Client,
	ns: &str,
	replica: PostgresPhysicalReplica,
	bucket: &str,
) -> PostgresPhysicalReplica {
	let snapshot = snapshot_id_for(bucket).await;
	start_replica_at(client, ns, replica, bucket, &snapshot).await
}

/// [`start_replica`] with the snapshot named, for a replica whose bucket is not
/// the one the snapshot is in.
pub async fn start_replica_at(
	client: &Client,
	ns: &str,
	mut replica: PostgresPhysicalReplica,
	bucket: &str,
	snapshot_id: &str,
) -> PostgresPhysicalReplica {
	// Canopy has to be there before pgro's first call for credentials.
	stub();

	let name = replica.name_any();
	replica.metadata.namespace = Some(ns.into());

	let secrets: Api<Secret> = Api::namespaced(client.clone(), ns);
	secrets
		.create(
			&PostParams::default(),
			&build_canopy_creds_secret(ns, &name, bucket),
		)
		.await
		.expect("failed to create the canopy credentials secret");

	let replicas: Api<PostgresPhysicalReplica> = Api::namespaced(client.clone(), ns);
	let created = replicas
		.create(&PostParams::default(), &replica)
		.await
		.expect("failed to create replica");
	replicas
		.patch_status(
			&name,
			&PatchParams::default(),
			&Patch::Merge(serde_json::json!({
				"status": { "canopyDesiredSnapshotId": snapshot_id }
			})),
		)
		.await
		.expect("failed to point the replica at its snapshot");
	created
}

pub async fn wait_for_replica_phase(
	api: &Api<PostgresPhysicalReplica>,
	name: &str,
	target: ReplicaPhase,
	timeout_dur: Duration,
) {
	let phase_name = format!("{target:?}");
	timeout(timeout_dur, async {
		loop {
			if let Ok(replica) = api.get(name).await {
				let phase = replica.status.as_ref().and_then(|s| s.phase.as_ref());
				if phase == Some(&target) {
					println!("[{name}] reached phase {phase_name}");
					return;
				}
				println!("[{name}] phase: {phase:?}, waiting for {phase_name}");
			}
			sleep(POLL_INTERVAL).await;
		}
	})
	.await
	.unwrap_or_else(|_| panic!("timed out waiting for replica {name} to reach phase {phase_name}"));
}

pub async fn wait_for_restore_phase(
	api: &Api<PostgresPhysicalRestore>,
	replica_name: &str,
	target: RestorePhase,
	timeout_dur: Duration,
) -> String {
	let phase_name = format!("{target:?}");
	timeout(timeout_dur, async {
		loop {
			let list = api
				.list(&ListParams::default().labels(&format!("pgro.bes.au/replica={replica_name}")))
				.await
				.expect("failed to list restores");

			for restore in &list.items {
				let phase = restore.status.as_ref().and_then(|s| s.phase.as_ref());
				if phase == Some(&target) {
					let name = restore.name_any();
					println!("[{replica_name}] restore {name} reached phase {phase_name}");
					return name;
				}
				println!(
					"[{replica_name}] restore {} phase: {phase:?}, waiting for {phase_name}",
					restore.name_any(),
				);
			}

			if list.items.is_empty() {
				println!("[{replica_name}] no restores found yet, waiting for {phase_name}");
			}

			sleep(POLL_INTERVAL).await;
		}
	})
	.await
	.unwrap_or_else(|_| {
		panic!("timed out waiting for a restore of {replica_name} to reach phase {phase_name}")
	})
}

pub async fn wait_for_replica_condition(
	client: &Client,
	ns: &str,
	name: &str,
	condition_type: &str,
	expected_status: &str,
	timeout_dur: Duration,
) {
	let replicas: Api<PostgresPhysicalReplica> = Api::namespaced(client.clone(), ns);
	timeout(timeout_dur, async {
		loop {
			if let Ok(replica) = replicas.get(name).await
				&& let Some(status) = &replica.status
			{
				for cond in &status.conditions {
					if cond.type_ == condition_type && cond.status == expected_status {
						println!(
							"[{name}] condition {condition_type}={expected_status} (reason: {})",
							cond.reason
						);
						return;
					}
				}
			}
			sleep(POLL_INTERVAL).await;
		}
	})
	.await
	.unwrap_or_else(|_| {
		panic!("timed out waiting for replica {name} condition {condition_type}={expected_status}")
	});
}

pub async fn count_restores_for_replica(client: &Client, ns: &str, replica_name: &str) -> usize {
	let restores: Api<PostgresPhysicalRestore> = Api::namespaced(client.clone(), ns);
	let list = match restores
		.list(&ListParams::default().labels(&format!("pgro.bes.au/replica={replica_name}")))
		.await
	{
		Ok(l) => l,
		Err(_) => return 0,
	};
	list.items.len()
}

pub async fn try_kubectl_exec(ns: &str, target: &str, cmd: &[&str]) -> (bool, String, String) {
	let mut args = vec!["exec", "-n", ns, target, "--"];
	args.extend_from_slice(cmd);
	let output = tokio::process::Command::new("kubectl")
		.args(&args)
		.output()
		.await
		.expect("failed to run kubectl exec");
	let stdout = String::from_utf8_lossy(&output.stdout).to_string();
	let stderr = String::from_utf8_lossy(&output.stderr).to_string();
	(output.status.success(), stdout, stderr)
}

pub async fn kubectl_exec(ns: &str, target: &str, cmd: &[&str]) -> String {
	let (ok, stdout, stderr) = try_kubectl_exec(ns, target, cmd).await;
	if !ok {
		panic!("kubectl exec failed\nstdout: {stdout}\nstderr: {stderr}",);
	}
	stdout
}

/// Wait for a pod to be ready (all containers running).
pub async fn wait_for_pod_ready(ns: &str, pod: &str, timeout_dur: Duration) {
	timeout(timeout_dur, async {
		loop {
			let result = tokio::process::Command::new("kubectl")
				.args([
					"get",
					"pod",
					"-n",
					ns,
					pod,
					"-o",
					"jsonpath={.status.conditions[?(@.type=='Ready')].status}",
				])
				.output()
				.await
				.expect("failed to run kubectl");
			let stdout = String::from_utf8_lossy(&result.stdout);
			if stdout.trim() == "True" {
				println!("[{pod}] pod is ready");
				return;
			}
			println!("[{pod}] not ready yet, waiting...");
			sleep(POLL_INTERVAL).await;
		}
	})
	.await
	.unwrap_or_else(|_| panic!("timed out waiting for pod {pod} to be ready in namespace {ns}"));
}

pub fn build_second_restore(
	name: &str,
	ns: &str,
	first_restore: &PostgresPhysicalRestore,
	replica: &PostgresPhysicalReplica,
) -> PostgresPhysicalRestore {
	let mut restore = PostgresPhysicalRestore::new(
		name,
		PostgresPhysicalRestoreSpec {
			rank: None,
			migrate_to: None,
			builder_image: None,
			replica: LocalObjectReference {
				name: replica.name_any(),
			},
			snapshot: first_restore.spec.snapshot.clone(),
			snapshot_size: first_restore.spec.snapshot_size.clone(),
			snapshot_time: None,
			storage_size: first_restore.spec.storage_size.clone(),
		},
	);
	restore.metadata.namespace = Some(ns.to_string());
	restore.metadata.labels = Some(BTreeMap::from([(
		"pgro.bes.au/replica".to_string(),
		replica.name_any(),
	)]));
	restore.metadata.owner_references = Some(vec![replica.owner_reference()]);
	restore
}
