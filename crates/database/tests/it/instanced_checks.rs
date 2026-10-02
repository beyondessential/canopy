//! Checks graded through their instances, one instance silenced on a target,
//! and brokenness belonging to the whole check.
//!
//! spec: CHK#checks-with-instances
//! spec: CHK#silencing-one-instance

use std::collections::HashMap;

use commons_tests::db::TestDb;
use commons_types::namespace::Namespace;
use commons_types::server::app_type::ApplicationType;
use commons_types::status::CheckResult;
use database::{
	check_policies::{CheckPolicy, FilingScope, IfLadder, ScopedCheckPolicy},
	diesel_async::AsyncPgConnection,
	issues::{
		CheckFiling, CheckGrading, CheckInstance, CheckOutcome, GradingContext, Incident,
		InstancedCheckFiling, InstancedDetail, Issue, Scope, file_check, file_check_instances,
		grade_instances,
	},
	silenced_refs::{
		MachineSilencedRef, ServerGroupSilencedRef, ServerSilencedRef, is_silenced,
		silenced_health_checks_for_server,
	},
	statuses::CANOPY_SOURCE,
};
use diesel::prelude::*;
use diesel_async::{RunQueryDsl, SimpleAsyncConnection};
use serde_json::{Map, Value, json};
use uuid::Uuid;

const CHECK: &str = "sync-stale";

struct Seeded {
	group: Uuid,
	machine: Uuid,
	application: Uuid,
}

/// A group with a machine and one application on it. The group closes an
/// incident only after a linger window, so a close that happens at once is
/// one an operator caused rather than a recovery.
async fn seed(conn: &mut AsyncPgConnection) -> Seeded {
	let group = Uuid::new_v4();
	let machine = Uuid::new_v4();
	let application = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (id, name, slack_close_delay) \
		 VALUES ('{group}', 'instances-{group}', INTERVAL '5 minutes'); \
		 INSERT INTO machines (id, group_id) VALUES ('{machine}', '{group}'); \
		 INSERT INTO applications (id, host, type, group_id, machine_id) \
		 VALUES ('{application}', 'https://{application}.example', 'tamanu-central', \
		         '{group}', '{machine}')"
	))
	.await
	.expect("seed");
	Seeded {
		group,
		machine,
		application,
	}
}

/// A second application in the group, on its own machine.
async fn another_application(conn: &mut AsyncPgConnection, group: Uuid) -> Uuid {
	let machine = Uuid::new_v4();
	let application = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO machines (id, group_id) VALUES ('{machine}', '{group}'); \
		 INSERT INTO applications (id, host, type, group_id, machine_id) \
		 VALUES ('{application}', 'https://{application}.example', 'tamanu-central', \
		         '{group}', '{machine}')"
	))
	.await
	.expect("seed another application");
	application
}

fn instance(key: &str, label: Option<&str>, observed: CheckResult, detail: Value) -> CheckInstance {
	CheckInstance {
		key: key.into(),
		label: label.map(Into::into),
		observed,
		detail: Some(detail),
	}
}

/// Two devices, one failing and one passing, as central reports its sync.
fn devices() -> Vec<CheckInstance> {
	vec![
		instance(
			"dev-north",
			Some("Northgate Clinic"),
			CheckResult::Failed,
			json!({"minutes_since_success": 2875.4}),
		),
		instance(
			"dev-harbour",
			Some("Harbour Hospital"),
			CheckResult::Passed,
			json!({"minutes_since_success": 1.2}),
		),
	]
}

async fn file(conn: &mut AsyncPgConnection, scope: Scope, outcome: CheckOutcome) -> Issue {
	file_check_instances(
		conn,
		InstancedCheckFiling {
			source: CANOPY_SOURCE,
			scope,
			device_id: None,
			check: CHECK,
			title: Some("sync is stale"),
			detail: Some(json!({"fail_minutes": 30}).as_object().cloned().unwrap()),
			outcome,
			default_ceiling: CheckResult::Failed,
			default_escalates: true,
			documentation: None,
		},
		&|degraded| format!("{} degraded", degraded.len()),
	)
	.await
	.expect("file the check")
}

async fn state(conn: &mut AsyncPgConnection, scope: Scope) -> Issue {
	Issue::check_state_at(conn, scope, CANOPY_SOURCE, CHECK)
		.await
		.expect("read state")
		.expect("state filed")
}

fn stored(issue: &Issue) -> InstancedDetail {
	InstancedDetail::parse(issue.detail.as_ref()).expect("an instanced detail")
}

fn ladder(rules: Value) -> IfLadder {
	serde_json::from_value(rules).expect("ladder")
}

async fn incidents(conn: &mut AsyncPgConnection, group: Uuid) -> Vec<Incident> {
	use database::schema::incidents::dsl;
	dsl::incidents
		.select(Incident::as_select())
		.filter(dsl::server_group_id.eq(group))
		.load(conn)
		.await
		.expect("incidents")
}

// ── One grading path ────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn the_most_urgent_instance_settles_the_check_and_every_instance_is_stored() {
	TestDb::run(async |mut conn, _| {
		let s = seed(&mut conn).await;
		let mut instances = devices();
		instances.push(instance(
			"dev-east",
			None,
			CheckResult::Warning,
			json!({"minutes_since_success": 14.0}),
		));
		let issue = file(
			&mut conn,
			Scope::Application(s.application),
			CheckOutcome::Instances(instances),
		)
		.await;

		assert_eq!(issue.effective_result, Some(CheckResult::Failed));
		assert_eq!(issue.observed_result, Some(CheckResult::Failed));
		assert!(issue.escalates);

		let detail = stored(&issue);
		assert_eq!(detail.total, 3);
		assert_eq!(detail.degraded, 2);
		assert_eq!(detail.detail.as_ref().unwrap()["fail_minutes"], 30);
		let harbour = &detail.instances["dev-harbour"];
		assert_eq!(harbour.label.as_deref(), Some("Harbour Hospital"));
		assert_eq!(harbour.observed, CheckResult::Passed);
		assert_eq!(harbour.effective, CheckResult::Passed);
		assert_eq!(
			harbour.detail.as_ref().unwrap()["minutes_since_success"],
			1.2
		);
		assert_eq!(detail.instances["dev-east"].label, None);
		assert_eq!(detail.instances["dev-east"].effective, CheckResult::Warning);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_check_without_instances_stores_its_detail_as_reported() {
	TestDb::run(async |mut conn, _| {
		let s = seed(&mut conn).await;
		let detail = json!({"free_percent": 4.2, "instances": 3});
		let issue = file_check(
			&mut conn,
			CheckFiling {
				source: CANOPY_SOURCE,
				scope: Scope::Application(s.application),
				device_id: None,
				check: CHECK,
				observed: CheckResult::Warning,
				title: None,
				message: "low",
				detail: Some(detail.clone()),
				default_ceiling: CheckResult::Failed,
				default_escalates: false,
				documentation: None,
			},
		)
		.await
		.expect("file");
		assert_eq!(issue.detail, Some(detail));
		assert_eq!(issue.message, "low");
		assert!(InstancedDetail::parse(issue.detail.as_ref()).is_none());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_check_with_an_empty_set_of_instances_passes() {
	TestDb::run(async |mut conn, _| {
		let s = seed(&mut conn).await;
		let scope = Scope::Application(s.application);
		file(&mut conn, scope, CheckOutcome::Instances(devices())).await;
		let issue = file(&mut conn, scope, CheckOutcome::Instances(Vec::new())).await;
		assert_eq!(issue.effective_result, Some(CheckResult::Passed));
		assert!(!issue.active);
		assert!(stored(&issue).instances.is_empty());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rule_reads_the_instance_over_the_shared_detail_and_its_own_result() {
	TestDb::run(async |mut conn, _| {
		let s = seed(&mut conn).await;
		let scope = Scope::Application(s.application);
		file(&mut conn, scope, CheckOutcome::Instances(devices())).await;
		// `pace` is shared as "prompt" and overridden to "deferred" by one
		// instance; a failing deferred instance is only a warning. Another
		// rule pins `check.result`, which is each instance's own, and the last
		// a field only one instance carries.
		CheckPolicy::update_rules(
			&mut conn,
			CANOPY_SOURCE,
			&Namespace::Flat,
			CHECK,
			Some(&ladder(json!({"if": [
				{"==": [{"var": "check.pace"}, "deferred"]}, "warning",
				{"==": [{"var": "check.result"}, "passed"]}, "skipped",
				{"==": [{"var": "check.muted"}, true]}, "passed",
			]}))),
			"op",
		)
		.await
		.expect("rules");

		let issue = file_check_instances(
			&mut conn,
			InstancedCheckFiling {
				source: CANOPY_SOURCE,
				scope,
				device_id: None,
				check: CHECK,
				title: None,
				detail: Some(json!({"pace": "prompt"}).as_object().cloned().unwrap()),
				outcome: CheckOutcome::Instances(vec![
					instance("a", None, CheckResult::Failed, json!({"pace": "deferred"})),
					instance("b", None, CheckResult::Failed, json!({})),
					instance("c", None, CheckResult::Passed, json!({})),
					instance("d", None, CheckResult::Failed, json!({"muted": true})),
				]),
				default_ceiling: CheckResult::Failed,
				default_escalates: false,
				documentation: None,
			},
			&|_| String::new(),
		)
		.await
		.expect("file");
		let detail = stored(&issue);
		assert_eq!(
			detail.instances["a"].effective,
			CheckResult::Warning,
			"the instance's own field wins over the shared one",
		);
		assert_eq!(
			detail.instances["b"].effective,
			CheckResult::Failed,
			"an instance without the field reads the shared one",
		);
		assert_eq!(
			detail.instances["c"].effective,
			CheckResult::Skipped,
			"check.result is the instance's own result",
		);
		assert_eq!(
			detail.instances["d"].effective,
			CheckResult::Passed,
			"a rule pinning a field only one instance carries grades that one",
		);
		assert_eq!(issue.effective_result, Some(CheckResult::Failed));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rule_evaluated_for_an_instance_sees_the_report_and_the_tags() {
	TestDb::run(async |mut conn, _| {
		let ns = Namespace::Application(ApplicationType::TamanuCentral);
		CheckPolicy::upsert_default(&mut conn, "alertd", &ns, CHECK)
			.await
			.expect("catalog");
		CheckPolicy::update(
			&mut conn,
			"alertd",
			&ns,
			CHECK,
			CheckResult::Failed,
			false,
			None,
			"op",
		)
		.await
		.expect("review");
		CheckPolicy::update_rules(
			&mut conn,
			"alertd",
			&ns,
			CHECK,
			Some(&ladder(json!({"if": [
				{"==": [{"var": "status.tamanuVersion"}, "2.40.0"]}, "warning",
				{"==": [{"var": "tag.tier"}, "lab"]}, "passed",
			]}))),
			"op",
		)
		.await
		.expect("rules");
		let grading = CheckGrading::load(&mut conn, "alertd", &ns, CHECK, FilingScope::default())
			.await
			.expect("load grading");

		let grade = |status: Value, tags: &[(&str, &str)]| {
			let status: Map<String, Value> = status.as_object().cloned().unwrap();
			let tags: HashMap<String, Value> = tags
				.iter()
				.map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
				.collect();
			grade_instances(
				&grading,
				&GradingContext {
					source: "alertd",
					check: CHECK,
					status_extra: &status,
					tags: &tags,
				},
				None,
				&CheckOutcome::Instances(devices()),
				None,
			)
		};
		assert_eq!(grade(json!({}), &[]).effective, CheckResult::Failed);
		assert_eq!(
			grade(json!({"tamanuVersion": "2.40.0"}), &[]).effective,
			CheckResult::Warning,
		);
		assert_eq!(
			grade(json!({}), &[("tier", "lab")]).effective,
			CheckResult::Passed,
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_message_names_degraded_instances_by_label_and_counts_only_the_considered() {
	TestDb::run(async |mut conn, _| {
		let s = seed(&mut conn).await;
		let scope = Scope::Application(s.application);
		let mut instances = devices();
		instances.push(instance("dev-east", None, CheckResult::Warning, json!({})));
		file(&mut conn, scope, CheckOutcome::Instances(instances.clone())).await;
		let grading = CheckGrading::load(
			&mut conn,
			CANOPY_SOURCE,
			&Namespace::Flat,
			CHECK,
			FilingScope {
				application_id: Some(s.application),
				group_id: Some(s.group),
				..Default::default()
			},
		)
		.await
		.expect("grading");
		let empty = Map::new();
		let tags = HashMap::new();
		let ctx = GradingContext {
			source: CANOPY_SOURCE,
			check: CHECK,
			status_extra: &empty,
			tags: &tags,
		};
		let graded = grade_instances(
			&grading,
			&ctx,
			None,
			&CheckOutcome::Instances(instances),
			None,
		);
		let message = graded.message(CHECK);
		assert!(message.contains("Northgate Clinic (failed)"), "{message}");
		assert!(
			message.contains("dev-east (warning)"),
			"an unlabelled instance is named by its key: {message}",
		);
		assert!(
			!message.contains("Harbour"),
			"a passing instance is not named: {message}"
		);
		assert!(message.contains("2 of 3"), "{message}");

		// A silenced instance drops out of the count as well as the names.
		ServerSilencedRef::add(
			&mut conn,
			s.application,
			CANOPY_SOURCE,
			CHECK,
			Some("dev-east"),
			Some("op"),
		)
		.await
		.expect("silence");
		let issue = state(&mut conn, scope).await;
		assert!(!issue.message.contains("dev-east"), "{}", issue.message);
		assert!(
			issue.message.contains("Northgate Clinic"),
			"{}",
			issue.message
		);
	})
	.await
}

// ── Instance silences ───────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn an_instance_silence_on_an_application_quiets_that_instance_and_regrades_at_once() {
	TestDb::run(async |mut conn, _| {
		let s = seed(&mut conn).await;
		let scope = Scope::Application(s.application);
		let mut instances = devices();
		instances.push(instance("dev-east", None, CheckResult::Warning, json!({})));
		let issue = file(&mut conn, scope, CheckOutcome::Instances(instances)).await;
		assert_eq!(issue.effective_result, Some(CheckResult::Failed));
		let last_seen = issue.last_seen;

		let silence = ServerSilencedRef::add(
			&mut conn,
			s.application,
			CANOPY_SOURCE,
			CHECK,
			Some("dev-north"),
			Some("op"),
		)
		.await
		.expect("silence one instance");
		assert_eq!(silence.instance.as_deref(), Some("dev-north"));

		let issue = state(&mut conn, scope).await;
		assert_eq!(
			issue.effective_result,
			Some(CheckResult::Warning),
			"graded on the instances left, without waiting for a filing",
		);
		assert_eq!(
			issue.observed_result,
			Some(CheckResult::Failed),
			"what was observed is untouched",
		);
		assert_eq!(issue.last_seen, last_seen, "a re-grade is not a report");
		let detail = stored(&issue);
		assert_eq!(
			detail.instances["dev-north"].effective,
			CheckResult::Skipped
		);
		assert_eq!(detail.instances["dev-east"].effective, CheckResult::Warning);
		assert_eq!(detail.degraded, 1);

		// The whole check is not silenced, wherever that is read.
		assert!(
			!is_silenced(&mut conn, scope, Some(s.group), CANOPY_SOURCE, CHECK)
				.await
				.expect("is_silenced")
		);
		assert!(
			!silenced_health_checks_for_server(
				&mut conn,
				Some(s.application),
				Some(s.machine),
				Some(s.group),
				CANOPY_SOURCE,
			)
			.await
			.expect("silenced checks")
			.contains(CHECK)
		);

		// The next filing applies it too.
		let mut instances = devices();
		instances.push(instance("dev-east", None, CheckResult::Warning, json!({})));
		let issue = file(&mut conn, scope, CheckOutcome::Instances(instances)).await;
		assert_eq!(issue.effective_result, Some(CheckResult::Warning));

		ServerSilencedRef::remove(
			&mut conn,
			s.application,
			CANOPY_SOURCE,
			CHECK,
			Some("dev-north"),
		)
		.await
		.expect("unsilence");
		let issue = state(&mut conn, scope).await;
		assert_eq!(
			issue.effective_result,
			Some(CheckResult::Failed),
			"unsilencing re-grades at once too",
		);
		assert_eq!(
			stored(&issue).instances["dev-north"].effective,
			CheckResult::Failed
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn silencing_every_degraded_instance_leaves_the_passing_ones_and_silencing_all_skips() {
	TestDb::run(async |mut conn, _| {
		let s = seed(&mut conn).await;
		let scope = Scope::Application(s.application);
		file(&mut conn, scope, CheckOutcome::Instances(devices())).await;

		ServerSilencedRef::add(
			&mut conn,
			s.application,
			CANOPY_SOURCE,
			CHECK,
			Some("dev-north"),
			None,
		)
		.await
		.expect("silence the failing one");
		let issue = state(&mut conn, scope).await;
		assert_eq!(issue.effective_result, Some(CheckResult::Passed));
		assert!(!issue.active);

		ServerSilencedRef::add(
			&mut conn,
			s.application,
			CANOPY_SOURCE,
			CHECK,
			Some("dev-harbour"),
			None,
		)
		.await
		.expect("silence the passing one");
		let issue = state(&mut conn, scope).await;
		assert_eq!(issue.effective_result, Some(CheckResult::Skipped));
		assert!(
			!issue.escalates,
			"escalation is only read from instances that were not skipped",
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn silencing_the_instance_behind_an_incident_closes_it_at_once() {
	TestDb::run(async |mut conn, _| {
		let s = seed(&mut conn).await;
		let scope = Scope::Application(s.application);
		file(&mut conn, scope, CheckOutcome::Instances(devices())).await;
		let opened = incidents(&mut conn, s.group).await;
		assert_eq!(opened.len(), 1, "the failing instance opened an incident");
		assert!(opened[0].closed_at.is_none());

		ServerSilencedRef::add(
			&mut conn,
			s.application,
			CANOPY_SOURCE,
			CHECK,
			Some("dev-north"),
			Some("op"),
		)
		.await
		.expect("silence");
		let after = incidents(&mut conn, s.group).await;
		assert!(
			after[0].closed_at.is_some(),
			"an operator's silence closes rather than lingering as a recovery",
		);

		ServerSilencedRef::remove(
			&mut conn,
			s.application,
			CANOPY_SOURCE,
			CHECK,
			Some("dev-north"),
		)
		.await
		.expect("unsilence");
		let after = incidents(&mut conn, s.group).await;
		assert_eq!(
			after.iter().filter(|i| i.closed_at.is_none()).count(),
			1,
			"the failure is back once the silence is lifted",
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_group_instance_silence_quiets_that_key_on_every_application_in_the_group() {
	TestDb::run(async |mut conn, _| {
		let s = seed(&mut conn).await;
		let other = another_application(&mut conn, s.group).await;
		for application in [s.application, other] {
			file(
				&mut conn,
				Scope::Application(application),
				CheckOutcome::Instances(devices()),
			)
			.await;
		}

		let silence = ServerGroupSilencedRef::add(
			&mut conn,
			s.group,
			CANOPY_SOURCE,
			CHECK,
			None,
			Some("dev-north"),
			Some("op"),
		)
		.await
		.expect("group instance silence");
		assert_eq!(silence.instance.as_deref(), Some("dev-north"));

		for application in [s.application, other] {
			let issue = state(&mut conn, Scope::Application(application)).await;
			assert_eq!(issue.effective_result, Some(CheckResult::Passed));
			assert_eq!(
				stored(&issue).instances["dev-north"].effective,
				CheckResult::Skipped
			);
		}

		let listed = ServerGroupSilencedRef::list_for_group(&mut conn, s.group)
			.await
			.expect("list");
		assert_eq!(listed.len(), 1);
		assert_eq!(listed[0].instance.as_deref(), Some("dev-north"));

		ServerGroupSilencedRef::remove(
			&mut conn,
			s.group,
			CANOPY_SOURCE,
			CHECK,
			None,
			Some("dev-north"),
		)
		.await
		.expect("lift");
		for application in [s.application, other] {
			let issue = state(&mut conn, Scope::Application(application)).await;
			assert_eq!(issue.effective_result, Some(CheckResult::Failed));
		}
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_instance_silence_quiets_that_key_on_the_machine() {
	TestDb::run(async |mut conn, _| {
		let s = seed(&mut conn).await;
		let scope = Scope::Machine(s.machine);
		file(&mut conn, scope, CheckOutcome::Instances(devices())).await;

		MachineSilencedRef::add(
			&mut conn,
			s.machine,
			CANOPY_SOURCE,
			CHECK,
			Some("dev-north"),
			Some("op"),
		)
		.await
		.expect("machine instance silence");
		let issue = state(&mut conn, scope).await;
		assert_eq!(issue.effective_result, Some(CheckResult::Passed));

		let listed = MachineSilencedRef::list_for_machine(&mut conn, s.machine)
			.await
			.expect("list");
		assert_eq!(listed[0].instance.as_deref(), Some("dev-north"));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn whole_check_and_instance_silences_are_unique_apart_and_coexist() {
	TestDb::run(async |mut conn, _| {
		let s = seed(&mut conn).await;
		let scope = Scope::Application(s.application);
		file(&mut conn, scope, CheckOutcome::Instances(devices())).await;

		for instance in [
			None,
			None,
			Some("dev-north"),
			Some("dev-north"),
			Some("dev-harbour"),
		] {
			ScopedCheckPolicy::silence(
				&mut conn,
				scope,
				CANOPY_SOURCE,
				&Namespace::Flat,
				CHECK,
				instance,
				Some("op"),
			)
			.await
			.expect("silence");
		}
		let mut keys: Vec<Option<String>> = ScopedCheckPolicy::list_silences(&mut conn, scope)
			.await
			.expect("list")
			.into_iter()
			.map(|p| p.instance_key)
			.collect();
		keys.sort();
		assert_eq!(
			keys,
			vec![None, Some("dev-harbour".into()), Some("dev-north".into())],
			"one row per (check, instance), the whole-check row among them",
		);

		// Storage holds the same line a caller writing around the model would
		// hit: a second row for the same instance is refused.
		let duplicate = conn
			.batch_execute(&format!(
				"INSERT INTO scoped_check_policies \
				 (source, check_name, application_id, ceiling, instance_key) \
				 VALUES ('{CANOPY_SOURCE}', '{CHECK}', '{}', 'skipped', 'dev-north')",
				s.application
			))
			.await;
		assert!(duplicate.is_err(), "a second row for one instance");
		let empty = conn
			.batch_execute(&format!(
				"INSERT INTO scoped_check_policies \
				 (source, check_name, application_id, ceiling, instance_key) \
				 VALUES ('{CANOPY_SOURCE}', 'other', '{}', 'skipped', '')",
				s.application
			))
			.await;
		assert!(empty.is_err(), "the empty key is refused");
		assert!(
			ScopedCheckPolicy::silence(
				&mut conn,
				scope,
				CANOPY_SOURCE,
				&Namespace::Flat,
				CHECK,
				Some(""),
				None,
			)
			.await
			.is_err()
		);

		// Lifting one instance's silence leaves the whole-check one alone.
		ServerSilencedRef::remove(
			&mut conn,
			s.application,
			CANOPY_SOURCE,
			CHECK,
			Some("dev-north"),
		)
		.await
		.expect("lift one instance");
		assert!(
			is_silenced(&mut conn, scope, Some(s.group), CANOPY_SOURCE, CHECK)
				.await
				.expect("is_silenced"),
			"the whole-check silence still holds",
		);
	})
	.await
}

// ── Brokenness is whole-check ───────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn a_broken_check_holds_its_instances_as_broken_and_retains_its_failure() {
	TestDb::run(async |mut conn, _| {
		let s = seed(&mut conn).await;
		let scope = Scope::Application(s.application);
		file(&mut conn, scope, CheckOutcome::Instances(devices())).await;

		let issue = file(&mut conn, scope, CheckOutcome::Broken).await;
		assert_eq!(issue.observed_result, Some(CheckResult::Broken));
		assert_eq!(
			issue.effective_result,
			Some(CheckResult::Failed),
			"the open failure is retained through the brokenness",
		);
		assert!(issue.escalates, "with the escalation it had");
		assert!(issue.active);
		let detail = stored(&issue);
		assert_eq!(
			detail.instances.keys().collect::<Vec<_>>(),
			vec!["dev-harbour", "dev-north"],
			"no instance is recovered",
		);
		for held in detail.instances.values() {
			assert_eq!(held.observed, CheckResult::Broken);
			assert_eq!(held.effective, CheckResult::Broken);
		}
		assert_eq!(
			detail.instances["dev-north"].label.as_deref(),
			Some("Northgate Clinic"),
			"held with what it was last stored with",
		);
		assert_eq!(
			detail.instances["dev-north"].detail.as_ref().unwrap()["minutes_since_success"],
			2875.4
		);

		// Still broken: still nothing recovered.
		let issue = file(&mut conn, scope, CheckOutcome::Broken).await;
		assert_eq!(stored(&issue).total, 2);

		// The next definite filing grades its instances afresh.
		let issue = file(
			&mut conn,
			scope,
			CheckOutcome::Instances(vec![instance(
				"dev-harbour",
				None,
				CheckResult::Passed,
				json!({}),
			)]),
		)
		.await;
		assert_eq!(issue.effective_result, Some(CheckResult::Passed));
		assert_eq!(
			stored(&issue).instances.keys().collect::<Vec<_>>(),
			vec!["dev-harbour"]
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_broken_check_with_nothing_definite_to_retain_counts_as_broken() {
	TestDb::run(async |mut conn, _| {
		let s = seed(&mut conn).await;
		let scope = Scope::Application(s.application);
		file(
			&mut conn,
			scope,
			CheckOutcome::Instances(vec![instance(
				"dev-harbour",
				None,
				CheckResult::Passed,
				json!({}),
			)]),
		)
		.await;
		let issue = file(&mut conn, scope, CheckOutcome::Broken).await;
		assert_eq!(issue.effective_result, Some(CheckResult::Broken));
		assert_eq!(
			stored(&issue).instances["dev-harbour"].effective,
			CheckResult::Broken
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_broken_check_that_held_no_instances_is_its_plain_self() {
	TestDb::run(async |mut conn, _| {
		let s = seed(&mut conn).await;
		let issue = file_check(
			&mut conn,
			CheckFiling {
				source: CANOPY_SOURCE,
				scope: Scope::Application(s.application),
				device_id: None,
				check: CHECK,
				observed: CheckResult::Broken,
				title: None,
				message: "could not run",
				detail: Some(json!({"error": "timeout"})),
				default_ceiling: CheckResult::Failed,
				default_escalates: false,
				documentation: None,
			},
		)
		.await
		.expect("file");
		assert_eq!(issue.effective_result, Some(CheckResult::Broken));
		assert_eq!(issue.detail, Some(json!({"error": "timeout"})));
	})
	.await
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "brokenness is the whole check's")]
fn a_broken_instance_is_refused() {
	let empty = Map::new();
	let tags = HashMap::new();
	grade_instances(
		&CheckGrading::default(),
		&GradingContext {
			source: CANOPY_SOURCE,
			check: CHECK,
			status_extra: &empty,
			tags: &tags,
		},
		None,
		&CheckOutcome::Instances(vec![instance("a", None, CheckResult::Broken, json!({}))]),
		None,
	);
}
