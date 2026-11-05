//! What is exposed to an advisory, how it is triaged, and when a decision
//! stops standing.
//!
//! spec: VUL, VTR, PKG

use commons_types::{
	server::{RESERVED_TAG_PREFIX, TagMap, app_type::ApplicationType, rank::ServerRank},
	version::VersionStatus,
};
use database::{
	applications::Application,
	audit::Actor,
	machines::{Machine, NewMachine},
	osv::{Affected, Counted, Event, Package, Range, Record, Reopening, Severity},
	packages::{MachinePackageReport, PackageReport, Reported, ReportedPackage, VersionPackage},
	pg_duration::PgDuration,
	reported_detail::ReportedDetail,
	url_field::UrlField,
	versions::{NewVersion, Version},
	vulnerabilities::{
		Advisory, CountedFleet, Decision, ItemDetail, ItemSummary, ListFilter, NewDecision,
		Outcome, Reopened, Source, Status, Suggestion, Via, claim_for_model, prune_matches,
		refresh_counts, replace_matches,
	},
};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::{SignedDuration, civil::date};
use uuid::Uuid;

const NPM: &str = "npm";

fn today() -> jiff::civil::Date {
	date(2026, 11, 5)
}

fn range(ecosystem: &str, name: &str, kind: &str, events: &[(&str, &str)]) -> Affected {
	Affected {
		package: Some(Package {
			ecosystem: ecosystem.into(),
			name: name.into(),
		}),
		ranges: vec![Range {
			r#type: kind.into(),
			events: events
				.iter()
				.map(|(what, at)| {
					let mut event = Event::default();
					let at = Some((*at).to_owned());
					match *what {
						"introduced" => event.introduced = at,
						"fixed" => event.fixed = at,
						_ => event.last_affected = at,
					}
					event
				})
				.collect(),
		}],
		..Default::default()
	}
}

fn record(id: &str, severity: &str, affected: Vec<Affected>) -> Record {
	Record {
		id: id.into(),
		summary: format!("{id} summary"),
		modified: "2026-10-01T00:00:00Z".into(),
		database_specific: serde_json::json!({ "severity": severity }),
		affected,
		..Default::default()
	}
}

async fn version(conn: &mut AsyncPgConnection, major: i32, minor: i32, patch: i32) -> Version {
	diesel::insert_into(database::schema::versions::table)
		.values(NewVersion {
			major,
			minor,
			patch,
			status: VersionStatus::Published,
			changelog: String::new(),
			device_id: None,
		})
		.returning(<Version as diesel::SelectableHelper<_>>::as_select())
		.get_result(conn)
		.await
		.unwrap()
}

fn package(name: &str, version: &str, workspaces: &[&str], direct: bool) -> VersionPackage {
	VersionPackage {
		name: name.into(),
		version: version.into(),
		workspaces: workspaces.iter().map(|w| (*w).to_owned()).collect(),
		direct,
		paths: Default::default(),
	}
}

fn application(machine_id: Uuid, rank: Option<ServerRank>) -> Application {
	Application {
		id: Uuid::new_v4(),
		name: Some("central".into()),
		host: Some(UrlField(
			format!("https://central-{}.example/", Uuid::new_v4())
				.parse()
				.unwrap(),
		)),
		r#type: ApplicationType::TamanuCentral,
		rank,
		machine_id: Some(machine_id),
		kubernetes_cluster_id: None,
		reported_key: None,
		group_id: None,
		public_name: None,
		cloud: None,
		geolocation: None,
		is_monitored: true,
		alert_when_down_for: PgDuration(SignedDuration::from_secs(600)),
		notes: String::new(),
		tags: TagMap::default(),
		deleted_at: None,
		registered_at: None,
		may_manage_dns: false,
		may_manage_tls: false,
		certificate_profile: None,
		name_management_paused_at: None,
		name_management_paused_by: None,
		name_management_pause_reason: None,
	}
}

/// A box ranked production, with a Tamanu central on it running `version`.
async fn tamanu_on_production(
	conn: &mut AsyncPgConnection,
	version: &str,
) -> (Machine, Application) {
	let machine = Machine::create(
		conn,
		NewMachine {
			rank: Some(ServerRank::Production),
			..NewMachine::named("central-box")
		},
	)
	.await
	.unwrap();
	let app = Application::create(conn, application(machine.id, Some(ServerRank::Production)))
		.await
		.unwrap();
	ReportedDetail::record(
		conn,
		Some(app.id),
		machine.id,
		"alertd",
		&serde_json::json!({}),
		Some(&version.parse().unwrap()),
	)
	.await
	.unwrap();
	(machine, app)
}

async fn scan_npm(conn: &mut AsyncPgConnection, name: &str, version: &str, ids: &[&str]) {
	replace_matches(
		conn,
		&[(
			Counted {
				ecosystem: NPM.into(),
				name: name.into(),
				version: version.into(),
			},
			ids.iter().map(|id| (*id).to_owned()).collect(),
		)],
	)
	.await
	.unwrap();
}

fn operator() -> Actor {
	Actor::operator("admin@example.org", Some("Admin".into()))
}

fn decision(id: &str, source: Source, outcome: Outcome, via: Via) -> NewDecision {
	NewDecision {
		vulnerability_id: id.into(),
		source,
		outcome,
		reason: Some("because".into()),
		review_by: (outcome == Outcome::AcceptedRisk).then(|| date(2027, 1, 1)),
		issue_url: None,
		ground: None,
		actor: if via == Via::Model {
			Actor::canopy("ai-worker")
		} else {
			operator()
		},
		via,
	}
}

async fn list_open(conn: &mut AsyncPgConnection) -> Vec<ItemSummary> {
	ItemSummary::list(
		conn,
		&ListFilter {
			status: Some(Status::Open),
			..Default::default()
		},
		today(),
	)
	.await
	.unwrap()
	.items
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lockfile_is_stored_once() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let v = version(&mut conn, 2, 3, 4).await;
		assert!(
			!VersionPackage::lockfile_read(&mut conn, v.id)
				.await
				.unwrap()
		);
		VersionPackage::store(
			&mut conn,
			v.id,
			&[package("left-pad", "1.0.0", &["server"], true)],
		)
		.await
		.unwrap();
		assert!(
			VersionPackage::lockfile_read(&mut conn, v.id)
				.await
				.unwrap()
		);

		VersionPackage::store(&mut conn, v.id, &[package("other", "2.0.0", &[], false)])
			.await
			.unwrap();
		let held: i64 = {
			use diesel::{ExpressionMethods, QueryDsl};
			database::schema::version_packages::table
				.filter(database::schema::version_packages::version_id.eq(v.id))
				.count()
				.get_result(&mut conn)
				.await
				.unwrap()
		};
		assert_eq!(held, 1, "a second read does not replace the first");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_report_replaces_the_list_and_an_identical_one_changes_nothing() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let machine = Machine::create(&mut conn, NewMachine::named("box"))
			.await
			.unwrap();
		let report = |version: &str| PackageReport {
			distribution: "ubuntu".into(),
			release: "24.04".into(),
			packages: vec![ReportedPackage {
				name: "openssl".into(),
				version: version.into(),
			}],
		};

		let first = MachinePackageReport::store(&mut conn, machine.id, &report("3.0.13"))
			.await
			.unwrap();
		assert_eq!(
			first,
			Reported::Changed {
				package_count: 1,
				ecosystem: Some("Ubuntu:24.04:LTS".into())
			}
		);
		assert_eq!(
			MachinePackageReport::store(&mut conn, machine.id, &report("3.0.13"))
				.await
				.unwrap(),
			Reported::Unchanged
		);
		assert!(matches!(
			MachinePackageReport::store(&mut conn, machine.id, &report("3.0.14"))
				.await
				.unwrap(),
			Reported::Changed { .. }
		));
		let held = MachinePackageReport::get(&mut conn, machine.id)
			.await
			.unwrap()
			.unwrap();
		assert_eq!(held.package_count, 1);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_report_from_an_unplaced_distribution_is_kept_and_counts_for_nothing() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let machine = Machine::create(
			&mut conn,
			NewMachine {
				rank: Some(ServerRank::Production),
				..NewMachine::named("box")
			},
		)
		.await
		.unwrap();
		let stored = MachinePackageReport::store(
			&mut conn,
			machine.id,
			&PackageReport {
				distribution: "alpine".into(),
				release: "3.20".into(),
				packages: vec![ReportedPackage {
					name: "musl".into(),
					version: "1.2".into(),
				}],
			},
		)
		.await
		.unwrap();
		assert_eq!(
			stored,
			Reported::Changed {
				package_count: 1,
				ecosystem: None
			}
		);
		let fleet = CountedFleet::load(&mut conn).await.unwrap();
		assert!(fleet.packages(&mut conn).await.unwrap().is_empty());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_version_carrying_two_affected_packages_counts_its_applications_once() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let tamanu = version(&mut conn, 2, 3, 4).await;
		tamanu_on_production(&mut conn, "2.3.4").await;
		VersionPackage::store(
			&mut conn,
			tamanu.id,
			&[
				package("axios", "1.0.0", &["server"], true),
				package("follow-redirects", "1.0.0", &["server"], false),
			],
		)
		.await
		.unwrap();
		Advisory::upsert(
			&mut conn,
			&record(
				"GHSA-2",
				"HIGH",
				vec![
					range(NPM, "axios", "SEMVER", &[("introduced", "0")]),
					range(NPM, "follow-redirects", "SEMVER", &[("introduced", "0")]),
				],
			),
		)
		.await
		.unwrap();
		scan_npm(&mut conn, "axios", "1.0.0", &["GHSA-2"]).await;
		scan_npm(&mut conn, "follow-redirects", "1.0.0", &["GHSA-2"]).await;

		let listing = list_open(&mut conn).await;
		assert_eq!(listing.len(), 1);
		assert_eq!(listing[0].exposed, 1);
		assert_eq!(listing[0].packages, ["axios", "follow-redirects"]);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn only_production_exposes_and_a_rerank_changes_it_without_a_rescan() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let tamanu = version(&mut conn, 2, 3, 4).await;
		let other = version(&mut conn, 2, 3, 5).await;
		let (_, app) = tamanu_on_production(&mut conn, "2.3.4").await;
		VersionPackage::store(
			&mut conn,
			tamanu.id,
			&[package("left-pad", "1.0.0", &["server"], true)],
		)
		.await
		.unwrap();
		VersionPackage::store(
			&mut conn,
			other.id,
			&[package("left-pad", "1.0.0", &["server"], true)],
		)
		.await
		.unwrap();

		Advisory::upsert(
			&mut conn,
			&record(
				"GHSA-1",
				"HIGH",
				vec![range(NPM, "left-pad", "SEMVER", &[("introduced", "0")])],
			),
		)
		.await
		.unwrap();
		scan_npm(&mut conn, "left-pad", "1.0.0", &["GHSA-1"]).await;

		// 2.3.5 is the newest published: counted although nothing runs it.
		let listing = list_open(&mut conn).await;
		assert_eq!(listing.len(), 1);
		assert_eq!(listing[0].exposed, 1, "the one application on 2.3.4");

		let detail = ItemDetail::load(&mut conn, "GHSA-1", Source::Tamanu, today())
			.await
			.unwrap();
		let newest = detail.versions.iter().find(|v| v.newest_published).unwrap();
		assert_eq!(newest.version, "2.3.5");
		assert_eq!(
			newest.applications, 0,
			"nothing runs the newest published version"
		);
		assert_eq!(detail.versions.len(), 2);

		let machine_id = app.machine_id.unwrap();
		Machine::set_rank(&mut conn, machine_id, ServerRank::Test, None)
			.await
			.unwrap();
		let after = list_open(&mut conn).await;
		assert_eq!(after.len(), 1, "the newest published version still counts");
		assert_eq!(
			after[0].exposed, 0,
			"nothing in production runs it any more"
		);
		let _ = RESERVED_TAG_PREFIX;
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_production_machine_exposes_and_an_archived_one_does_not() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let machine = Machine::create(
			&mut conn,
			NewMachine {
				rank: Some(ServerRank::Production),
				..NewMachine::named("box")
			},
		)
		.await
		.unwrap();
		MachinePackageReport::store(
			&mut conn,
			machine.id,
			&PackageReport {
				distribution: "ubuntu".into(),
				release: "24.04".into(),
				packages: vec![ReportedPackage {
					name: "openssl".into(),
					version: "3.0.13-0ubuntu3.4".into(),
				}],
			},
		)
		.await
		.unwrap();
		Advisory::upsert(
			&mut conn,
			&record(
				"UBUNTU-1",
				"high",
				vec![range(
					"Ubuntu:24.04:LTS",
					"openssl",
					"ECOSYSTEM",
					&[("introduced", "0"), ("fixed", "3.0.13-0ubuntu3.5")],
				)],
			),
		)
		.await
		.unwrap();
		replace_matches(
			&mut conn,
			&[(
				Counted {
					ecosystem: "Ubuntu:24.04:LTS".into(),
					name: "openssl".into(),
					version: "3.0.13-0ubuntu3.4".into(),
				},
				vec!["UBUNTU-1".into()],
			)],
		)
		.await
		.unwrap();

		let listing = list_open(&mut conn).await;
		assert_eq!(listing.len(), 1);
		assert_eq!(listing[0].source, Source::Linux);
		assert_eq!(listing[0].exposed, 1);

		Machine::archive(&mut conn, machine.id).await.unwrap();
		assert!(
			list_open(&mut conn).await.is_empty(),
			"an item nothing counts for leaves the open list"
		);
		let all = ItemSummary::list(
			&mut conn,
			&ListFilter {
				include_uncounted: true,
				..Default::default()
			},
			today(),
		)
		.await
		.unwrap();
		assert_eq!(
			all.items.len(),
			1,
			"and returns when uncounted items are included"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_application_on_a_non_production_box_exposes_nothing() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let v = version(&mut conn, 3, 0, 0).await;
		// 3.0.0 is the newest published, so it counts regardless; the rank only
		// governs the applications it exposes.
		let machine = Machine::create(&mut conn, NewMachine::named("box"))
			.await
			.unwrap();
		let app = Application::create(&mut conn, application(machine.id, Some(ServerRank::Test)))
			.await
			.unwrap();
		ReportedDetail::record(
			&mut conn,
			Some(app.id),
			machine.id,
			"alertd",
			&serde_json::json!({}),
			Some(&"3.0.0".parse().unwrap()),
		)
		.await
		.unwrap();
		let fleet = CountedFleet::load(&mut conn).await.unwrap();
		assert_eq!(fleet.versions.len(), 1);
		assert_eq!(fleet.versions[0].version_id, v.id);
		assert!(fleet.versions[0].applications.is_empty());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn each_distinct_package_version_is_queried_once() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let a = version(&mut conn, 2, 0, 0).await;
		let b = version(&mut conn, 2, 1, 0).await;
		for v in [&a, &b] {
			VersionPackage::store(
				&mut conn,
				v.id,
				&[package("left-pad", "1.0.0", &["server"], true)],
			)
			.await
			.unwrap();
		}
		tamanu_on_production(&mut conn, "2.0.0").await;
		let fleet = CountedFleet::load(&mut conn).await.unwrap();
		let packages = fleet.packages(&mut conn).await.unwrap();
		assert_eq!(packages.len(), 1);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_advisory_is_refetched_only_for_a_later_revision() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		Advisory::upsert(&mut conn, &record("GHSA-1", "LOW", vec![]))
			.await
			.unwrap();
		let held = Advisory::modified_ats(&mut conn, &["GHSA-1".into(), "GHSA-2".into()])
			.await
			.unwrap();
		assert_eq!(held.len(), 1, "only what is held is reported");
		assert_eq!(
			held["GHSA-1"],
			"2026-10-01T00:00:00Z".parse::<jiff::Timestamp>().unwrap()
		);
	})
	.await;
}

async fn one_open_item(conn: &mut AsyncPgConnection) -> (Version, Application) {
	let tamanu = version(conn, 2, 3, 4).await;
	let (_, app) = tamanu_on_production(conn, "2.3.4").await;
	VersionPackage::store(
		conn,
		tamanu.id,
		&[package("left-pad", "1.5.0", &["server"], true)],
	)
	.await
	.unwrap();
	Advisory::upsert(
		conn,
		&record(
			"GHSA-1",
			"MODERATE",
			vec![range(
				NPM,
				"left-pad",
				"SEMVER",
				&[("introduced", "0"), ("fixed", "2.0.0")],
			)],
		),
	)
	.await
	.unwrap();
	scan_npm(conn, "left-pad", "1.5.0", &["GHSA-1"]).await;
	(tamanu, app)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_decision_is_current_until_replaced_and_the_history_keeps_both() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		one_open_item(&mut conn).await;
		assert_eq!(list_open(&mut conn).await.len(), 1);

		Decision::record(
			&mut conn,
			decision("GHSA-1", Source::Tamanu, Outcome::AcceptedRisk, Via::Ui),
			today(),
		)
		.await
		.unwrap();
		assert!(list_open(&mut conn).await.is_empty());

		Decision::record(
			&mut conn,
			decision("GHSA-1", Source::Tamanu, Outcome::WillFix, Via::Mcp),
			today(),
		)
		.await
		.unwrap();
		let detail = ItemDetail::load(&mut conn, "GHSA-1", Source::Tamanu, today())
			.await
			.unwrap();
		assert_eq!(detail.item.status, Status::WillFix);
		let decisions = detail
			.history
			.iter()
			.filter(|entry| {
				matches!(
					entry,
					database::vulnerabilities::HistoryEntry::Decision { .. }
				)
			})
			.count();
		assert_eq!(decisions, 2);
		assert!(detail.person_has_decided);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_accepted_risk_past_its_review_date_is_open_again_and_says_why() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		one_open_item(&mut conn).await;
		Decision::record(
			&mut conn,
			decision("GHSA-1", Source::Tamanu, Outcome::AcceptedRisk, Via::Ui),
			today(),
		)
		.await
		.unwrap();

		let later = date(2027, 1, 2);
		let open = ItemSummary::list(&mut conn, &ListFilter::default(), later)
			.await
			.unwrap()
			.items;
		assert_eq!(open[0].status, Status::Open);
		assert_eq!(
			open[0].reopened.as_ref().unwrap().reason,
			Reopening::ReviewDue
		);

		let fleet = CountedFleet::load(&mut conn).await.unwrap();
		let counted = fleet.packages(&mut conn).await.unwrap();
		let reopened = Reopened::pass(&mut conn, &counted, later).await.unwrap();
		assert_eq!(reopened.len(), 1);
		assert!(
			Reopened::pass(&mut conn, &counted, later)
				.await
				.unwrap()
				.is_empty(),
			"a decision reopens once"
		);
	})
	.await;
}

async fn revise(
	conn: &mut AsyncPgConnection,
	severity: &str,
	affected: Vec<Affected>,
) -> Vec<Reopened> {
	Advisory::upsert(conn, &record("GHSA-1", severity, affected))
		.await
		.unwrap();
	let fleet = CountedFleet::load(conn).await.unwrap();
	let counted = fleet.packages(conn).await.unwrap();
	Reopened::pass(conn, &counted, today()).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revision_raising_severity_reopens_a_decided_item() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		one_open_item(&mut conn).await;
		Decision::record(
			&mut conn,
			decision("GHSA-1", Source::Tamanu, Outcome::NotAffected, Via::Ui),
			today(),
		)
		.await
		.unwrap();
		let reopened = revise(
			&mut conn,
			"CRITICAL",
			vec![range(
				NPM,
				"left-pad",
				"SEMVER",
				&[("introduced", "0"), ("fixed", "2.0.0")],
			)],
		)
		.await;
		assert_eq!(reopened.len(), 1);
		assert!(matches!(
			reopened[0].reason,
			Reopening::SeverityRose {
				to: Severity::Critical,
				..
			}
		));
		assert_eq!(list_open(&mut conn).await.len(), 1);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revision_that_only_adds_a_fixed_version_leaves_the_decision() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		one_open_item(&mut conn).await;
		Decision::record(
			&mut conn,
			decision("GHSA-1", Source::Tamanu, Outcome::NotAffected, Via::Ui),
			today(),
		)
		.await
		.unwrap();
		let reopened = revise(
			&mut conn,
			"MODERATE",
			vec![range(
				NPM,
				"left-pad",
				"SEMVER",
				&[("introduced", "0"), ("fixed", "1.9.0")],
			)],
		)
		.await;
		assert!(reopened.is_empty());
		assert!(list_open(&mut conn).await.is_empty());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revision_widening_the_range_to_reach_a_counted_version_reopens() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let tamanu = version(&mut conn, 2, 3, 4).await;
		tamanu_on_production(&mut conn, "2.3.4").await;
		VersionPackage::store(
			&mut conn,
			tamanu.id,
			&[package("left-pad", "2.5.0", &["server"], true)],
		)
		.await
		.unwrap();
		// The advisory first matches 1.x only, and the fleet is on 2.5.0, so
		// something else in the tree carries the match.
		Advisory::upsert(
			&mut conn,
			&record(
				"GHSA-1",
				"MODERATE",
				vec![range(
					NPM,
					"left-pad",
					"SEMVER",
					&[("introduced", "0"), ("fixed", "2.0.0")],
				)],
			),
		)
		.await
		.unwrap();
		scan_npm(&mut conn, "left-pad", "2.5.0", &["GHSA-1"]).await;
		Decision::record(
			&mut conn,
			decision("GHSA-1", Source::Tamanu, Outcome::NotAffected, Via::Ui),
			today(),
		)
		.await
		.unwrap();

		let reopened = revise(
			&mut conn,
			"MODERATE",
			vec![range(
				NPM,
				"left-pad",
				"SEMVER",
				&[("introduced", "0"), ("fixed", "3.0.0")],
			)],
		)
		.await;
		assert_eq!(reopened.len(), 1);
		assert!(matches!(reopened[0].reason, Reopening::RangeWidened { .. }));
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_decision_covers_a_later_version_carrying_a_package_inside_its_ranges() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		one_open_item(&mut conn).await;
		Decision::record(
			&mut conn,
			decision("GHSA-1", Source::Tamanu, Outcome::NotAffected, Via::Ui),
			today(),
		)
		.await
		.unwrap();

		let later = version(&mut conn, 2, 4, 0).await;
		VersionPackage::store(
			&mut conn,
			later.id,
			&[package("left-pad", "1.9.9", &["server"], true)],
		)
		.await
		.unwrap();
		scan_npm(&mut conn, "left-pad", "1.9.9", &["GHSA-1"]).await;
		let fleet = CountedFleet::load(&mut conn).await.unwrap();
		let counted = fleet.packages(&mut conn).await.unwrap();
		assert!(
			Reopened::pass(&mut conn, &counted, today())
				.await
				.unwrap()
				.is_empty()
		);
		assert!(list_open(&mut conn).await.is_empty());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_decision_needs_an_item_that_exists() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let result = Decision::record(
			&mut conn,
			decision("GHSA-none", Source::Tamanu, Outcome::NotAffected, Via::Ui),
			today(),
		)
		.await;
		assert!(result.is_err());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_model_is_asked_once_for_a_new_item_and_again_when_it_reopens() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		one_open_item(&mut conn).await;
		let fleet = CountedFleet::load(&mut conn).await.unwrap();
		let exposure = database::vulnerabilities::Exposure::load(&mut conn, &fleet, None)
			.await
			.unwrap();
		refresh_counts(&mut conn, &exposure).await.unwrap();

		let retry = SignedDuration::from_hours(1);
		let first = claim_for_model(&mut conn, 10, retry).await.unwrap();
		assert_eq!(first, vec![("GHSA-1".to_owned(), Source::Tamanu)]);
		assert!(
			claim_for_model(&mut conn, 10, retry)
				.await
				.unwrap()
				.is_empty(),
			"claimed items are not handed out again at once"
		);

		Suggestion::record(
			&mut conn,
			"GHSA-1",
			Source::Tamanu,
			Outcome::WillFix,
			None,
			"patch it",
			"model",
		)
		.await
		.unwrap();
		assert!(
			claim_for_model(&mut conn, 10, SignedDuration::ZERO)
				.await
				.unwrap()
				.is_empty(),
			"an item with a suggestion is not looked at"
		);

		Decision::record(
			&mut conn,
			decision("GHSA-1", Source::Tamanu, Outcome::NotAffected, Via::Model),
			today(),
		)
		.await
		.unwrap();
		assert!(
			!Decision::person_has_decided(&mut conn, "GHSA-1", Source::Tamanu)
				.await
				.unwrap()
		);
		let reopened = revise(
			&mut conn,
			"CRITICAL",
			vec![range(
				NPM,
				"left-pad",
				"SEMVER",
				&[("introduced", "0"), ("fixed", "2.0.0")],
			)],
		)
		.await;
		assert_eq!(reopened.len(), 1);
		assert_eq!(
			claim_for_model(&mut conn, 10, SignedDuration::ZERO)
				.await
				.unwrap()
				.len(),
			1,
			"a reopened item is looked at again"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn matches_for_package_versions_no_longer_counted_are_pruned() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		one_open_item(&mut conn).await;
		let pruned = prune_matches(&mut conn, &[]).await.unwrap();
		assert_eq!(pruned, 1);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_query_that_no_longer_finds_an_advisory_drops_only_its_own_match() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		for id in ["GHSA-1", "GHSA-2"] {
			Advisory::upsert(
				&mut conn,
				&record(
					id,
					"HIGH",
					vec![range(NPM, "left-pad", "SEMVER", &[("introduced", "0")])],
				),
			)
			.await
			.unwrap();
		}
		scan_npm(&mut conn, "left-pad", "1.0.0", &["GHSA-1", "GHSA-2"]).await;
		scan_npm(&mut conn, "left-pad", "2.0.0", &["GHSA-1"]).await;

		scan_npm(&mut conn, "left-pad", "1.0.0", &["GHSA-2"]).await;

		#[derive(diesel::QueryableByName)]
		struct Row {
			#[diesel(sql_type = diesel::sql_types::Text)]
			vulnerability_id: String,
			#[diesel(sql_type = diesel::sql_types::Text)]
			package_version: String,
		}
		let rows: Vec<Row> = diesel::sql_query(
			"SELECT vulnerability_id, package_version FROM vulnerability_matches \
			 ORDER BY vulnerability_id, package_version",
		)
		.load(&mut conn)
		.await
		.unwrap();
		let held: Vec<(&str, &str)> = rows
			.iter()
			.map(|r| (r.vulnerability_id.as_str(), r.package_version.as_str()))
			.collect();
		assert_eq!(held, [("GHSA-1", "2.0.0"), ("GHSA-2", "1.0.0")]);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_advisory_with_malformed_times_is_still_stored() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let mut malformed = record("GHSA-1", "HIGH", vec![]);
		malformed.modified = "yesterday".into();
		malformed.published = Some("last week".into());
		Advisory::upsert(&mut conn, &malformed).await.unwrap();
		let held = Advisory::get(&mut conn, "GHSA-1").await.unwrap().unwrap();
		assert_eq!(held.published_at, None);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_item_nothing_counts_for_still_names_its_packages() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		Advisory::upsert(
			&mut conn,
			&record(
				"GHSA-1",
				"HIGH",
				vec![range(NPM, "left-pad", "SEMVER", &[("introduced", "0")])],
			),
		)
		.await
		.unwrap();
		scan_npm(&mut conn, "left-pad", "1.0.0", &["GHSA-1"]).await;

		let listing = ItemSummary::list(
			&mut conn,
			&ListFilter {
				include_uncounted: true,
				..Default::default()
			},
			today(),
		)
		.await
		.unwrap();
		assert_eq!(listing.items.len(), 1);
		assert!(!listing.items[0].counts);
		assert_eq!(listing.items[0].packages, ["left-pad"]);
	})
	.await;
}
