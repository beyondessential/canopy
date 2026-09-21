//! What a relay answers, and what it refuses.
//!
//! Dispatch is exercised without a connection, because what matters here is
//! the decisions: which refusals are the relay's own to make, and that they
//! hold whatever the cluster work behind them would have done.

use std::sync::Arc;

use relay::{
	VersionFloor,
	client::dispatch,
	duties::{Duties, DutyError},
};
use relay_protocol::{Hello, Instance, RefusalKind, Request, Response, RosterEntry};

/// Duties that answer, so dispatch can be tested against something other than
/// failure. `expiry` decides whether sleeping is allowed, which is the one
/// refusal that depends on cluster state.
struct Cluster {
	expiry: bool,
	rolled_to: std::sync::Mutex<Option<String>>,
}

impl Cluster {
	fn new(expiry: bool) -> Arc<Self> {
		Arc::new(Self {
			expiry,
			rolled_to: std::sync::Mutex::new(None),
		})
	}
}

impl Duties for Cluster {
	async fn roster(&self, namespace: &str) -> Result<Vec<RosterEntry>, DutyError> {
		if namespace != "nauru-demo" {
			return Err(DutyError::UnknownNamespace {
				namespace: namespace.into(),
			});
		}
		Ok(vec![RosterEntry {
			instance: Instance::Central,
			label: Some("central".into()),
		}])
	}

	fn build(&self) -> Hello {
		Hello {
			suite_version: "2.30.1".into(),
			relay_version: "2.0.0".into(),
			version_floor: "2.0.0".into(),
		}
	}

	async fn sleep(&self, namespace: &str) -> Result<(), DutyError> {
		if self.expiry {
			Ok(())
		} else {
			Err(DutyError::NoScheduledExpiry {
				namespace: namespace.into(),
			})
		}
	}

	async fn wake(&self, _namespace: &str) -> Result<(), DutyError> {
		Ok(())
	}

	async fn run_version(&self, version: &str) -> Result<(), DutyError> {
		*self.rolled_to.lock().unwrap() = Some(version.to_string());
		Ok(())
	}
}

fn floor() -> VersionFloor {
	VersionFloor::at("2.0.0").unwrap()
}

#[tokio::test]
async fn the_named_questions_are_answered() {
	let duties = Cluster::new(true);
	let floor = floor();

	assert_eq!(
		dispatch(&Request::Ping, duties.clone(), &floor).await,
		Response::Pong,
	);

	let Response::Build(build) = dispatch(&Request::Build, duties.clone(), &floor).await else {
		panic!("a build request must be answered with a build");
	};
	assert_eq!(build.suite_version, "2.30.1");

	let roster = dispatch(
		&Request::NamespaceRoster {
			namespace: "nauru-demo".into(),
		},
		duties,
		&floor,
	)
	.await;
	assert_eq!(
		roster,
		Response::NamespaceRoster {
			instances: vec![RosterEntry {
				instance: Instance::Central,
				label: Some("central".into()),
			}],
		},
	);
}

/// The restriction `K8S` puts on the relay rather than on canopy: a deployment
/// with no scheduled expiry cannot be put to sleep, and the relay is what
/// refuses, because the expiry is a fact of the namespace.
#[tokio::test]
async fn sleeping_a_deployment_with_no_expiry_is_refused_by_the_relay() {
	let response = dispatch(
		&Request::Sleep {
			namespace: "nauru-prod".into(),
		},
		Cluster::new(false),
		&floor(),
	)
	.await;

	let Response::Refused(refusal) = response else {
		panic!("a deployment with no expiry must be refused, got {response:?}");
	};
	assert_eq!(refusal.kind, RefusalKind::NoScheduledExpiry);
}

#[tokio::test]
async fn a_deployment_with_an_expiry_sleeps_and_wakes() {
	let duties = Cluster::new(true);
	assert_eq!(
		dispatch(
			&Request::Sleep {
				namespace: "nauru-demo".into()
			},
			duties.clone(),
			&floor(),
		)
		.await,
		Response::Asleep,
	);
	assert_eq!(
		dispatch(
			&Request::Wake {
				namespace: "nauru-demo".into()
			},
			duties,
			&floor(),
		)
		.await,
		Response::Awake,
	);
}

/// The downgrade attack the floor answers. A canopy that has been compromised
/// can order any *published* image, including an old one with a known bug —
/// and the relay must not take it, whatever the cluster work would have done.
#[tokio::test]
async fn a_version_below_the_floor_is_refused_before_anything_is_patched() {
	let duties = Cluster::new(true);
	let response = dispatch(
		&Request::RunVersion {
			version: "1.0.0".into(),
		},
		duties.clone(),
		&floor(),
	)
	.await;

	let Response::Refused(refusal) = response else {
		panic!("a downgrade must be refused, got {response:?}");
	};
	assert_eq!(refusal.kind, RefusalKind::BelowVersionFloor);
	assert_eq!(
		*duties.rolled_to.lock().unwrap(),
		None,
		"the floor must be checked before the Deployment is touched",
	);
}

#[tokio::test]
async fn a_version_at_or_above_the_floor_is_accepted() {
	let duties = Cluster::new(true);
	assert_eq!(
		dispatch(
			&Request::RunVersion {
				version: "2.1.0".into()
			},
			duties.clone(),
			&floor(),
		)
		.await,
		Response::VersionAccepted,
	);
	assert_eq!(
		duties.rolled_to.lock().unwrap().as_deref(),
		Some("2.1.0"),
		"an admitted version reaches the cluster work",
	);
}

/// A namespace the relay does not serve is a refusal, not a failure: the
/// distinction tells canopy whether the relay declined or tried.
#[tokio::test]
async fn a_namespace_the_relay_does_not_serve_is_a_refusal() {
	let response = dispatch(
		&Request::NamespaceRoster {
			namespace: "somewhere-else".into(),
		},
		Cluster::new(true),
		&floor(),
	)
	.await;

	let Response::Refused(refusal) = response else {
		panic!("expected a refusal, got {response:?}");
	};
	assert_eq!(refusal.kind, RefusalKind::UnknownNamespace);
}

/// A relay with no cluster access answers rather than going quiet. A silence
/// would read as a network fault; a failure reads as what it is.
#[tokio::test]
async fn an_unattached_relay_answers_with_failures_but_still_reports_its_build() {
	let build = Hello {
		suite_version: "unattached".into(),
		relay_version: "2.0.0".into(),
		version_floor: "2.0.0".into(),
	};
	let duties = Arc::new(relay::duties::Unattached::new(build));

	assert_eq!(
		dispatch(&Request::Ping, duties.clone(), &floor()).await,
		Response::Pong,
	);
	assert!(matches!(
		dispatch(&Request::Build, duties.clone(), &floor()).await,
		Response::Build(_),
	));
	assert!(matches!(
		dispatch(
			&Request::Sleep {
				namespace: "nauru-demo".into()
			},
			duties,
			&floor(),
		)
		.await,
		Response::Failed { .. },
	));
}
