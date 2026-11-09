//! What is exposed to an advisory, how it is triaged, and when a decision
//! stops standing.
//!
//! spec: VUL, VTR, PKG

use commons_errors::AppError;
use commons_types::server::rank::ServerRank;
use database::{
	audit::Actor,
	machines::{Machine, NewMachine},
	osv::{Affected, Counted, Event, LapseReason, Package, Range, Record, Severity},
	packages::{
		MachinePackageReport, PackagePath, PackageReport, Reported, ReportedPackage,
		RepositoryPackage, RepositoryRead, Scope,
	},
	vulnerabilities::{
		Advisory, Basis, Decision, Edit, Entry, GroupBy, ItemDetail, ItemSummary, Lapse, ListQuery,
		NewDecision, Outcome, Sort, SortColumn, Source, Status, Suggestion, Via, Withdrawal,
		claim_for_model, counted_packages, prune_matches, refresh_items, replace_matches,
	},
};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::SignedDuration;

const NPM: &str = "npm";

fn tamanu() -> Source {
	Source::Repository("tamanu".into())
}

fn canopy() -> Source {
	Source::Repository("canopy".into())
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

fn fixed_before_2(name: &str) -> Vec<Affected> {
	vec![range(
		NPM,
		name,
		"SEMVER",
		&[("introduced", "0"), ("fixed", "2.0.0")],
	)]
}

/// A path of a workspace package to a package.
fn path(workspace: &str, scope: Scope, optional: bool, direct: bool) -> PackagePath {
	PackagePath {
		workspace_package: workspace.into(),
		scope,
		optional,
		direct,
		chain: vec![
			if direct { "pkg".into() } else { "via".into() },
			"pkg".into(),
		],
	}
}

fn npm_package(name: &str, version: &str, paths: Vec<PackagePath>) -> RepositoryPackage {
	RepositoryPackage {
		ecosystem: NPM.into(),
		name: name.into(),
		version: version.into(),
		paths,
	}
}

async fn read_repository(
	conn: &mut AsyncPgConnection,
	repository: &str,
	commit: &str,
	packages: &[RepositoryPackage],
) {
	RepositoryRead::store(conn, repository, commit, packages)
		.await
		.unwrap();
}

/// Store an advisory, say that it matches a package version, and make its items.
async fn scan_npm(
	conn: &mut AsyncPgConnection,
	name: &str,
	version: &str,
	ids: &[&str],
	affected: Vec<Affected>,
) {
	for id in ids {
		Advisory::upsert(conn, &record(id, "MODERATE", affected.clone()))
			.await
			.unwrap();
	}
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
	refresh_items(conn).await.unwrap();
}

fn operator() -> Actor {
	Actor::operator("admin@example.org", Some("Admin".into()))
}

fn entry(packages: Option<&[&str]>, outcome: Outcome) -> Entry {
	Entry {
		workspace_packages: packages.map(|p| p.iter().map(|s| (*s).to_owned()).collect()),
		outcome,
		reason: (outcome != Outcome::WillFix).then(|| "because".into()),
		issue_url: None,
	}
}

fn whole(id: &str, source: Source, outcome: Outcome) -> NewDecision {
	NewDecision {
		vulnerability_id: id.into(),
		source,
		entries: vec![entry(None, outcome)],
		actor: operator(),
		via: Via::Ui,
	}
}

fn split(id: &str, source: Source, entries: Vec<Entry>) -> NewDecision {
	NewDecision {
		vulnerability_id: id.into(),
		source,
		entries,
		actor: operator(),
		via: Via::Ui,
	}
}

async fn list(conn: &mut AsyncPgConnection, query: &ListQuery) -> Vec<ItemSummary> {
	ItemSummary::list(conn, query).await.unwrap().items
}

fn open() -> ListQuery {
	ListQuery {
		status: Some(Status::Open),
		..Default::default()
	}
}

fn ids(items: &[ItemSummary]) -> Vec<(&str, &str)> {
	items
		.iter()
		.map(|item| (item.id.as_str(), item.source.as_str()))
		.collect()
}

/// Tamanu reading `left-pad` 1.5.0 reached by three workspace packages, and an
/// advisory matching it.
async fn tamanu_item(conn: &mut AsyncPgConnection) {
	read_repository(
		conn,
		"tamanu",
		"abc123",
		&[npm_package(
			"left-pad",
			"1.5.0",
			vec![
				path("server", Scope::Runtime, false, true),
				path("web", Scope::Runtime, false, false),
				path("mobile", Scope::Runtime, false, false),
			],
		)],
	)
	.await;
	scan_npm(
		conn,
		"left-pad",
		"1.5.0",
		&["GHSA-1"],
		fixed_before_2("left-pad"),
	)
	.await;
}

fn linux_report(version: &str) -> PackageReport {
	PackageReport {
		distribution: "ubuntu".into(),
		release: "24.04".into(),
		packages: vec![ReportedPackage {
			name: "openssl".into(),
			version: version.into(),
		}],
	}
}

async fn production_machine(conn: &mut AsyncPgConnection, name: &str, version: &str) -> Machine {
	let machine = Machine::create(
		conn,
		NewMachine {
			rank: Some(ServerRank::Production),
			..NewMachine::named(name)
		},
	)
	.await
	.unwrap();
	MachinePackageReport::store(conn, machine.id, &linux_report(version))
		.await
		.unwrap();
	machine
}

async fn openssl_advisory(conn: &mut AsyncPgConnection, version: &str) {
	Advisory::upsert(
		conn,
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
		conn,
		&[(
			Counted {
				ecosystem: "Ubuntu:24.04:LTS".into(),
				name: "openssl".into(),
				version: version.into(),
			},
			vec!["UBUNTU-1".into()],
		)],
	)
	.await
	.unwrap();
	refresh_items(conn).await.unwrap();
}

fn refused(result: Result<impl std::fmt::Debug, AppError>) -> String {
	match result {
		Err(AppError::BadRequest(message)) => message,
		other => panic!("expected a refusal, got {other:?}"),
	}
}

// ── Reports and what counts ─────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn a_report_replaces_the_list_and_an_identical_one_changes_nothing() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let machine = Machine::create(&mut conn, NewMachine::named("box"))
			.await
			.unwrap();
		let first = MachinePackageReport::store(&mut conn, machine.id, &linux_report("3.0.13"))
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
			MachinePackageReport::store(&mut conn, machine.id, &linux_report("3.0.13"))
				.await
				.unwrap(),
			Reported::Unchanged
		);
		assert!(matches!(
			MachinePackageReport::store(&mut conn, machine.id, &linux_report("3.0.14"))
				.await
				.unwrap(),
			Reported::Changed { .. }
		));
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
		assert!(matches!(
			stored,
			Reported::Changed {
				ecosystem: None,
				..
			}
		));
		assert!(counted_packages(&mut conn).await.unwrap().is_empty());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn each_distinct_package_version_is_queried_once_however_many_sources_carry_it() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		for repository in ["tamanu", "canopy"] {
			read_repository(
				&mut conn,
				repository,
				"abc",
				&[npm_package("left-pad", "1.0.0", vec![])],
			)
			.await;
		}
		assert_eq!(counted_packages(&mut conn).await.unwrap().len(), 1);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_repository_read_replaces_what_was_held() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		read_repository(
			&mut conn,
			"tamanu",
			"one",
			&[npm_package(
				"a",
				"1.0.0",
				vec![path("w", Scope::Runtime, false, true)],
			)],
		)
		.await;
		read_repository(
			&mut conn,
			"tamanu",
			"two",
			&[npm_package(
				"b",
				"1.0.0",
				vec![path("w", Scope::Runtime, false, true)],
			)],
		)
		.await;
		let held = counted_packages(&mut conn).await.unwrap();
		assert_eq!(held.len(), 1);
		assert_eq!(held[0].name, "b");
		let read = RepositoryRead::get(&mut conn, "tamanu")
			.await
			.unwrap()
			.unwrap();
		assert_eq!(read.commit_sha.as_deref(), Some("two"));
	})
	.await;
}

// ── Items ───────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn the_same_npm_version_in_two_repositories_makes_two_items() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		for repository in ["tamanu", "canopy"] {
			read_repository(
				&mut conn,
				repository,
				"abc",
				&[npm_package(
					"left-pad",
					"1.5.0",
					vec![path("app", Scope::Runtime, false, true)],
				)],
			)
			.await;
		}
		scan_npm(
			&mut conn,
			"left-pad",
			"1.5.0",
			&["GHSA-1"],
			fixed_before_2("left-pad"),
		)
		.await;

		let items = list(&mut conn, &open()).await;
		let mut found = ids(&items);
		found.sort();
		assert_eq!(found, [("GHSA-1", "canopy"), ("GHSA-1", "tamanu")]);

		Decision::record(&mut conn, whole("GHSA-1", tamanu(), Outcome::NotAffected))
			.await
			.unwrap();
		assert_eq!(
			ids(&list(&mut conn, &open()).await),
			[("GHSA-1", "canopy")],
			"a decision for one repository leaves the other needing triage"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fix_landing_closes_an_item_and_keeps_its_status() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		tamanu_item(&mut conn).await;
		Decision::record(&mut conn, whole("GHSA-1", tamanu(), Outcome::WillFix))
			.await
			.unwrap();

		read_repository(
			&mut conn,
			"tamanu",
			"fixed",
			&[npm_package(
				"left-pad",
				"2.0.0",
				vec![path("server", Scope::Runtime, false, true)],
			)],
		)
		.await;

		assert!(list(&mut conn, &ListQuery::default()).await.is_empty());
		let closed = list(
			&mut conn,
			&ListQuery {
				include_closed: true,
				..Default::default()
			},
		)
		.await;
		assert_eq!(closed.len(), 1);
		assert!(closed[0].closed);
		assert_eq!(closed[0].status, Status::WillFix);
		assert!(closed[0].decision.is_some());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unreadable_repository_keeps_its_items() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		tamanu_item(&mut conn).await;
		RepositoryRead::note_failure(&mut conn, "tamanu", "unreachable")
			.await
			.unwrap();
		assert_eq!(list(&mut conn, &open()).await.len(), 1);
		let read = RepositoryRead::get(&mut conn, "tamanu")
			.await
			.unwrap()
			.unwrap();
		assert_eq!(read.failure.as_deref(), Some("unreachable"));
		assert_eq!(read.commit_sha.as_deref(), Some("abc123"));

		RepositoryRead::unchanged(&mut conn, "tamanu")
			.await
			.unwrap();
		let read = RepositoryRead::get(&mut conn, "tamanu")
			.await
			.unwrap()
			.unwrap();
		assert_eq!(read.failure, None);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_repository_is_stale_when_it_has_not_been_read_for_the_period() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		read_repository(&mut conn, "tamanu", "abc", &[]).await;
		RepositoryRead::note_failure(&mut conn, "canopy", "unreachable")
			.await
			.unwrap();
		diesel::sql_query(
			"UPDATE repository_reads SET read_at = now() - interval '2 days' WHERE repository = 'tamanu'",
		)
		.execute(&mut conn)
		.await
		.unwrap();
		let day = SignedDuration::from_hours(24);
		let names = |stale: Vec<RepositoryRead>| -> Vec<String> {
			stale.into_iter().map(|r| r.repository).collect()
		};
		assert_eq!(
			names(RepositoryRead::stale(&mut conn, day).await.unwrap()),
			["tamanu"],
			"a repository never read is given a day from its first attempt"
		);

		diesel::sql_query(
			"UPDATE repository_reads SET first_attempt_at = now() - interval '2 days' \
			 WHERE repository = 'canopy'",
		)
		.execute(&mut conn)
		.await
		.unwrap();
		assert_eq!(
			names(RepositoryRead::stale(&mut conn, day).await.unwrap()),
			["canopy", "tamanu"]
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn items_of_a_repository_no_longer_tracked_are_neither_counted_nor_listed() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		tamanu_item(&mut conn).await;
		diesel::sql_query(
			"INSERT INTO vulnerability_items (vulnerability_id, source) VALUES ('GHSA-1', 'dropped')",
		)
		.execute(&mut conn)
		.await
		.unwrap();

		let page = ItemSummary::list(
			&mut conn,
			&ListQuery {
				include_closed: true,
				..Default::default()
			},
		)
		.await
		.unwrap();
		assert_eq!(page.total, 1, "the dropped item is not in the total");
		assert_eq!(page.items.len(), 1);
		assert_eq!(ItemSummary::needs_triage_count(&mut conn).await.unwrap(), 1);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_item_carries_its_packages_scopes_and_minimum_fix() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		read_repository(
			&mut conn,
			"tamanu",
			"abc",
			&[npm_package(
				"left-pad",
				"1.5.0",
				vec![
					path("server", Scope::Runtime, false, true),
					path("web", Scope::Development, true, false),
				],
			)],
		)
		.await;
		scan_npm(
			&mut conn,
			"left-pad",
			"1.5.0",
			&["GHSA-1"],
			fixed_before_2("left-pad"),
		)
		.await;

		let items = list(&mut conn, &open()).await;
		let item = &items[0];
		assert_eq!(item.packages.len(), 1);
		assert_eq!(item.packages[0].version, "1.5.0");
		assert_eq!(item.packages[0].min_fix_version.as_deref(), Some("2.0.0"));
		assert_eq!(item.workspace_packages, ["server", "web"]);
		assert_eq!(item.scopes, ["development", "runtime", "optional"]);

		let detail = ItemDetail::load(&mut conn, "GHSA-1", &tamanu())
			.await
			.unwrap();
		assert_eq!(detail.commit.as_deref(), Some("abc"));
		assert_eq!(detail.packages.len(), 1);
		assert_eq!(detail.packages[0].paths.len(), 2);
		assert_eq!(detail.packages[0].fixed_in, ["2.0.0"]);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_production_machine_exposes_and_an_archived_one_closes_the_item_without_a_rescan() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let machine = production_machine(&mut conn, "box", "3.0.13-0ubuntu3.4").await;
		openssl_advisory(&mut conn, "3.0.13-0ubuntu3.4").await;

		let items = list(&mut conn, &open()).await;
		assert_eq!(items.len(), 1);
		assert_eq!(items[0].source, Source::Linux);
		assert_eq!(items[0].machines, 1);

		Machine::archive(&mut conn, machine.id).await.unwrap();
		assert!(list(&mut conn, &open()).await.is_empty());
		assert_eq!(
			list(
				&mut conn,
				&ListQuery {
					include_closed: true,
					..Default::default()
				}
			)
			.await
			.len(),
			1
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn only_production_exposes_linux_and_a_rerank_changes_it() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let machine = production_machine(&mut conn, "box", "3.0.13-0ubuntu3.4").await;
		openssl_advisory(&mut conn, "3.0.13-0ubuntu3.4").await;
		assert_eq!(list(&mut conn, &open()).await.len(), 1);

		Machine::set_rank(&mut conn, machine.id, ServerRank::Test, None)
			.await
			.unwrap();
		assert!(list(&mut conn, &open()).await.is_empty());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_linux_item_presents_machines_with_the_fix_of_the_release_they_run() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		production_machine(&mut conn, "box", "3.0.13-0ubuntu3.4").await;
		openssl_advisory(&mut conn, "3.0.13-0ubuntu3.4").await;
		let detail = ItemDetail::load(&mut conn, "UBUNTU-1", &Source::Linux)
			.await
			.unwrap();
		assert_eq!(detail.machines.len(), 1);
		assert_eq!(
			detail.machines[0].min_fix_version.as_deref(),
			Some("3.0.13-0ubuntu3.5")
		);
		assert!(detail.packages.is_empty());
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

// ── Decisions ───────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn a_decision_is_current_until_replaced_and_the_history_keeps_both() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		tamanu_item(&mut conn).await;
		assert_eq!(list(&mut conn, &open()).await.len(), 1);

		Decision::record(&mut conn, whole("GHSA-1", tamanu(), Outcome::NotAffected))
			.await
			.unwrap();
		assert!(list(&mut conn, &open()).await.is_empty());
		let items = list(&mut conn, &ListQuery::default()).await;
		assert_eq!(items[0].status, Status::NotAffected);

		Decision::record(&mut conn, whole("GHSA-1", tamanu(), Outcome::WillFix))
			.await
			.unwrap();
		let items = list(&mut conn, &ListQuery::default()).await;
		assert_eq!(items[0].status, Status::WillFix);

		let detail = ItemDetail::load(&mut conn, "GHSA-1", &tamanu())
			.await
			.unwrap();
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
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_decision_needs_an_item_that_exists() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let result =
			Decision::record(&mut conn, whole("GHSA-none", tamanu(), Outcome::WillFix)).await;
		assert!(matches!(result, Err(AppError::NotFound(_))));
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_split_decision_is_checked_against_the_workspace_packages_that_reach_the_advisory() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		tamanu_item(&mut conn).await;

		let uncovered = split(
			"GHSA-1",
			tamanu(),
			vec![entry(Some(&["server"]), Outcome::WillFix)],
		);
		assert!(refused(Decision::record(&mut conn, uncovered).await).contains("not covered"));

		let twice = split(
			"GHSA-1",
			tamanu(),
			vec![
				entry(Some(&["server", "web"]), Outcome::WillFix),
				entry(Some(&["web"]), Outcome::WillFix),
				entry(None, Outcome::WillFix),
			],
		);
		assert!(refused(Decision::record(&mut conn, twice).await).contains("more than one entry"));

		let unreached = split(
			"GHSA-1",
			tamanu(),
			vec![
				entry(Some(&["desktop"]), Outcome::WillFix),
				entry(None, Outcome::WillFix),
			],
		);
		assert!(refused(Decision::record(&mut conn, unreached).await).contains("does not reach"));

		let not_last = split(
			"GHSA-1",
			tamanu(),
			vec![
				entry(None, Outcome::WillFix),
				entry(Some(&["web"]), Outcome::WillFix),
			],
		);
		assert!(refused(Decision::record(&mut conn, not_last).await).contains("must be last"));

		let two = split(
			"GHSA-1",
			tamanu(),
			vec![entry(None, Outcome::WillFix), entry(None, Outcome::WillFix)],
		);
		assert!(refused(Decision::record(&mut conn, two).await).contains("at most one catch-all"));

		assert_eq!(list(&mut conn, &open()).await.len(), 1, "none was recorded");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_linux_item_takes_only_a_whole_decision() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		production_machine(&mut conn, "box", "3.0.13-0ubuntu3.4").await;
		openssl_advisory(&mut conn, "3.0.13-0ubuntu3.4").await;
		let result = Decision::record(
			&mut conn,
			split(
				"UBUNTU-1",
				Source::Linux,
				vec![
					entry(Some(&["a"]), Outcome::WillFix),
					entry(None, Outcome::WillFix),
				],
			),
		)
		.await;
		assert!(refused(result).contains("Linux"));
		Decision::record(
			&mut conn,
			whole("UBUNTU-1", Source::Linux, Outcome::AcceptedRisk),
		)
		.await
		.unwrap();
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_split_gives_a_shared_outcome_or_mixed() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		tamanu_item(&mut conn).await;
		let same = split(
			"GHSA-1",
			tamanu(),
			vec![
				entry(Some(&["server"]), Outcome::WillFix),
				entry(None, Outcome::WillFix),
			],
		);
		Decision::record(&mut conn, same).await.unwrap();
		assert_eq!(
			list(&mut conn, &ListQuery::default()).await[0].status,
			Status::WillFix
		);

		let different = split(
			"GHSA-1",
			tamanu(),
			vec![
				entry(Some(&["mobile"]), Outcome::WillFix),
				entry(Some(&["server"]), Outcome::AcceptedRisk),
				entry(None, Outcome::NotAffected),
			],
		);
		Decision::record(&mut conn, different).await.unwrap();
		let items = list(
			&mut conn,
			&ListQuery {
				status: Some(Status::Mixed),
				..Default::default()
			},
		)
		.await;
		assert_eq!(items.len(), 1);
		assert_eq!(items[0].decision.as_ref().unwrap().entries.len(), 3);
		assert_eq!(
			items[0].decision.as_ref().unwrap().status,
			items[0].status,
			"the view's status and the decision's own are one rule"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn withdrawing_leaves_the_item_needing_triage_with_its_suggestion() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		tamanu_item(&mut conn).await;
		Suggestion::record(
			&mut conn,
			"GHSA-1",
			&tamanu(),
			vec![entry(None, Outcome::NotAffected)],
			Basis::DependencyPathOnly,
			"model",
		)
		.await
		.unwrap();
		assert!(
			refused(
				Decision::withdraw(
					&mut conn,
					"GHSA-1",
					&tamanu(),
					"why",
					operator(),
					Via::Ui,
					None
				)
				.await
			)
			.contains("no current decision")
		);

		Decision::record(&mut conn, whole("GHSA-1", tamanu(), Outcome::AcceptedRisk))
			.await
			.unwrap();
		assert!(
			refused(
				Decision::withdraw(
					&mut conn,
					"GHSA-1",
					&tamanu(),
					"  ",
					operator(),
					Via::Ui,
					None
				)
				.await
			)
			.contains("needs a reason")
		);
		Decision::withdraw(
			&mut conn,
			"GHSA-1",
			&tamanu(),
			"wrong call",
			operator(),
			Via::Ui,
			None,
		)
		.await
		.unwrap();

		let items = list(&mut conn, &open()).await;
		assert_eq!(items.len(), 1);
		assert!(items[0].withdrawn);
		assert!(items[0].decision.is_none());
		assert!(
			items[0].suggestion.is_some(),
			"withdrawing keeps the suggestion"
		);
		assert!(
			refused(
				Decision::withdraw(
					&mut conn,
					"GHSA-1",
					&tamanu(),
					"again",
					operator(),
					Via::Ui,
					None
				)
				.await
			)
			.contains("no current decision")
		);

		let history = Withdrawal::history(&mut conn, "GHSA-1", &tamanu())
			.await
			.unwrap();
		assert_eq!(history.len(), 1);
		assert_eq!(history[0].reason, "wrong call");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_withdrawal_naming_a_decision_is_refused_when_another_stands() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		tamanu_item(&mut conn).await;
		let inspected =
			Decision::record(&mut conn, whole("GHSA-1", tamanu(), Outcome::NotAffected))
				.await
				.unwrap();
		let newer = Decision::record(&mut conn, whole("GHSA-1", tamanu(), Outcome::WillFix))
			.await
			.unwrap();

		let refusal = Decision::withdraw(
			&mut conn,
			"GHSA-1",
			&tamanu(),
			"model no longer decides",
			Actor::canopy("migration"),
			Via::Canopy,
			Some(inspected.id),
		)
		.await;
		assert!(refused(refusal).contains("no longer the one"));
		assert_eq!(
			list(&mut conn, &ListQuery::default()).await[0].status,
			Status::WillFix,
			"the newer decision still stands"
		);

		Decision::withdraw(
			&mut conn,
			"GHSA-1",
			&tamanu(),
			"model no longer decides",
			Actor::canopy("migration"),
			Via::Canopy,
			Some(newer.id),
		)
		.await
		.unwrap();
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn what_a_source_no_longer_tracked_holds_is_left_out_not_read_as_linux() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		tamanu_item(&mut conn).await;
		Decision::record(&mut conn, whole("GHSA-1", tamanu(), Outcome::WillFix))
			.await
			.unwrap();
		diesel::sql_query(
			"INSERT INTO vulnerability_items (vulnerability_id, source) VALUES ('GHSA-1', 'dropped')",
		)
		.execute(&mut conn)
		.await
		.unwrap();
		diesel::sql_query(
			"WITH d AS (INSERT INTO vulnerability_decisions \
			   (vulnerability_id, source, actor, via, severity_held, affected_held) \
			   VALUES ('GHSA-1', 'dropped', '{\"kind\":\"canopy\",\"job\":\"x\"}', 'ui', 'low', '[]') \
			   RETURNING id) \
			 INSERT INTO vulnerability_decision_withdrawals (decision_id, reason, actor, via) \
			   SELECT id, 'gone', '{\"kind\":\"canopy\",\"job\":\"x\"}', 'ui' FROM d",
		)
		.execute(&mut conn)
		.await
		.unwrap();

		assert_eq!(list(&mut conn, &ListQuery::default()).await.len(), 1);
		assert!(Lapse::pass(&mut conn).await.unwrap().is_empty());
		let detail = ItemDetail::load(&mut conn, "GHSA-1", &tamanu())
			.await
			.unwrap();
		assert!(
			detail.history.iter().all(|entry| !matches!(
				entry,
				database::vulnerabilities::HistoryEntry::Withdrawal { .. }
			)),
			"another source's withdrawal is not in this item's history"
		);
		assert!(
			Withdrawal::history(&mut conn, "GHSA-1", &Source::Linux)
				.await
				.unwrap()
				.is_empty()
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn editing_one_entry_keeps_the_earlier_text() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		tamanu_item(&mut conn).await;
		let mut will_fix = entry(Some(&["mobile"]), Outcome::WillFix);
		will_fix.issue_url = Some("https://example.org/issues/1".into());
		Decision::record(
			&mut conn,
			split(
				"GHSA-1",
				tamanu(),
				vec![
					will_fix,
					entry(Some(&["server"]), Outcome::AcceptedRisk),
					entry(None, Outcome::NotAffected),
				],
			),
		)
		.await
		.unwrap();

		let edit = Decision::edit(
			&mut conn,
			"GHSA-1",
			&tamanu(),
			Some("server"),
			Some("internal only, behind the VPN"),
			None,
			operator(),
			Via::Ui,
		)
		.await
		.unwrap();
		assert_eq!(edit.earlier_reason.as_deref(), Some("because"));
		assert_eq!(
			edit.reason.as_deref(),
			Some("internal only, behind the VPN")
		);
		assert_eq!(edit.workspace_packages, Some(vec!["server".to_owned()]));

		let items = list(&mut conn, &ListQuery::default()).await;
		let decision = items[0].decision.as_ref().unwrap();
		assert!(decision.edited);
		assert!(items[0].edited);
		assert_eq!(
			decision.entries[1].reason.as_deref(),
			Some("internal only, behind the VPN")
		);
		assert_eq!(
			decision.entries[0].reason, None,
			"the other entries are untouched"
		);

		let cleared = Decision::edit(
			&mut conn,
			"GHSA-1",
			&tamanu(),
			Some("mobile"),
			None,
			Some(""),
			operator(),
			Via::Ui,
		)
		.await
		.unwrap();
		assert_eq!(
			cleared.earlier_issue_url.as_deref(),
			Some("https://example.org/issues/1")
		);
		assert_eq!(cleared.issue_url, None);

		let catch_all = Decision::edit(
			&mut conn,
			"GHSA-1",
			&tamanu(),
			None,
			Some("never reached"),
			None,
			operator(),
			Via::Mcp,
		)
		.await
		.unwrap();
		assert_eq!(catch_all.workspace_packages, None);
		assert_eq!(
			Edit::history(&mut conn, "GHSA-1", &tamanu())
				.await
				.unwrap()
				.len(),
			3
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_edit_cannot_blank_a_required_reason_or_change_nothing() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		tamanu_item(&mut conn).await;
		Decision::record(&mut conn, whole("GHSA-1", tamanu(), Outcome::NotAffected))
			.await
			.unwrap();
		let blank = Decision::edit(
			&mut conn,
			"GHSA-1",
			&tamanu(),
			None,
			Some(" "),
			None,
			operator(),
			Via::Ui,
		)
		.await;
		assert!(refused(blank).contains("needs a reason"));
		let same = Decision::edit(
			&mut conn,
			"GHSA-1",
			&tamanu(),
			None,
			Some("because"),
			None,
			operator(),
			Via::Ui,
		)
		.await;
		assert!(refused(same).contains("changes nothing"));
		let nothing = Decision::edit(
			&mut conn,
			"GHSA-1",
			&tamanu(),
			None,
			None,
			None,
			operator(),
			Via::Ui,
		)
		.await;
		assert!(refused(nothing).contains("changes the reason"));
		let link = Decision::edit(
			&mut conn,
			"GHSA-1",
			&tamanu(),
			None,
			None,
			Some("https://example.org/1"),
			operator(),
			Via::Ui,
		)
		.await;
		assert!(refused(link).contains("only a will fix"));
		let unknown = Decision::edit(
			&mut conn,
			"GHSA-1",
			&tamanu(),
			Some("nope"),
			Some("x"),
			None,
			operator(),
			Via::Ui,
		)
		.await;
		assert!(refused(unknown).contains("no entry"));
	})
	.await;
}

// ── Lapses ──────────────────────────────────────────────────────────────────

async fn revise(
	conn: &mut AsyncPgConnection,
	severity: &str,
	affected: Vec<Affected>,
) -> Vec<Lapse> {
	Advisory::upsert(conn, &record("GHSA-1", severity, affected))
		.await
		.unwrap();
	Lapse::pass(conn).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revision_raising_severity_lapses_a_decision_and_discards_the_suggestion() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		tamanu_item(&mut conn).await;
		Suggestion::record(
			&mut conn,
			"GHSA-1",
			&tamanu(),
			vec![entry(None, Outcome::NotAffected)],
			Basis::AdvisoryTextOnly,
			"model",
		)
		.await
		.unwrap();
		Decision::record(&mut conn, whole("GHSA-1", tamanu(), Outcome::NotAffected))
			.await
			.unwrap();

		let lapsed = revise(&mut conn, "CRITICAL", fixed_before_2("left-pad")).await;
		assert_eq!(lapsed.len(), 1);
		assert!(matches!(
			lapsed[0].reason,
			LapseReason::SeverityRose {
				to: Severity::Critical,
				..
			}
		));
		let items = list(&mut conn, &open()).await;
		assert_eq!(items.len(), 1);
		assert!(items[0].lapse.is_some());
		assert!(
			items[0].suggestion.is_none(),
			"a lapse discards the suggestion"
		);
		assert!(
			revise(&mut conn, "CRITICAL", fixed_before_2("left-pad"))
				.await
				.is_empty()
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revision_that_only_adds_a_fixed_version_leaves_the_decision() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		tamanu_item(&mut conn).await;
		Decision::record(&mut conn, whole("GHSA-1", tamanu(), Outcome::NotAffected))
			.await
			.unwrap();
		let lapsed = revise(
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
		assert!(lapsed.is_empty());
		assert!(list(&mut conn, &open()).await.is_empty());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revision_widening_the_range_to_reach_what_the_source_carries_lapses_the_decision() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		read_repository(
			&mut conn,
			"tamanu",
			"abc",
			&[npm_package(
				"left-pad",
				"1.8.0",
				vec![path("server", Scope::Runtime, false, true)],
			)],
		)
		.await;
		scan_npm(
			&mut conn,
			"left-pad",
			"1.8.0",
			&["GHSA-1"],
			vec![range(
				NPM,
				"left-pad",
				"SEMVER",
				&[("introduced", "0"), ("fixed", "1.5.0")],
			)],
		)
		.await;
		Decision::record(&mut conn, whole("GHSA-1", tamanu(), Outcome::AcceptedRisk))
			.await
			.unwrap();
		let lapsed = revise(&mut conn, "MODERATE", fixed_before_2("left-pad")).await;
		assert_eq!(lapsed.len(), 1);
		assert!(matches!(lapsed[0].reason, LapseReason::RangeWidened { .. }));
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_workspace_package_lapses_a_split_without_a_catch_all_but_not_with_one() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		read_repository(
			&mut conn,
			"tamanu",
			"abc",
			&[npm_package(
				"left-pad",
				"1.5.0",
				vec![
					path("server", Scope::Runtime, false, true),
					path("web", Scope::Runtime, false, true),
				],
			)],
		)
		.await;
		scan_npm(
			&mut conn,
			"left-pad",
			"1.5.0",
			&["GHSA-1"],
			fixed_before_2("left-pad"),
		)
		.await;
		read_repository(
			&mut conn,
			"canopy",
			"abc",
			&[npm_package(
				"left-pad",
				"1.5.0",
				vec![
					path("server", Scope::Runtime, false, true),
					path("web", Scope::Runtime, false, true),
				],
			)],
		)
		.await;
		refresh_items(&mut conn).await.unwrap();

		Decision::record(
			&mut conn,
			split(
				"GHSA-1",
				tamanu(),
				vec![
					entry(Some(&["server"]), Outcome::WillFix),
					entry(Some(&["web"]), Outcome::NotAffected),
				],
			),
		)
		.await
		.unwrap();
		Decision::record(
			&mut conn,
			split(
				"GHSA-1",
				canopy(),
				vec![
					entry(Some(&["server"]), Outcome::WillFix),
					entry(None, Outcome::NotAffected),
				],
			),
		)
		.await
		.unwrap();
		assert!(Lapse::pass(&mut conn).await.unwrap().is_empty());

		for repository in ["tamanu", "canopy"] {
			read_repository(
				&mut conn,
				repository,
				"def",
				&[npm_package(
					"left-pad",
					"1.5.0",
					vec![
						path("server", Scope::Runtime, false, true),
						path("web", Scope::Runtime, false, true),
						path("mobile", Scope::Runtime, false, true),
					],
				)],
			)
			.await;
		}
		let lapsed = Lapse::pass(&mut conn).await.unwrap();
		assert_eq!(lapsed.len(), 1);
		assert_eq!(lapsed[0].source, tamanu());
		assert!(matches!(
			&lapsed[0].reason,
			LapseReason::NewWorkspacePackage { name } if name == "mobile"
		));
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_decision_covers_a_later_version_inside_its_ranges() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		tamanu_item(&mut conn).await;
		Decision::record(&mut conn, whole("GHSA-1", tamanu(), Outcome::AcceptedRisk))
			.await
			.unwrap();
		read_repository(
			&mut conn,
			"tamanu",
			"next",
			&[npm_package(
				"left-pad",
				"1.9.9",
				vec![path("server", Scope::Runtime, false, true)],
			)],
		)
		.await;
		replace_matches(
			&mut conn,
			&[(
				Counted {
					ecosystem: NPM.into(),
					name: "left-pad".into(),
					version: "1.9.9".into(),
				},
				vec!["GHSA-1".into()],
			)],
		)
		.await
		.unwrap();
		assert!(Lapse::pass(&mut conn).await.unwrap().is_empty());
		let items = list(&mut conn, &ListQuery::default()).await;
		assert_eq!(items[0].status, Status::AcceptedRisk);
	})
	.await;
}

// ── The model's queue ───────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn the_model_is_asked_once_for_a_new_item_and_again_when_it_lapses() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		tamanu_item(&mut conn).await;
		let retry = SignedDuration::from_secs(3600);

		let first = claim_for_model(&mut conn, 10, retry).await.unwrap();
		assert_eq!(first, [("GHSA-1".to_owned(), tamanu())]);
		assert!(
			claim_for_model(&mut conn, 10, retry)
				.await
				.unwrap()
				.is_empty(),
			"not asked again within the retry period"
		);

		Suggestion::record(
			&mut conn,
			"GHSA-1",
			&tamanu(),
			vec![entry(None, Outcome::NotAffected)],
			Basis::DependencyPathOnly,
			"model",
		)
		.await
		.unwrap();
		diesel::sql_query("UPDATE vulnerability_items SET model_attempted_at = NULL")
			.execute(&mut conn)
			.await
			.unwrap();
		assert!(
			claim_for_model(&mut conn, 10, retry)
				.await
				.unwrap()
				.is_empty(),
			"an item with a suggestion is not asked about"
		);

		Decision::record(&mut conn, whole("GHSA-1", tamanu(), Outcome::NotAffected))
			.await
			.unwrap();
		assert!(
			claim_for_model(&mut conn, 10, retry)
				.await
				.unwrap()
				.is_empty()
		);
		revise(&mut conn, "CRITICAL", fixed_before_2("left-pad")).await;
		assert_eq!(
			claim_for_model(&mut conn, 10, retry).await.unwrap().len(),
			1
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_model_is_not_asked_about_a_closed_item() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		tamanu_item(&mut conn).await;
		read_repository(&mut conn, "tamanu", "empty", &[]).await;
		assert!(
			claim_for_model(&mut conn, 10, SignedDuration::from_secs(60))
				.await
				.unwrap()
				.is_empty()
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_suggestion_is_held_to_the_rules_of_a_decision() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		tamanu_item(&mut conn).await;
		let result = Suggestion::record(
			&mut conn,
			"GHSA-1",
			&tamanu(),
			vec![entry(Some(&["server"]), Outcome::NotAffected)],
			Basis::DependencyPathOnly,
			"model",
		)
		.await;
		assert!(refused(result).contains("not covered"));
		Suggestion::record(
			&mut conn,
			"GHSA-1",
			&tamanu(),
			vec![
				entry(Some(&["server"]), Outcome::NotAffected),
				entry(None, Outcome::WillFix),
			],
			Basis::DependencyPathAndAdvisoryText,
			"model",
		)
		.await
		.unwrap();
		let items = list(&mut conn, &open()).await;
		let suggestion = items[0].suggestion.as_ref().unwrap();
		assert_eq!(suggestion.status, Status::Mixed);
		assert_eq!(suggestion.basis, Basis::DependencyPathAndAdvisoryText);
	})
	.await;
}

// ── Listing ─────────────────────────────────────────────────────────────────

/// Several items in two repositories, differing in severity, scope and text.
async fn assorted(conn: &mut AsyncPgConnection) {
	read_repository(
		conn,
		"tamanu",
		"abc",
		&[
			npm_package(
				"undici",
				"6.19.8",
				vec![path("server", Scope::Runtime, false, true)],
			),
			npm_package(
				"lodash",
				"4.0.0",
				vec![
					path("web", Scope::Development, false, false),
					path("server", Scope::Runtime, true, false),
				],
			),
			npm_package(
				"semver",
				"7.5.1",
				vec![path("web", Scope::Development, false, false)],
			),
		],
	)
	.await;
	read_repository(
		conn,
		"canopy",
		"abc",
		&[npm_package(
			"undici",
			"6.19.8",
			vec![path("app", Scope::Development, false, true)],
		)],
	)
	.await;

	let mut undici = record("GHSA-undici", "HIGH", fixed_before_2("undici"));
	undici.summary = "Denial of service in fetch decompression".into();
	undici.details = "A crafted response exhausts memory while decompressing.".into();
	let mut lodash = record("GHSA-lodash", "CRITICAL", fixed_before_2("lodash"));
	lodash.summary = "Prototype pollution in merge helper".into();
	lodash.details = "The undici client is not involved, but this text mentions it.".into();
	let mut semver = record("GHSA-semver", "LOW", fixed_before_2("semver"));
	semver.summary = "Regular expression denial of service".into();
	for (record, name, version) in [
		(undici, "undici", "6.19.8"),
		(lodash, "lodash", "4.0.0"),
		(semver, "semver", "7.5.1"),
	] {
		let id = record.id.clone();
		Advisory::upsert(conn, &record).await.unwrap();
		replace_matches(
			conn,
			&[(
				Counted {
					ecosystem: NPM.into(),
					name: name.into(),
					version: version.into(),
				},
				vec![id],
			)],
		)
		.await
		.unwrap();
	}
	refresh_items(conn).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn items_are_ordered_by_severity_by_default_and_by_any_column_when_chosen() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		assorted(&mut conn).await;
		let default = list(&mut conn, &ListQuery::default()).await;
		assert_eq!(
			ids(&default),
			[
				("GHSA-lodash", "tamanu"),
				("GHSA-undici", "canopy"),
				("GHSA-undici", "tamanu"),
				("GHSA-semver", "tamanu"),
			]
		);

		let by_package = list(
			&mut conn,
			&ListQuery {
				sort: Some(Sort {
					column: SortColumn::Package,
					descending: true,
				}),
				..Default::default()
			},
		)
		.await;
		assert_eq!(by_package[0].packages[0].name, "undici");
		assert_eq!(by_package[3].packages[0].name, "lodash");

		let by_source = list(
			&mut conn,
			&ListQuery {
				sort: Some(Sort {
					column: SortColumn::Source,
					descending: false,
				}),
				..Default::default()
			},
		)
		.await;
		assert_eq!(by_source[0].source, canopy());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_search_ranks_a_summary_hit_above_a_details_only_hit_and_tolerates_a_typo() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		assorted(&mut conn).await;
		let search = |term: &str| ListQuery {
			search: Some(term.into()),
			..Default::default()
		};

		let found = list(&mut conn, &search("undici")).await;
		let order = ids(&found);
		assert_eq!(order[0].0, "GHSA-undici");
		assert!(
			order.contains(&("GHSA-lodash", "tamanu")),
			"a details-only hit still matches"
		);
		assert_eq!(
			order.last().unwrap(),
			&("GHSA-lodash", "tamanu"),
			"and ranks below the summary and package hits"
		);
		assert!(!order.iter().any(|(id, _)| *id == "GHSA-semver"));

		let typo = list(&mut conn, &search("undcii")).await;
		assert!(typo.iter().any(|item| item.id == "GHSA-undici"));

		let partial = list(&mut conn, &search("prototy")).await;
		assert_eq!(ids(&partial), [("GHSA-lodash", "tamanu")]);

		let details_words = list(&mut conn, &search("respon crafted")).await;
		assert_eq!(
			ids(&details_words),
			[("GHSA-undici", "canopy"), ("GHSA-undici", "tamanu")],
			"details match every word by its beginning, in any order"
		);
		let query_syntax = list(&mut conn, &search("exhausts & | ! (:*")).await;
		assert_eq!(query_syntax.len(), 2, "the search's own symbols are not query syntax");

		let by_alias = list(&mut conn, &search("tupaia")).await;
		assert!(by_alias.is_empty());
		let by_source = list(&mut conn, &search("canopy")).await;
		assert_eq!(ids(&by_source), [("GHSA-undici", "canopy")]);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn filters_narrow_the_list() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		assorted(&mut conn).await;
		let with = |edit: &dyn Fn(&mut ListQuery)| {
			let mut query = ListQuery::default();
			edit(&mut query);
			query
		};

		assert_eq!(
			list(&mut conn, &with(&|q| q.source = Some(canopy())))
				.await
				.len(),
			1
		);
		assert_eq!(
			ids(&list(&mut conn, &with(&|q| q.scope = Some("optional".into()))).await),
			[("GHSA-lodash", "tamanu")]
		);
		assert_eq!(
			list(&mut conn, &with(&|q| q.scope = Some("development".into())))
				.await
				.len(),
			4 - 1,
			"the runtime-only undici item of tamanu has no development path"
		);
		assert_eq!(
			list(&mut conn, &with(&|q| q.min_severity = Some(Severity::High)))
				.await
				.len(),
			3
		);
		assert_eq!(
			list(&mut conn, &with(&|q| q.severity = Some(Severity::High)))
				.await
				.len(),
			2
		);
		assert_eq!(
			list(&mut conn, &with(&|q| q.package = Some("semver".into())))
				.await
				.len(),
			1
		);
		assert_eq!(
			list(
				&mut conn,
				&with(&|q| q.workspace_package = Some("app".into()))
			)
			.await
			.len(),
			1
		);
		assert_eq!(
			list(&mut conn, &with(&|q| q.has_suggestion = Some(true)))
				.await
				.len(),
			0
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn each_facet_ignores_its_own_filter() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		assorted(&mut conn).await;
		Decision::record(
			&mut conn,
			whole("GHSA-semver", tamanu(), Outcome::NotAffected),
		)
		.await
		.unwrap();

		let query = ListQuery {
			source: Some(tamanu()),
			status: Some(Status::Open),
			..Default::default()
		};
		let facets = ItemSummary::facets(&mut conn, &query).await.unwrap();
		let count = |counts: &[database::vulnerabilities::FacetCount], value: &str| {
			counts.iter().find(|c| c.value == value).unwrap().count
		};

		assert_eq!(count(&facets.source, "tamanu"), 2, "with the status filter");
		assert_eq!(
			count(&facets.source, "canopy"),
			1,
			"ignoring the source filter"
		);
		assert_eq!(count(&facets.status, "open"), 2, "with the source filter");
		assert_eq!(
			count(&facets.status, "not_affected"),
			1,
			"ignoring the status filter"
		);
		assert_eq!(count(&facets.status, "mixed"), 0);
		assert_eq!(count(&facets.severity, "critical"), 1);
		assert_eq!(count(&facets.suggestion, "no"), 2);
		assert_eq!(count(&facets.scope, "optional"), 1);
		assert_eq!(count(&facets.source, "linux"), 0, "every source is listed");

		let total = ItemSummary::list(&mut conn, &query).await.unwrap().total;
		assert_eq!(total, 2);

		assert_eq!(facets.totals.source, 3, "every filter but the source's");
		assert_eq!(
			facets.totals.scope, 2,
			"an item is one item however many scopes it has"
		);
		let summed: i64 = facets.scope.iter().map(|c| c.count).sum();
		assert!(
			summed > facets.totals.scope,
			"the choices of a scope add up to more than the items"
		);

		let counted = ItemSummary::count(&mut conn, &query).await.unwrap();
		assert_eq!(counted, total, "counting matches the page's total");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn group_by_scope_counts_a_multi_scope_item_in_each() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		assorted(&mut conn).await;
		let groups = ItemSummary::group(&mut conn, &ListQuery::default(), GroupBy::Scope)
			.await
			.unwrap();
		let count = |key: &str| groups.iter().find(|g| g.key == key).map(|g| g.count);
		assert_eq!(count("runtime"), Some(2));
		assert_eq!(count("development"), Some(4 - 1));
		assert_eq!(count("optional"), Some(1));

		let by_package = ItemSummary::group(&mut conn, &ListQuery::default(), GroupBy::Package)
			.await
			.unwrap();
		assert_eq!(by_package[0].key, "undici");
		assert_eq!(by_package[0].count, 2);

		let by_status = ItemSummary::group(&mut conn, &ListQuery::default(), GroupBy::Status)
			.await
			.unwrap();
		assert_eq!(by_status.len(), 1);
		assert_eq!(by_status[0].key, "open");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cursor_pages_through_without_overlap() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		assorted(&mut conn).await;
		let mut seen = Vec::new();
		let mut cursor = None;
		loop {
			let page = ItemSummary::list(
				&mut conn,
				&ListQuery {
					page_size: Some(3 - 1),
					cursor: cursor.clone(),
					..Default::default()
				},
			)
			.await
			.unwrap();
			assert_eq!(page.total, 4);
			seen.extend(page.items.iter().map(|i| (i.id.clone(), i.source.clone())));
			match page.next_cursor {
				Some(next) => cursor = Some(next),
				None => break,
			}
		}
		assert_eq!(seen.len(), 4);
		let mut unique = seen.clone();
		unique.sort();
		unique.dedup();
		assert_eq!(unique.len(), 4);

		let bad = ItemSummary::list(
			&mut conn,
			&ListQuery {
				cursor: Some("junk".into()),
				..Default::default()
			},
		)
		.await;
		assert!(matches!(bad, Err(AppError::BadRequest(_))));
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_sidebar_count_leaves_out_closed_and_decided_items() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		assorted(&mut conn).await;
		assert_eq!(ItemSummary::needs_triage_count(&mut conn).await.unwrap(), 4);
		Decision::record(&mut conn, whole("GHSA-semver", tamanu(), Outcome::WillFix))
			.await
			.unwrap();
		assert_eq!(ItemSummary::needs_triage_count(&mut conn).await.unwrap(), 3);
		read_repository(&mut conn, "canopy", "empty", &[]).await;
		assert_eq!(ItemSummary::needs_triage_count(&mut conn).await.unwrap(), 2);
	})
	.await;
}

// ── Matches ─────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn matches_for_package_versions_no_longer_counted_are_pruned() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		tamanu_item(&mut conn).await;
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
		let matched = |version: &str, ids: &[&str]| {
			(
				Counted {
					ecosystem: NPM.into(),
					name: "left-pad".into(),
					version: version.into(),
				},
				ids.iter().map(|id| (*id).to_owned()).collect::<Vec<_>>(),
			)
		};
		replace_matches(&mut conn, &[matched("1.0.0", &["GHSA-1", "GHSA-2"])])
			.await
			.unwrap();
		replace_matches(&mut conn, &[matched("2.0.0", &["GHSA-1"])])
			.await
			.unwrap();
		replace_matches(&mut conn, &[matched("1.0.0", &["GHSA-2"])])
			.await
			.unwrap();

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
