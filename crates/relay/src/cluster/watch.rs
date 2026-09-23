//! Watching the cluster's objects and determining the cluster checks from
//! them.
//!
//! Each kind a check reads is held in a reflector store kept current by a
//! watch, so the relay holds the cluster's state rather than polling for it.
//! Any change wakes the evaluation, which is also run on a short tick for the
//! holds that depend on time passing, and everything held is refiled every
//! minute.

use std::{
	fmt::Debug,
	hash::Hash,
	sync::{Arc, Mutex},
	time::{Duration, Instant},
};

use futures::StreamExt;
use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, StatefulSet};
use kube::{
	Api, Client, Resource,
	api::{ApiResource, DynamicObject, GroupVersionKind},
	runtime::{
		WatchStreamExt,
		reflector::{self, Lookup, Store, store::Writer},
		watcher,
	},
};
use serde::de::DeserializeOwned;
use serde_json::json;
use tokio::sync::Notify;
use tracing::{debug, warn};

use super::{
	Determination, Held, NODE_POOLS, Refusal, TAILSCALE_API_PROXY, WORKLOADS_RUNNING, node_pools,
	tailscale::{self, ProxyGroup},
	workloads::{self, Grader, Replicas, Share},
};
use crate::client::Filings;

/// How often everything held is refiled, whether or not it changed. The same
/// cadence alertd refiles on, so a relay sends on one clock.
const REFILE_EVERY: Duration = Duration::from_secs(60);

/// How often the checks are re-evaluated with nothing having changed, so a
/// hold that has run its length takes effect without waiting for an event.
const EVALUATE_EVERY: Duration = Duration::from_secs(10);

/// Changes arriving together are evaluated together.
const SETTLE: Duration = Duration::from_secs(1);

/// How long a watch may keep failing before a check reading it is broken. A
/// watch drops and restarts routinely, and the store stays current enough
/// across that; one that does not come back is a check that cannot run.
const FAILING_FOR: Duration = Duration::from_secs(120);

/// Run the cluster checks until the filing channel closes.
pub async fn run(client: Client, filings: Filings) {
	let changed = Arc::new(Notify::new());

	let deployments =
		Watched::<Deployment>::start(Api::all(client.clone()), "deployments.apps", (), &changed);
	let statefulsets =
		Watched::<StatefulSet>::start(Api::all(client.clone()), "statefulsets.apps", (), &changed);
	let daemonsets =
		Watched::<DaemonSet>::start(Api::all(client.clone()), "daemonsets.apps", (), &changed);
	let databases = dynamic(
		&client,
		GroupVersionKind::gvk("postgresql.cnpg.io", "v1", "Cluster"),
		"clusters",
		&changed,
	);
	let pools = dynamic(
		&client,
		GroupVersionKind::gvk("karpenter.sh", "v1", "NodePool"),
		"nodepools",
		&changed,
	);
	let proxy_groups = dynamic(
		&client,
		GroupVersionKind::gvk("tailscale.com", "v1alpha1", "ProxyGroup"),
		"proxygroups",
		&changed,
	);

	let mut held = Held::default();
	let mut grader = Grader::new(Instant::now());
	let mut evaluate = tokio::time::interval(EVALUATE_EVERY);
	let mut refile = tokio::time::interval(REFILE_EVERY);
	refile.tick().await;

	loop {
		tokio::select! {
			() = changed.notified() => tokio::time::sleep(SETTLE).await,
			_ = evaluate.tick() => {}
			_ = refile.tick() => {
				for filing in held.all() {
					if !send(&filings, filing) {
						return;
					}
				}
				continue;
			}
		}

		let now = Instant::now();
		let determined = [
			(NODE_POOLS, determine_node_pools(&pools, now)),
			(
				TAILSCALE_API_PROXY,
				determine_api_proxy(&proxy_groups, &deployments, now),
			),
			(
				WORKLOADS_RUNNING,
				determine_workloads(
					&deployments,
					&statefulsets,
					&daemonsets,
					&databases,
					&mut grader,
					now,
				),
			),
		];
		for (spec, determination) in determined {
			let Some(determination) = determination else {
				continue;
			};
			if let Some(filing) = held.set(spec, determination)
				&& !send(&filings, filing)
			{
				return;
			}
		}
	}
}

/// Queue a filing, returning whether anything is still receiving them.
///
/// A full channel drops the filing rather than waiting: the connection is down
/// or slow, and the next refile carries the current state, which is what a
/// filing sent late would have been superseded by anyway.
fn send(filings: &Filings, filing: relay_protocol::Filing) -> bool {
	match filings.try_send(filing) {
		Ok(()) => true,
		Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
			debug!("filing queue full; the next refile carries this state");
			true
		}
		Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => false,
	}
}

fn determine_node_pools(pools: &Watched<DynamicObject>, now: Instant) -> Option<Determination> {
	let pools = match pools.read(now) {
		Read::Ready(pools) => pools,
		Read::Refused(refusal) => return Some(Determination::refused(&refusal)),
		Read::Broken(message) => return Some(broken(message)),
		// No Karpenter in this cluster, so the condition does not exist here.
		Read::Absent | Read::NotYet => return None,
	};
	let mut instances: Vec<_> = pools
		.iter()
		.map(|pool| {
			node_pools::grade(
				pool.metadata.name.as_deref().unwrap_or_default(),
				&node_pools::conditions(&pool.data),
			)
		})
		.collect();
	instances.sort_by(|a, b| a.label.cmp(&b.label));
	if instances.is_empty() {
		return None;
	}
	let message = node_pools::message(&instances);
	Some(Determination { instances, message })
}

fn determine_api_proxy(
	groups: &Watched<DynamicObject>,
	deployments: &Watched<Deployment>,
	now: Instant,
) -> Option<Determination> {
	let groups: Vec<ProxyGroup> = match groups.read(now) {
		Read::Ready(all) => all
			.iter()
			.filter_map(|g| {
				ProxyGroup::read(g.metadata.name.as_deref().unwrap_or_default(), &g.data)
			})
			.collect(),
		// No ProxyGroups served here: the proxy, if any, is in-process.
		Read::Absent => Vec::new(),
		other => return other.unreadable(),
	};
	let deployments = match deployments.read(now) {
		Read::Ready(all) => all,
		other => return other.unreadable(),
	};
	let in_process = deployments.iter().find(|d| {
		d.spec
			.as_ref()
			.and_then(|s| s.template.spec.as_ref())
			.is_some_and(|pod| {
				pod.containers.iter().any(|c| {
					tailscale::runs_proxy_in_process(
						c.env
							.iter()
							.flatten()
							.find(|e| e.name == tailscale::APISERVER_PROXY_ENV)
							.and_then(|e| e.value.as_deref()),
					)
				})
			})
	});
	let name = in_process.map(|d| {
		format!(
			"{}/{}",
			d.metadata.namespace.as_deref().unwrap_or_default(),
			d.metadata.name.as_deref().unwrap_or_default()
		)
	});
	let replicas = in_process.map(|d| {
		workloads::scaled(
			d.spec.as_ref().and_then(|s| s.replicas),
			d.status.as_ref().and_then(|s| s.ready_replicas),
		)
	});
	tailscale::determine(&groups, name.as_deref().zip(replicas))
}

fn determine_workloads(
	deployments: &Watched<Deployment>,
	statefulsets: &Watched<StatefulSet>,
	daemonsets: &Watched<DaemonSet>,
	databases: &Watched<DynamicObject>,
	grader: &mut Grader,
	now: Instant,
) -> Option<Determination> {
	let mut counted: Vec<Replicas> = Vec::new();

	match deployments.read(now) {
		Read::Ready(all) => counted.extend(all.iter().map(|d| {
			workloads::scaled(
				d.spec.as_ref().and_then(|s| s.replicas),
				d.status.as_ref().and_then(|s| s.ready_replicas),
			)
		})),
		other => return other.unreadable(),
	}
	match statefulsets.read(now) {
		Read::Ready(all) => counted.extend(all.iter().map(|s| {
			workloads::scaled(
				s.spec.as_ref().and_then(|s| s.replicas),
				s.status.as_ref().and_then(|s| s.ready_replicas),
			)
		})),
		other => return other.unreadable(),
	}
	match daemonsets.read(now) {
		Read::Ready(all) => counted.extend(all.iter().map(|d| {
			let status = d.status.as_ref();
			workloads::daemon(
				status.map_or(0, |s| s.desired_number_scheduled),
				status.map_or(0, |s| s.number_ready),
			)
		})),
		other => return other.unreadable(),
	}
	match databases.read(now) {
		Read::Ready(all) => counted.extend(all.iter().filter_map(|c| {
			workloads::database(
				c.data.pointer("/spec/instances").and_then(|v| v.as_i64()),
				c.data
					.pointer("/status/readyInstances")
					.and_then(|v| v.as_i64()),
				c.metadata
					.annotations
					.as_ref()
					.and_then(|a| a.get(workloads::CNPG_HIBERNATION))
					.map(String::as_str),
			)
		})),
		// No CNPG in this cluster: no database clusters to count.
		Read::Absent => {}
		other => return other.unreadable(),
	}

	let share = Share::sum(counted);
	let percent = share.percent();
	let result = grader.observe(percent, now)?;
	Some(Determination {
		instances: vec![relay_protocol::SubstrateInstance::only(
			result,
			Some(json!({
				"healthy_share": (percent * 10.0).round() / 10.0,
				"desired": share.desired,
				"ready": share.ready,
			})),
		)],
		message: format!(
			"{:.1}% of the cluster's workload is ready ({} of {} replicas)",
			percent, share.ready, share.desired
		),
	})
}

fn broken(message: String) -> Determination {
	Determination {
		instances: vec![relay_protocol::SubstrateInstance::only(
			commons_types::status::CheckResult::Broken,
			Some(json!({ "message": message })),
		)],
		message: format!("the relay cannot read the cluster: {message}"),
	}
}

/// A watch over a CRD-defined kind, which may not be installed.
fn dynamic(
	client: &Client,
	gvk: GroupVersionKind,
	plural: &str,
	changed: &Arc<Notify>,
) -> Watched<DynamicObject> {
	let resource = ApiResource::from_gvk_with_plural(&gvk, plural);
	Watched::start(
		Api::all_with(client.clone(), &resource),
		&format!("{plural}.{}", gvk.group),
		resource,
		changed,
	)
}

/// One kind, held current by a watch.
struct Watched<K: Lookup + 'static>
where
	K::DynamicType: Eq + Hash + Clone,
{
	store: Store<K>,
	state: Arc<Mutex<State>>,
}

#[derive(Debug, Default)]
struct State {
	/// Whether the store has been filled at least once.
	synced: bool,
	problem: Option<Problem>,
}

#[derive(Debug, Clone)]
enum Problem {
	Refused(Refusal),
	/// The kind is not served by this cluster: its CRD is not installed.
	Absent,
	Failing {
		since: Instant,
		message: String,
	},
}

/// What a check can read of a kind right now.
enum Read<K> {
	Ready(Vec<Arc<K>>),
	Refused(Refusal),
	Absent,
	Broken(String),
	/// Not listed yet, so there is nothing to determine from.
	NotYet,
}

impl<K> Read<K> {
	/// What a check reading this kind files when it cannot be read.
	fn unreadable(self) -> Option<Determination> {
		match self {
			Read::Refused(refusal) => Some(Determination::refused(&refusal)),
			Read::Broken(message) => Some(broken(message)),
			Read::Absent => Some(broken("the kind is not served by this cluster".into())),
			Read::Ready(_) | Read::NotYet => None,
		}
	}
}

impl<K> Watched<K>
where
	K: Resource + Lookup + Clone + DeserializeOwned + Debug + Send + Sync + 'static,
	<K as Lookup>::DynamicType: Eq + Hash + Clone + Send + Sync,
{
	fn start(
		api: Api<K>,
		resource: &str,
		dyntype: <K as Lookup>::DynamicType,
		changed: &Arc<Notify>,
	) -> Self {
		let writer = Writer::new(dyntype);
		let store = writer.as_reader();
		let state = Arc::new(Mutex::new(State::default()));

		let stream = reflector::reflector(
			writer,
			watcher::watcher(api, watcher::Config::default()).default_backoff(),
		);
		let resource = resource.to_owned();
		let changed = changed.clone();
		let watching = state.clone();
		tokio::spawn(async move {
			let mut stream = std::pin::pin!(stream);
			while let Some(event) = stream.next().await {
				let mut state = watching.lock().unwrap_or_else(|p| p.into_inner());
				match event {
					Ok(watcher::Event::InitDone) => {
						state.synced = true;
						state.problem = None;
					}
					Ok(_) => {
						if matches!(state.problem, Some(Problem::Failing { .. })) {
							state.problem = None;
						}
					}
					Err(err) => {
						let problem = classify(&err, &resource);
						if let Problem::Refused(r) = &problem {
							warn!(resource = %r.resource, verb = r.verb, "the cluster refused the relay");
						} else {
							debug!(%resource, "watch failed: {err}");
						}
						state.problem = Some(match (state.problem.take(), problem) {
							// A watch failing for a while is timed from when it
							// started failing, not from the latest attempt.
							(
								Some(Problem::Failing { since, .. }),
								Problem::Failing { message, .. },
							) => Problem::Failing { since, message },
							(_, problem) => problem,
						});
					}
				}
				drop(state);
				changed.notify_one();
			}
		});

		Self { store, state }
	}

	fn read(&self, now: Instant) -> Read<K> {
		let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
		match &state.problem {
			Some(Problem::Refused(r)) => Read::Refused(r.clone()),
			Some(Problem::Absent) => Read::Absent,
			Some(Problem::Failing { since, message })
				if now.saturating_duration_since(*since) >= FAILING_FOR =>
			{
				Read::Broken(message.clone())
			}
			_ if !state.synced => Read::NotYet,
			_ => Read::Ready(self.store.state()),
		}
	}
}

/// What a watch error says about the kind being watched.
fn classify(err: &watcher::Error, resource: &str) -> Problem {
	let (verb, status) = match err {
		watcher::Error::InitialListFailed(kube::Error::Api(s)) => ("list", Some(s.as_ref())),
		watcher::Error::WatchStartFailed(kube::Error::Api(s))
		| watcher::Error::WatchFailed(kube::Error::Api(s)) => ("watch", Some(s.as_ref())),
		watcher::Error::WatchError(s) => ("watch", Some(s.as_ref())),
		_ => ("watch", None),
	};
	match status {
		Some(s) if s.code == 403 => Problem::Refused(Refusal {
			verb,
			resource: resource.to_owned(),
			message: s.message.clone(),
		}),
		Some(s) if s.code == 404 && verb == "list" => Problem::Absent,
		_ => Problem::Failing {
			since: Instant::now(),
			message: err.to_string(),
		},
	}
}
