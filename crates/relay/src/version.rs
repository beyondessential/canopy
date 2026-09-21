//! The version floor: the lowest relay image this relay will accept being
//! told to run.
//!
//! Canopy keeps the fleet current by naming the version a relay should be on;
//! the relay patches its own Deployment's image tag and Kubernetes pulls the
//! signed image and performs the rollout. Canopy therefore supplies a version
//! string and never a binary, which bounds a compromised canopy to ordering
//! *published* images — but published includes old ones, with their known
//! bugs.
//!
//! The floor is the answer to that. It is **baked into the relay and never
//! supplied by canopy**, which is the whole of why it holds: a floor canopy
//! could set would be a floor canopy could lower.

use node_semver::Version;

/// The lowest version this build will roll itself back to.
#[derive(Debug, Clone)]
pub struct VersionFloor(Version);

#[derive(Debug, thiserror::Error)]
pub enum FloorError {
	#[error("the version floor baked into this relay is not a version: {0}")]
	Unparseable(String),

	#[error("{named} is below this relay's floor of {floor}")]
	BelowFloor { named: String, floor: Version },

	#[error("canopy named {0}, which is not a version")]
	NotAVersion(String),
}

impl VersionFloor {
	/// The floor for this build.
	///
	/// Compiled in from the crate version rather than read from the
	/// environment or from canopy: a relay refuses to go below the release it
	/// *is*, which needs no configuration to be right and cannot be
	/// reconfigured to be wrong.
	pub fn compiled() -> Result<Self, FloorError> {
		Self::at(env!("CARGO_PKG_VERSION"))
	}

	pub fn at(version: &str) -> Result<Self, FloorError> {
		version
			.parse::<Version>()
			.map(Self)
			.map_err(|e| FloorError::Unparseable(format!("{version}: {e}")))
	}

	/// Whether the relay may run the version canopy named.
	///
	/// Equal to the floor is allowed: the floor is the lowest *acceptable*
	/// version, and a relay told to stay where it is has nothing to refuse.
	pub fn admits(&self, named: &str) -> Result<Version, FloorError> {
		let named_version = named
			.parse::<Version>()
			.map_err(|_| FloorError::NotAVersion(named.to_string()))?;

		if named_version < self.0 {
			return Err(FloorError::BelowFloor {
				named: named.to_string(),
				floor: self.0.clone(),
			});
		}

		Ok(named_version)
	}
}

impl std::fmt::Display for VersionFloor {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "{}", self.0)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_compiled_floor_is_this_build() {
		let floor = VersionFloor::compiled().expect("the crate version is a version");
		assert_eq!(floor.to_string(), env!("CARGO_PKG_VERSION"));
	}

	/// The downgrade attack the floor exists for: a canopy that has been
	/// compromised orders an old image with a known bug.
	#[test]
	fn a_version_below_the_floor_is_refused() {
		let floor = VersionFloor::at("2.0.0").unwrap();
		let err = floor.admits("1.9.9").expect_err("must refuse");
		assert!(matches!(err, FloorError::BelowFloor { .. }), "got {err:?}");
	}

	#[test]
	fn the_floor_itself_and_anything_above_is_admitted() {
		let floor = VersionFloor::at("2.0.0").unwrap();
		for named in ["2.0.0", "2.0.1", "2.1.0", "3.0.0"] {
			assert!(floor.admits(named).is_ok(), "{named} must be admitted");
		}
	}

	/// Pre-release ordering is semver's, not string ordering: a relay on
	/// `2.0.0` must not be talked back onto `2.0.0-rc.1`.
	#[test]
	fn a_prerelease_of_the_floor_is_below_it() {
		let floor = VersionFloor::at("2.0.0").unwrap();
		assert!(floor.admits("2.0.0-rc.1").is_err());
	}

	#[test]
	fn a_version_that_is_not_a_version_is_refused_rather_than_guessed() {
		let floor = VersionFloor::at("2.0.0").unwrap();
		for named in ["latest", "", "main", "2", "not a version"] {
			assert!(
				matches!(floor.admits(named), Err(FloorError::NotAVersion(_))),
				"{named:?} must not be interpreted as a version",
			);
		}
	}

	/// `node-semver` is lenient: a leading `v` is accepted, and a trailing
	/// `-something` reads as a prerelease. So `v2.0.0-ish` is a version — and a
	/// prerelease of the floor, therefore below it. Recorded because the
	/// property that matters is that it is refused, not which refusal it gets.
	#[test]
	fn a_leniently_parsed_version_is_still_held_to_the_floor() {
		let floor = VersionFloor::at("2.0.0").unwrap();
		assert!(matches!(
			floor.admits("v2.0.0-ish"),
			Err(FloorError::BelowFloor { .. }),
		));
		assert!(floor.admits("v2.1.0").is_ok(), "a leading v is tolerated");
	}
}
