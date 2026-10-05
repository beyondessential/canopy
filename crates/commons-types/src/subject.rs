//! What a reported fact is about: the machine, or the application.
//!
//! Both a check and a detail field answer the same question, so both are
//! decided here.
//!
//! A check's *subject* is not the same question as what a check needs in order
//! to run. bestool's registry categorises by inputs (`@tamanu`, `@db`, `host`),
//! which is why the subject has to be decided here rather than read off the
//! wire: a check that needs the host to run may still be describing the
//! application on it.
//!
//! `disk_free`, `memory`, `load`, `time_sync`, `tailscale` and their like are
//! not application checks that happen to run on a host; they assert something
//! about the box. Canopy filed them against an application only because an
//! application was the only target available.
// spec: STA

use serde::{Deserialize, Serialize};

/// The grain a check's result belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum CheckSubject {
	/// The box: its disk, memory, clock, addresses, agent.
	Machine,
	/// The workload: its version, its database, its own health.
	Application,
}

/// The checks that describe the box rather than the workload on it.
///
/// **Whole names, never prefixes.** Caddy straddles the split — `caddy_certs`
/// is an application's while the rest describe the install — and `ips` and
/// `ips_errors` share a prefix and nothing else, one being the machine's
/// addresses and the other a Tamanu error stream. A prefix rule files both
/// wrongly, and silently.
///
/// Canopy holds this list only to separate unified pushes, which carry both
/// grains' checks in one set. A push in the current format states the grain
/// itself, and a check it reports under an application is the application's
/// whatever it is called, a name on this list included.
const MACHINE_SUBJECT_CHECKS: &[&str] = &[
	"billing_tags",
	"btrfs",
	"caddy_resolvers",
	"caddy_version",
	"caddyfile_version",
	"canopy_registration",
	"disk_free",
	"external_users",
	"held_captures",
	"inodes",
	"ips",
	"load",
	"memory",
	"munin",
	"tailscale",
	"tailscale_config",
	"time_sync",
	"uptime",
];

impl CheckSubject {
	/// What this check name asserts about. Anything not named as the machine's
	/// is the application's: an unrecognised check is far more likely to be a
	/// product's own than a new fact about the box, and filing it against the
	/// application keeps it where every check has always gone.
	pub fn of(check_name: &str) -> Self {
		if MACHINE_SUBJECT_CHECKS.contains(&check_name) {
			Self::Machine
		} else {
			Self::Application
		}
	}

	pub fn is_machine(self) -> bool {
		matches!(self, Self::Machine)
	}

	/// What a reported detail field describes.
	///
	/// The machine's platform, hardware, addresses and agent; the
	/// application's version, runtime, database and configuration. Same
	/// whole-name rule as the checks: `timezone` is the application's
	/// configured zone while `osTimezone` is the box's, and they routinely
	/// differ.
	// spec: FIG
	pub fn of_detail_field(field: &str) -> Self {
		if MACHINE_SUBJECT_DETAIL.contains(&field) {
			Self::Machine
		} else {
			Self::Application
		}
	}
}

/// The reported detail fields that describe the box.
///
/// Everything else stays with the application, on the same reasoning as the
/// checks: an unrecognised field is likelier to be a product's own than a new
/// fact about the box, and leaving it where detail has always gone keeps it
/// readable rather than losing it to a grain nobody looks at.
// spec: FIG
const MACHINE_SUBJECT_DETAIL: &[&str] = &[
	"arch",
	"bestoolVersion",
	"cpuCores",
	"filesystems",
	"hostname",
	"instanceTags",
	"ipv4",
	"ipv6",
	"kernel",
	"lanIps",
	"munin",
	"nat64",
	"osKind",
	"osName",
	"osTimezone",
	"osVersion",
	// The reporting agent's own schema version, on the same footing as its
	// version: it describes the reporter on the box, not any workload, so a
	// two-workload host stamps it once.
	"reportingSchemaVersion",
	"services",
	"totalMemoryBytes",
	"uptimeSecs",
	"virtualisation",
	"virtualised",
	"wanIpv4",
	"wanIpv6",
];

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_split_matches_whole_names_not_prefixes() {
		// Caddy straddles it: the install is the box's, the certificate the
		// application's.
		assert_eq!(CheckSubject::of("caddy_version"), CheckSubject::Machine);
		assert_eq!(CheckSubject::of("caddy_resolvers"), CheckSubject::Machine);
		assert_eq!(
			CheckSubject::of("caddy_certs"),
			CheckSubject::Application,
			"a certificate is served by an application, not by the box"
		);

		// Shared prefix, unrelated subjects.
		assert_eq!(CheckSubject::of("ips"), CheckSubject::Machine);
		assert_eq!(
			CheckSubject::of("ips_errors"),
			CheckSubject::Application,
			"an error stream is the product's, not the box's addresses"
		);
	}

	#[test]
	fn anything_unrecognised_is_the_applications() {
		assert_eq!(CheckSubject::of("postgres"), CheckSubject::Application);
		assert_eq!(CheckSubject::of("tasks"), CheckSubject::Application);
		assert_eq!(
			CheckSubject::of("some_check_invented_tomorrow"),
			CheckSubject::Application
		);
	}

	#[test]
	fn a_detail_field_lands_on_the_grain_it_describes() {
		assert_eq!(
			CheckSubject::of_detail_field("osVersion"),
			CheckSubject::Machine
		);
		assert_eq!(
			CheckSubject::of_detail_field("tamanuVersion"),
			CheckSubject::Application
		);
		// The agent's schema version travels with the agent, as its own
		// version does, rather than with whatever it reports on.
		assert_eq!(
			CheckSubject::of_detail_field("reportingSchemaVersion"),
			CheckSubject::Machine
		);
		// The box's clock and the application's configured zone are different
		// facts and routinely differ.
		assert_eq!(
			CheckSubject::of_detail_field("osTimezone"),
			CheckSubject::Machine
		);
		assert_eq!(
			CheckSubject::of_detail_field("timezone"),
			CheckSubject::Application
		);
		assert_eq!(
			CheckSubject::of_detail_field("somethingNew"),
			CheckSubject::Application
		);
	}

	#[test]
	fn the_machine_lists_are_sorted_and_unique() {
		for (label, list) in [
			("checks", MACHINE_SUBJECT_CHECKS),
			("detail", MACHINE_SUBJECT_DETAIL),
		] {
			let mut sorted = list.to_vec();
			sorted.sort_unstable();
			assert_eq!(
				list,
				&sorted[..],
				"keep the {label} list in alphabetical order"
			);
			sorted.dedup();
			assert_eq!(list.len(), sorted.len(), "{label}: one entry per name");
		}
	}

	/// The check-namespace migration carries its own copy of the machine list,
	/// because a migration runs once against the fleet as it was and freezing
	/// the list is what makes it reproducible.
	///
	/// The two must agree *until it ships*, though: while it is still unapplied
	/// anywhere, a name added or moved here and not there would namespace one
	/// way at migration and the other way at the next report. Once it has run
	/// in the field the copies are free to diverge, and this test comes out
	/// with the migration's `VALUES` list frozen behind it.
	#[test]
	fn the_migration_snapshot_matches_the_subject_list() {
		let sql = include_str!("../../../migrations/2026-09-02-080628-0000_check_namespace/up.sql");
		let values = sql
			.split_once("INSERT INTO machine_subject_checks (check_name) VALUES")
			.expect("the migration no longer seeds machine_subject_checks")
			.1
			.split_once(';')
			.expect("the seed has no terminator")
			.0;

		let snapshot: Vec<&str> = values.split('\'').skip(1).step_by(2).collect();

		assert_eq!(
			snapshot, MACHINE_SUBJECT_CHECKS,
			"the migration's machine-check snapshot has drifted from the list \
			 ingest uses, so an entry it creates and the next report of that \
			 check would land in different namespaces"
		);
	}

	/// The Playwright seeds carry their own copy too, because they write the
	/// rows ingestion would have written and cannot call this list from
	/// TypeScript.
	///
	/// A name here and not there would have a spec seeding a catalog entry in
	/// one namespace while the server files the check into another, so the
	/// page under test reads a row nothing reports into and the failure looks
	/// like a UI bug.
	#[test]
	fn the_e2e_seed_snapshot_matches_the_subject_list() {
		let ts = include_str!("../../../private-web/e2e/seed.ts");
		let array = ts
			.split_once("const MACHINE_SUBJECT_CHECKS = [")
			.expect("the e2e seed no longer snapshots the machine list")
			.1
			.split_once("];")
			.expect("the snapshot has no terminator")
			.0;

		let snapshot: Vec<&str> = array.split('"').skip(1).step_by(2).collect();

		assert_eq!(
			snapshot, MACHINE_SUBJECT_CHECKS,
			"the e2e seed's machine-check snapshot has drifted from the list \
			 ingest uses, so a seeded catalog entry and the check the server \
			 files would land in different namespaces"
		);
	}

	/// The same for the detail split, which the seed mirrors so a seeded push
	/// lands its fields at the grain ingestion would. A drift here puts a
	/// box's field on the workload's report, and a fleet spread that counts
	/// machines then reads it from a population that never had it.
	#[test]
	fn the_e2e_seed_snapshot_matches_the_detail_list() {
		let ts = include_str!("../../../private-web/e2e/seed.ts");
		let array = ts
			.split_once("const MACHINE_SUBJECT_DETAIL = [")
			.expect("the e2e seed no longer snapshots the machine detail list")
			.1
			.split_once("];")
			.expect("the snapshot has no terminator")
			.0;

		let snapshot: Vec<&str> = array.split('"').skip(1).step_by(2).collect();

		assert_eq!(
			snapshot, MACHINE_SUBJECT_DETAIL,
			"the e2e seed's machine-detail snapshot has drifted from the list \
			 ingest uses, so a seeded push would split its detail differently \
			 from a real one"
		);
	}
}
