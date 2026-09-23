//! Which catalog entry a check resolves to.
//!
//! **This is not the same question as [`CheckSubject`](crate::subject::CheckSubject),
//! and conflating the two is the trap.** A subject says which half of a
//! unified push a reading belongs to — the box, or the workload. A namespace
//! says which catalog entry the reading's *identity* lives in, so that a
//! ceiling, a rule, a silence and a document attach to the right thing.
//!
//! Nor is it the same question as [`Scope`](../../database/src/issues.rs) — what
//! a *result* is filed against. Reachability is one check filed at every
//! target: two targets, one identity. Reading the target axis and calling it
//! identity is a mistake this model has already made twice.
//!
//! A check is identified by a namespace and a name. `version` is why: a box's
//! version, a Tamanu's version and another product's version are unrelated
//! conditions colliding on one word, and grading them against one ceiling
//! grades none of them.
//!
//! **Namespacing exists because of control over names, not because of
//! targets.** Canopy's own names are curated — we choose every one and can
//! guarantee it means a single thing — so the reserved sources need no
//! namespace and their checks are identified by name alone. Names arriving
//! over the device API are not curated, so they carry a namespace to keep
//! unrelated conditions apart.
// spec: CHK

use serde::{Deserialize, Serialize};

use crate::{server::app_type::ApplicationType, source::SUBSTRATE_SOURCE, subject::CheckSubject};

/// Source value canopy uses for conditions it determines itself:
/// reachability, backup health, key expiry, self-monitoring.
pub const CANOPY_SOURCE: &str = "canopy";

/// The source operator-raised manual conditions file under.
pub const MANUAL_SOURCE: &str = "manual";

/// Source names a device push may not claim, because canopy curates them.
///
/// Reserved and flat are the same set, and not by coincidence: a name is
/// unqualified precisely when we control what it means. The substrate source
/// is filed by a relay canopy runs, under names canopy chooses, so it is as
/// curated as canopy's own.
pub const RESERVED_SOURCES: &[&str] = &[CANOPY_SOURCE, MANUAL_SOURCE, SUBSTRATE_SOURCE];

/// Whether `source` is one of canopy's own curated reporters.
///
/// Case-insensitive, because this is the gate a device push is refused at:
/// a push claiming `CANOPY` is claiming a curated name whatever its casing.
/// Stored sources are lowercase, so callers asking about a stored value get
/// the same answer either way.
pub fn is_reserved(source: &str) -> bool {
	RESERVED_SOURCES
		.iter()
		.any(|r| source.eq_ignore_ascii_case(r))
}

/// The `subject` column value for a machine-namespaced check.
pub const SUBJECT_MACHINE: &str = "machine";

/// The `subject` column value for an application-namespaced check.
pub const SUBJECT_APPLICATION: &str = "application";

/// The namespace half of a check's identity.
///
/// Note there is no group or canopy-wide variant. A check filed at group or
/// canopy scope is always canopy's own, and canopy's own source is curated —
/// so the flat case *is* the unscoped case, and a second way to say
/// "unqualified" would only be a way to disagree with the first.
// spec: CHK
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Namespace {
	/// A curated source, whose names are unique by construction. The name
	/// alone identifies the check.
	Flat,
	/// A structured source's check about the box.
	Machine,
	/// A structured source's check about an application of this type. Two
	/// types reporting one name are two checks.
	Application(ApplicationType),
}

impl Namespace {
	/// The namespace a filing from `source` about `check_name` belongs to.
	///
	/// `application_type` is the type of the application the check was
	/// reported for, and is `None` when it was reported for a machine.
	///
	/// Returns `None` in exactly one case: a structured source's
	/// application-subject check with no application to name a type. That is
	/// the undrivable entry the migration drops; ingest cannot produce one,
	/// because a check reported for an application has an application.
	///
	/// **This function is the whole compatibility guarantee between the
	/// migration and ingest.** Both derive through it, so an entry the
	/// migration created and the next report of that check land in the same
	/// namespace. Two rules that merely agree today would drift.
	pub fn of(
		source: &str,
		check_name: &str,
		application_type: Option<&ApplicationType>,
	) -> Option<Self> {
		if is_reserved(source) {
			return Some(Self::Flat);
		}

		match CheckSubject::of(check_name) {
			CheckSubject::Machine => Some(Self::Machine),
			CheckSubject::Application => application_type.cloned().map(Self::Application),
		}
	}

	/// The namespace of a check reported for an application of this type.
	///
	/// Total, unlike [`Self::of`]: the only case that has no namespace is an
	/// application-subject check with no type to qualify by, and a check
	/// reported for an application has one. Ingest goes through here.
	pub fn for_application(
		source: &str,
		check_name: &str,
		application_type: &ApplicationType,
	) -> Self {
		Self::of(source, check_name, Some(application_type))
			.unwrap_or_else(|| Self::Application(application_type.clone()))
	}

	/// The namespace of a check on a push that names no application.
	///
	/// Total for the same reason [`Self::for_application`] is, from the other
	/// side. A box Canopy holds no application for reports about itself, so
	/// every check on such a push is the box's, including one whose name is
	/// not in the machine set: there is no application for it to belong to.
	pub fn for_machine(source: &str, check_name: &str) -> Self {
		Self::of(source, check_name, None).unwrap_or(Self::Machine)
	}

	/// The `(subject, application_type)` storage columns for this namespace.
	pub fn to_columns(&self) -> (Option<&'static str>, Option<String>) {
		match self {
			Self::Flat => (None, None),
			Self::Machine => (Some(SUBJECT_MACHINE), None),
			Self::Application(ty) => (Some(SUBJECT_APPLICATION), Some(ty.to_string())),
		}
	}

	/// The namespace encoded by a `(subject, application_type)` pair.
	///
	/// The storage CHECK admits only the three shapes below, so a pair
	/// outside them is a row written around the constraint rather than a case
	/// to interpret, and is refused instead of guessed at.
	pub fn from_columns(
		subject: Option<&str>,
		application_type: Option<&str>,
	) -> Result<Self, NamespaceFromColumnsError> {
		match (subject, application_type) {
			(None, None) => Ok(Self::Flat),
			(Some(SUBJECT_MACHINE), None) => Ok(Self::Machine),
			(Some(SUBJECT_APPLICATION), Some(ty)) => ty
				.parse()
				.map(Self::Application)
				.map_err(|_| NamespaceFromColumnsError::Type(ty.to_owned())),
			(subject, ty) => Err(NamespaceFromColumnsError::Shape {
				subject: subject.map(str::to_owned),
				application_type: ty.map(str::to_owned),
			}),
		}
	}

	/// How a check in this namespace reads to an operator.
	///
	/// The qualification is presentation, not storage: an application check
	/// shows as `<type>.<check>` so two types reporting one name are
	/// distinguishable on sight, while the name is stored on its own.
	pub fn qualified_name(&self, check_name: &str) -> String {
		match self {
			Self::Flat | Self::Machine => check_name.to_owned(),
			Self::Application(ty) => format!("{ty}.{check_name}"),
		}
	}

	/// The application type this namespace qualifies by, if any.
	pub fn application_type(&self) -> Option<&ApplicationType> {
		match self {
			Self::Application(ty) => Some(ty),
			Self::Flat | Self::Machine => None,
		}
	}
}

/// A namespace on the wire, as the pair of fields it is stored as.
///
/// The private API names a check by source, namespace and name, so this is
/// what a request carries and what a listing returns alongside the
/// presentational qualified form. It mirrors the storage columns rather than
/// inventing a third encoding, so a row read out of a listing goes straight
/// back into a mutation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct NamespaceRef {
	/// `machine`, `application`, or `null` for a curated source, whose names
	/// are unqualified.
	#[serde(default)]
	pub subject: Option<String>,
	/// The application type, set when and only when `subject` is
	/// `application`.
	#[serde(default)]
	pub application_type: Option<String>,
}

impl From<&Namespace> for NamespaceRef {
	fn from(ns: &Namespace) -> Self {
		let (subject, application_type) = ns.to_columns();
		Self {
			subject: subject.map(str::to_owned),
			application_type,
		}
	}
}

impl TryFrom<&NamespaceRef> for Namespace {
	type Error = NamespaceFromColumnsError;

	fn try_from(r: &NamespaceRef) -> Result<Self, Self::Error> {
		Self::from_columns(r.subject.as_deref(), r.application_type.as_deref())
	}
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum NamespaceFromColumnsError {
	#[error("check namespace has subject {subject:?} with application type {application_type:?}")]
	Shape {
		subject: Option<String>,
		application_type: Option<String>,
	},
	#[error("check namespace names an application type that is not a slug: {0:?}")]
	Type(String),
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_reserved_source_is_flat_whatever_the_check_is_about() {
		// `disk_free` is a machine-subject name, but canopy curates its own
		// names, so a canopy `disk_free` is identified by that name alone.
		assert_eq!(
			Namespace::of(CANOPY_SOURCE, "disk_free", None),
			Some(Namespace::Flat)
		);
		assert_eq!(
			Namespace::of(MANUAL_SOURCE, "anything_at_all", None),
			Some(Namespace::Flat)
		);
		// A substrate check is one catalog entry fleet-wide, whichever
		// application type it happens to be filed against.
		assert_eq!(
			Namespace::of(
				SUBSTRATE_SOURCE,
				"pod-unschedulable",
				Some(&ApplicationType::TamanuCentral)
			),
			Some(Namespace::Flat)
		);
	}

	#[test]
	fn the_substrate_source_is_reserved_whatever_its_casing() {
		assert!(is_reserved("kubernetes"));
		assert!(is_reserved("Kubernetes"));
	}

	#[test]
	fn a_structured_source_splits_by_subject() {
		assert_eq!(
			Namespace::of("alertd", "disk_free", None),
			Some(Namespace::Machine)
		);
		assert_eq!(
			Namespace::of("alertd", "version", Some(&ApplicationType::TamanuCentral)),
			Some(Namespace::Application(ApplicationType::TamanuCentral))
		);
	}

	#[test]
	fn two_types_reporting_one_name_are_two_namespaces() {
		let central = Namespace::of("alertd", "version", Some(&ApplicationType::TamanuCentral));
		let facility = Namespace::of("alertd", "version", Some(&ApplicationType::TamanuFacility));
		assert_ne!(central, facility);
	}

	#[test]
	fn a_machine_check_does_not_vary_by_the_type_that_reported_it() {
		// The machine namespace has nothing to vary over: one box, one entry,
		// however many workloads present the check.
		assert_eq!(
			Namespace::of("alertd", "disk_free", Some(&ApplicationType::TamanuCentral)),
			Namespace::of("alertd", "disk_free", Some(&ApplicationType::Senaite)),
		);
	}

	#[test]
	fn an_application_check_with_no_type_has_no_namespace() {
		assert_eq!(Namespace::of("alertd", "version", None), None);
	}

	#[test]
	fn the_application_form_agrees_with_the_general_one() {
		for (source, check) in [
			(CANOPY_SOURCE, "version"),
			("alertd", "disk_free"),
			("alertd", "version"),
		] {
			let ty = ApplicationType::TamanuCentral;
			assert_eq!(
				Some(Namespace::for_application(source, check, &ty)),
				Namespace::of(source, check, Some(&ty)),
				"{source}/{check} derived two ways"
			);
		}
	}

	#[test]
	fn columns_round_trip() {
		for ns in [
			Namespace::Flat,
			Namespace::Machine,
			Namespace::Application(ApplicationType::TamanuFacility),
			Namespace::Application(ApplicationType::Other("weird-thing".into())),
		] {
			let (subject, ty) = ns.to_columns();
			assert_eq!(
				Namespace::from_columns(subject, ty.as_deref()).unwrap(),
				ns,
				"{ns:?} did not survive its columns"
			);
		}
	}

	#[test]
	fn a_half_populated_namespace_is_refused_rather_than_guessed_at() {
		assert!(Namespace::from_columns(Some(SUBJECT_APPLICATION), None).is_err());
		assert!(Namespace::from_columns(None, Some("tamanu-central")).is_err());
		assert!(Namespace::from_columns(Some(SUBJECT_MACHINE), Some("tamanu-central")).is_err());
		assert!(Namespace::from_columns(Some("group"), None).is_err());
	}

	#[test]
	fn qualification_is_presentation() {
		assert_eq!(Namespace::Machine.qualified_name("disk_free"), "disk_free");
		assert_eq!(
			Namespace::Flat.qualified_name("reachability"),
			"reachability"
		);
		assert_eq!(
			Namespace::Application(ApplicationType::TamanuCentral).qualified_name("version"),
			"tamanu-central.version"
		);
	}
}
