//! What canopy asks of a relay, and what a relay says on connect.
//!
//! Everything here is a named question or a named action. There is no
//! general-purpose read: canopy's authority over a cluster is exactly this
//! set, which is what lets the relay hold the cluster's permissions while
//! canopy holds none (spec `K8S`, "The relay in each cluster"). Adding a
//! variant here widens that authority, so each one carries what it is for.

use serde::{Deserialize, Serialize};

use crate::filing::Instance;

/// A request canopy opens a bidirectional stream to make. Exactly one
/// [`Response`] comes back on the same stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "kebab-case")]
pub enum Request {
	/// The instances running in a namespace, for the identity picker an
	/// operator uses when marking a server as running on Kubernetes.
	NamespaceRoster { namespace: String },

	/// Whether the relay is connected and answering. Canopy confirms this
	/// before a cluster is saved, so a cluster it cannot read is caught as the
	/// operator adds it.
	Ping,

	/// What the relay is running: the version of the embedded check suite,
	/// which the skew alert grades, and the relay's own build.
	Build,

	/// Put a deployment to sleep. A deployment is a namespace, so this covers
	/// every server in that group together.
	///
	/// The relay refuses a namespace carrying no scheduled expiry
	/// ([`RefusalKind::NoScheduledExpiry`]), so the restriction holds where
	/// the expiry is known rather than resting on canopy asking correctly.
	Sleep { namespace: String },

	/// Wake a sleeping deployment.
	Wake { namespace: String },

	/// The relay image version this relay should be running.
	///
	/// Canopy supplies a version string and never a binary: the relay patches
	/// its own Deployment's image tag and Kubernetes pulls the signed image
	/// and performs the rollout. A relay refuses a version below its own
	/// floor ([`RefusalKind::BelowVersionFloor`]), which is what bounds a
	/// canopy that has been compromised into ordering a known-bad release.
	RunVersion { version: String },
}

/// The answer to a [`Request`], on the stream the request arrived on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "response", rename_all = "kebab-case")]
pub enum Response {
	/// The instances found in the namespace. An empty roster is a valid
	/// answer: the namespace exists and holds nothing yet.
	NamespaceRoster { instances: Vec<RosterEntry> },

	/// Answering.
	Pong,

	/// What the relay is running.
	Build(Hello),

	/// The deployment is asleep, or was already.
	Asleep,

	/// The deployment is awake, or was already.
	Awake,

	/// The relay accepted the version and has asked Kubernetes to roll it.
	/// Not a report that the rollout succeeded — whether the version took is
	/// what the skew alert observes.
	VersionAccepted,

	/// The relay understood the request and will not do it. Distinct from a
	/// transport failure: a refusal is the relay enforcing something it knows
	/// and canopy does not.
	Refused(Refusal),

	/// The relay tried and failed.
	Failed { message: String },
}

/// One instance in a namespace roster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RosterEntry {
	pub instance: Instance,
	/// What an operator should see in the picker, when the instance carries a
	/// name of its own in the cluster. The identity is [`Self::instance`];
	/// this is only for reading.
	pub label: Option<String>,
}

/// Why the relay will not do what was asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
	pub kind: RefusalKind,
	pub message: String,
}

/// The refusals that are the relay's own to make. Each is a rule the relay
/// can enforce because it holds the fact the rule turns on, so it is enforced
/// there rather than where the fact would have to be trusted second-hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RefusalKind {
	/// A deployment with no scheduled expiry cannot be put to sleep. The
	/// expiry is a fact of the namespace, so the relay is what knows it.
	NoScheduledExpiry,
	/// The named version is below the floor baked into this relay. The floor
	/// is the relay's own and never supplied by canopy, which is what makes it
	/// hold against a canopy ordering a downgrade.
	BelowVersionFloor,
	/// The namespace is not one this relay serves, or does not exist.
	UnknownNamespace,
}

/// What a relay is running: the answer to [`Request::Build`], which canopy
/// asks as soon as it has authenticated a connection and then holds for as
/// long as that connection lasts.
///
/// Detail that is not protocol-breaking rides here rather than in the ALPN
/// token: the check-suite version and the build are things canopy records and
/// grades, not things that decide whether the two ends can talk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
	/// The version of the embedded Tamanu check suite, which is a property of
	/// the relay and not of any server it serves. Canopy grades a relay
	/// running an out-of-step suite as a skew alert (spec `SELF`).
	pub suite_version: String,
	/// The relay's own build version — the image tag it is running, which is
	/// what canopy compares against the version it named.
	pub relay_version: String,
	/// The lowest version this relay will accept being told to run. Reported
	/// so canopy can tell a refusal it should have expected from one it should
	/// not.
	pub version_floor: String,
}

impl Response {
	/// The refusal shorthand, so a relay's own rules read as one line at the
	/// point it enforces them.
	pub fn refuse(kind: RefusalKind, message: impl Into<String>) -> Self {
		Self::Refused(Refusal {
			kind,
			message: message.into(),
		})
	}

	/// Whether this response answers the given request, so a mismatched pair
	/// is caught where it arrives rather than misread by the caller. Every
	/// request may be refused or fail.
	pub fn answers(&self, request: &Request) -> bool {
		matches!(
			(request, self),
			(_, Self::Refused(_) | Self::Failed { .. })
				| (
					Request::NamespaceRoster { .. },
					Self::NamespaceRoster { .. }
				) | (Request::Ping, Self::Pong)
				| (Request::Build, Self::Build(_))
				| (Request::Sleep { .. }, Self::Asleep)
				| (Request::Wake { .. }, Self::Awake)
				| (Request::RunVersion { .. }, Self::VersionAccepted)
		)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::frame::{read_required_frame, write_frame};

	async fn round_trip<T>(value: &T) -> T
	where
		T: serde::Serialize + serde::de::DeserializeOwned,
	{
		let mut buf = Vec::new();
		write_frame(&mut buf, value).await.unwrap();
		read_required_frame(&mut buf.as_slice()).await.unwrap()
	}

	/// Every request and every response crosses the wire as itself. Worth
	/// covering exhaustively: this set *is* canopy's authority over a cluster,
	/// so a variant that silently fails to round-trip is a method that
	/// silently does not work.
	#[tokio::test]
	async fn every_request_round_trips() {
		let requests = [
			Request::NamespaceRoster {
				namespace: "nauru-demo".into(),
			},
			Request::Ping,
			Request::Build,
			Request::Sleep {
				namespace: "nauru-demo".into(),
			},
			Request::Wake {
				namespace: "nauru-demo".into(),
			},
			Request::RunVersion {
				version: "1.2.3".into(),
			},
		];
		for request in requests {
			assert_eq!(round_trip(&request).await, request);
		}
	}

	#[tokio::test]
	async fn every_response_round_trips() {
		let hello = Hello {
			suite_version: "2.30.1".into(),
			relay_version: "1.2.3".into(),
			version_floor: "1.0.0".into(),
		};
		let responses = [
			Response::NamespaceRoster {
				instances: vec![
					RosterEntry {
						instance: Instance::Central,
						label: Some("central".into()),
					},
					RosterEntry {
						instance: Instance::Facility {
							id: "ward-a".into(),
						},
						label: None,
					},
				],
			},
			Response::Pong,
			Response::Build(hello),
			Response::Asleep,
			Response::Awake,
			Response::VersionAccepted,
			Response::refuse(RefusalKind::NoScheduledExpiry, "no expiry scheduled"),
			Response::Failed {
				message: "the API server said no".into(),
			},
		];
		for response in responses {
			assert_eq!(round_trip(&response).await, response);
		}
	}

	#[test]
	fn a_response_is_matched_to_the_request_it_answers() {
		assert!(Response::Pong.answers(&Request::Ping));
		assert!(
			!Response::Pong.answers(&Request::Sleep {
				namespace: "n".into()
			}),
			"a mismatched pair must not read as an answer",
		);
	}

	/// A refusal answers whatever was asked: the relay enforcing its own rule
	/// is a valid outcome of every request, not a shape mismatch.
	#[test]
	fn a_refusal_answers_anything() {
		let refusal = Response::refuse(RefusalKind::BelowVersionFloor, "below floor");
		assert!(refusal.answers(&Request::RunVersion {
			version: "0.1.0".into()
		}));
		assert!(refusal.answers(&Request::Ping));
	}
}
