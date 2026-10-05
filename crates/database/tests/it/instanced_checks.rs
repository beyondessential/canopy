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
		CheckFiling, CheckGrading, CheckInstance, CheckOutcome, CheckStateStamp, GradingContext,
		GradingInputs, Incident, InstancedCheckFiling, Issue, NewEvent, Scope, StoredInstances,
		file_check, file_check_instances, grade_instances,
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
		 INSERT INTO machines (name, id, group_id) VALUES ('box', '{machine}', '{group}'); \
		 INSERT INTO applications (id, host, type, group_id, rank, machine_id) \
		 VALUES ('{application}', 'https://{application}.example', 'tamanu-central', \
		         '{group}', 'production', '{machine}')"
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
		"INSERT INTO machines (name, id, group_id) VALUES ('box', '{machine}', '{group}'); \
		 INSERT INTO applications (id, host, type, group_id, rank, machine_id) \
		 VALUES ('{application}', 'https://{application}.example', 'tamanu-central', \
		         '{group}', 'production', '{machine}')"
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
		&|graded| format!("{} degraded", graded.degraded().len()),
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

fn stored(issue: &Issue) -> StoredInstances {
	issue.stored_instances().expect("the state holds instances")
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
		assert_eq!(detail.0.len(), 3);
		assert_eq!(detail.degraded(), 2);
		assert_eq!(
			issue.detail,
			Some(json!({"fail_minutes": 30})),
			"the state's detail is the shared fields",
		);
		let harbour = &detail.0["dev-harbour"];
		assert_eq!(harbour.label.as_deref(), Some("Harbour Hospital"));
		assert_eq!(harbour.observed, CheckResult::Passed);
		assert_eq!(harbour.effective, CheckResult::Passed);
		assert_eq!(
			harbour.detail.as_ref().unwrap()["minutes_since_success"],
			1.2
		);
		assert_eq!(detail.0["dev-east"].label, None);
		assert_eq!(detail.0["dev-east"].effective, CheckResult::Warning);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_check_without_instances_stores_its_detail_as_reported() {
	TestDb::run(async |mut conn, _| {
		let s = seed(&mut conn).await;
		let detail = database::check_detail! {"free_percent": 4.2, "instances": 3};
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
		assert_eq!(issue.detail, Some(Value::Object(detail)));
		assert_eq!(issue.message, "low");
		assert!(issue.instances.is_none());
		assert!(issue.grading_context.is_none());
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
		assert!(stored(&issue).0.is_empty());
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
			detail.0["a"].effective,
			CheckResult::Warning,
			"the instance's own field wins over the shared one",
		);
		assert_eq!(
			detail.0["b"].effective,
			CheckResult::Failed,
			"an instance without the field reads the shared one",
		);
		assert_eq!(
			detail.0["c"].effective,
			CheckResult::Skipped,
			"check.result is the instance's own result",
		);
		assert_eq!(
			detail.0["d"].effective,
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
		assert_eq!(detail.0["dev-north"].effective, CheckResult::Skipped);
		assert_eq!(detail.0["dev-east"].effective, CheckResult::Warning);
		assert_eq!(detail.degraded(), 1);

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
		assert_eq!(stored(&issue).0["dev-north"].effective, CheckResult::Failed);
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
				stored(&issue).0["dev-north"].effective,
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
async fn a_group_instance_silence_regrades_every_covered_state_through_its_own_chain() {
	TestDb::run(async |mut conn, _| {
		let s = seed(&mut conn).await;
		let second = another_application(&mut conn, s.group).await;
		let third = another_application(&mut conn, s.group).await;
		let applications = [s.application, second, third];
		// A curated source's check is flat, so a group silence of it reaches the
		// group's machines as well as its applications.
		let scopes = applications
			.map(Scope::Application)
			.into_iter()
			.chain([Scope::Machine(s.machine)]);
		for scope in scopes.clone() {
			file(&mut conn, scope, CheckOutcome::Instances(devices())).await;
		}
		// One application has already silenced the other instance itself, which
		// its state alone is graded through.
		ServerSilencedRef::add(
			&mut conn,
			second,
			CANOPY_SOURCE,
			CHECK,
			Some("dev-harbour"),
			Some("op"),
		)
		.await
		.expect("application instance silence");

		ServerGroupSilencedRef::add(
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
		for scope in scopes.clone() {
			let issue = state(&mut conn, scope).await;
			let held = stored(&issue);
			assert_eq!(
				held.0["dev-north"].effective,
				CheckResult::Skipped,
				"{scope:?}"
			);
			if scope == Scope::Application(second) {
				assert_eq!(held.0["dev-harbour"].effective, CheckResult::Skipped);
				assert_eq!(issue.effective_result, Some(CheckResult::Skipped));
			} else {
				assert_eq!(
					held.0["dev-harbour"].effective,
					CheckResult::Passed,
					"{scope:?}"
				);
				assert_eq!(
					issue.effective_result,
					Some(CheckResult::Passed),
					"{scope:?}"
				);
			}
		}

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
		for scope in scopes {
			let issue = state(&mut conn, scope).await;
			assert_eq!(
				issue.effective_result,
				Some(CheckResult::Failed),
				"{scope:?}"
			);
			assert_eq!(
				stored(&issue).0["dev-harbour"].effective,
				if scope == Scope::Application(second) {
					CheckResult::Skipped
				} else {
					CheckResult::Passed
				},
				"{scope:?}"
			);
		}
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn silences_listed_together_each_present_their_own_instance() {
	TestDb::run(async |mut conn, _| {
		let s = seed(&mut conn).await;
		let second = another_application(&mut conn, s.group).await;
		let third = another_application(&mut conn, s.group).await;
		let unfiled = another_application(&mut conn, s.group).await;
		for application in [s.application, second, third] {
			file(
				&mut conn,
				Scope::Application(application),
				CheckOutcome::Instances(devices()),
			)
			.await;
		}
		for (application, instance) in [
			(s.application, Some("dev-north")),
			(second, Some("dev-harbour")),
			(second, Some("dev-gone")),
			(third, None),
			(unfiled, Some("dev-north")),
		] {
			let added = ServerSilencedRef::add(
				&mut conn,
				application,
				CANOPY_SOURCE,
				CHECK,
				instance,
				None,
			)
			.await
			.expect("silence");
			// What setting a silence presents agrees with what listing it does.
			let listed = ServerSilencedRef::list_for_server(&mut conn, application)
				.await
				.expect("list one")
				.into_iter()
				.find(|l| l.instance == added.instance)
				.expect("listed");
			assert_eq!(
				(added.instance_label, added.instance_reported),
				(listed.instance_label, listed.instance_reported),
			);
		}

		let listed = ServerSilencedRef::list_for_servers(
			&mut conn,
			&[s.application, second, third, unfiled],
		)
		.await
		.expect("list");
		let mut presented: Vec<(Uuid, Option<String>, Option<String>, Option<bool>)> = listed
			.into_iter()
			.map(|l| {
				(
					l.application_id,
					l.instance,
					l.instance_label,
					l.instance_reported,
				)
			})
			.collect();
		presented.sort();
		let mut expected = vec![
			(
				s.application,
				Some("dev-north".to_string()),
				Some("Northgate Clinic".to_string()),
				Some(true),
			),
			(
				second,
				Some("dev-harbour".to_string()),
				Some("Harbour Hospital".to_string()),
				Some(true),
			),
			(second, Some("dev-gone".to_string()), None, Some(false)),
			(third, None, None, None),
			(unfiled, Some("dev-north".to_string()), None, Some(false)),
		];
		expected.sort();
		assert_eq!(presented, expected);
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

/// A whole-check transform carrying rules and an instance silence at the same
/// scope: the silence is the more specific and has the last word, whichever
/// order the two rows were written (and so come back) in. The rules here
/// match the silenced instance and would replace its skipped result if they
/// applied after the silence.
// spec: CHK#silencing-one-instance
#[tokio::test(flavor = "multi_thread")]
async fn an_instance_silence_outlasts_a_whole_check_rule_at_its_scope_in_either_row_order() {
	TestDb::run(async |mut conn, _| {
		for silence_first in [false, true] {
			let s = seed(&mut conn).await;
			let scope = Scope::Application(s.application);
			let silence_instance = async |conn: &mut AsyncPgConnection| {
				ScopedCheckPolicy::silence(
					conn,
					scope,
					CANOPY_SOURCE,
					&Namespace::Flat,
					CHECK,
					Some("dev-north"),
					Some("op"),
				)
				.await
				.expect("instance silence");
			};
			let whole_check_rules = async |conn: &mut AsyncPgConnection| {
				let row = ScopedCheckPolicy::silence(
					conn,
					scope,
					CANOPY_SOURCE,
					&Namespace::Flat,
					CHECK,
					None,
					Some("op"),
				)
				.await
				.expect("whole-check row");
				conn.batch_execute(&format!(
					"UPDATE scoped_check_policies SET ceiling = NULL, rules = '{}' \
					 WHERE id = '{}'",
					json!({"if": [{"==": [{"var": "check.result"}, "failed"]}, "failed"]}),
					row.id,
				))
				.await
				.expect("rules on the whole-check row");
			};
			if silence_first {
				silence_instance(&mut conn).await;
				whole_check_rules(&mut conn).await;
			} else {
				whole_check_rules(&mut conn).await;
				silence_instance(&mut conn).await;
			}

			let issue = file(&mut conn, scope, CheckOutcome::Instances(devices())).await;
			let detail = stored(&issue);
			assert_eq!(
				detail.0["dev-north"].effective,
				CheckResult::Skipped,
				"the instance stays silenced (silence written first: {silence_first})",
			);
			assert_eq!(
				issue.effective_result,
				Some(CheckResult::Passed),
				"and the check settles on the instances left (silence written first: {silence_first})",
			);
		}
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_regrade_replays_the_report_and_tags_the_filing_graded_with() {
	TestDb::run(async |mut conn, _| {
		let s = seed(&mut conn).await;
		let source = "alertd";
		let r#ref = format!("health/{CHECK}");
		let ns = Namespace::Application(ApplicationType::TamanuCentral);
		CheckPolicy::upsert_default(&mut conn, source, &ns, CHECK)
			.await
			.expect("catalog");
		CheckPolicy::update(
			&mut conn,
			source,
			&ns,
			CHECK,
			CheckResult::Failed,
			false,
			None,
			"op",
		)
		.await
		.expect("review");
		// A tag only the filing supplies: the application stores none, so the
		// tags Canopy would read for it afresh do not carry it.
		CheckPolicy::update_rules(
			&mut conn,
			source,
			&ns,
			CHECK,
			Some(&ladder(json!({"if": [
				{"==": [{"var": "tag.tier"}, "lab"]}, "warning",
			]}))),
			"op",
		)
		.await
		.expect("rules");

		// Filed the way push ingestion files: graded with its own context,
		// then stamped from what was graded.
		let status: Map<String, Value> = json!({"release": "beta"}).as_object().cloned().unwrap();
		let tags: HashMap<String, Value> = [("tier".to_string(), json!("lab"))].into();
		let ctx = GradingContext {
			source,
			check: CHECK,
			status_extra: &status,
			tags: &tags,
		};
		let grading = CheckGrading::load(
			&mut conn,
			source,
			&ns,
			CHECK,
			FilingScope {
				application_id: Some(s.application),
				group_id: Some(s.group),
				..Default::default()
			},
		)
		.await
		.expect("grading");
		let graded = grade_instances(
			&grading,
			&ctx,
			None,
			&CheckOutcome::Instances(devices()),
			None,
		);
		assert_eq!(graded.effective, CheckResult::Warning);
		let stamp = CheckStateStamp::of_graded(CHECK, &graded, &ctx, Some("Sync is stale"));
		let filed = NewEvent {
			source: source.into(),
			r#ref: r#ref.clone(),
			description: Some("Sync is stale".into()),
			message: graded.message(CHECK),
			active: Some(true),
			occurred_at: None,
		}
		.save_with_state(&mut conn, s.application, None, Some(&stamp), false)
		.await
		.expect("file");
		assert_eq!(
			filed.grading_inputs(),
			Some(GradingInputs {
				status: status.clone(),
				tags: tags.clone(),
			}),
		);

		// Silencing the passing instance changes nothing a rule reads, so the
		// failing one stays graded as the filing graded it.
		ServerSilencedRef::add(
			&mut conn,
			s.application,
			source,
			&r#ref,
			Some("dev-harbour"),
			None,
		)
		.await
		.expect("silence");
		let issue =
			Issue::check_state_at(&mut conn, Scope::Application(s.application), source, &r#ref)
				.await
				.expect("read")
				.expect("filed");
		let held = stored(&issue);
		assert_eq!(held.0["dev-harbour"].effective, CheckResult::Skipped);
		assert_eq!(
			held.0["dev-north"].effective,
			CheckResult::Warning,
			"the re-grade read the tag the filing supplied",
		);
		assert_eq!(issue.effective_result, Some(CheckResult::Warning));
		assert_eq!(issue.grading_context, filed.grading_context);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_title_survives_a_silence_and_its_lifting() {
	TestDb::run(async |mut conn, _| {
		let s = seed(&mut conn).await;
		let scope = Scope::Application(s.application);
		let issue = file(&mut conn, scope, CheckOutcome::Instances(devices())).await;
		assert_eq!(issue.description.as_deref(), Some("sync is stale"));

		ServerSilencedRef::add(
			&mut conn,
			s.application,
			CANOPY_SOURCE,
			CHECK,
			Some("dev-north"),
			None,
		)
		.await
		.expect("silence");
		let issue = state(&mut conn, scope).await;
		assert!(!issue.active);
		assert_eq!(issue.description, None, "no headline while not degraded");
		assert_eq!(issue.title.as_deref(), Some("sync is stale"));

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
		assert!(issue.active);
		assert_eq!(
			issue.description.as_deref(),
			Some("sync is stale"),
			"back in trouble, with the title its last filing gave it",
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plain_check_whose_detail_looks_instanced_is_still_plain() {
	TestDb::run(async |mut conn, _| {
		let s = seed(&mut conn).await;
		let scope = Scope::Application(s.application);
		let detail = database::check_detail! {
			"instances": {"dev-north": {"observed": "failed", "effective": "failed"}},
			"degraded": 1,
			"total": 1,
		};
		let filed = file_check(
			&mut conn,
			CheckFiling {
				source: CANOPY_SOURCE,
				scope,
				device_id: None,
				check: CHECK,
				observed: CheckResult::Failed,
				title: None,
				message: "plain",
				detail: Some(detail.clone()),
				default_ceiling: CheckResult::Failed,
				default_escalates: false,
				documentation: None,
			},
		)
		.await
		.expect("file");
		assert!(filed.instances.is_none());
		assert!(filed.stored_instances().is_none());
		assert_eq!(filed.detail, Some(Value::Object(detail.clone())));

		// An instance silence naming a key in that detail reaches nothing.
		ServerSilencedRef::add(
			&mut conn,
			s.application,
			CANOPY_SOURCE,
			CHECK,
			Some("dev-north"),
			None,
		)
		.await
		.expect("silence");
		let issue = state(&mut conn, scope).await;
		assert_eq!(issue.effective_result, Some(CheckResult::Failed));
		assert_eq!(issue.detail, Some(Value::Object(detail)));
		assert_eq!(issue.message, "plain");
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
			detail.0.keys().collect::<Vec<_>>(),
			vec!["dev-harbour", "dev-north"],
			"no instance is recovered",
		);
		for held in detail.0.values() {
			assert_eq!(held.observed, CheckResult::Broken);
			assert_eq!(held.effective, CheckResult::Broken);
		}
		assert_eq!(
			detail.0["dev-north"].label.as_deref(),
			Some("Northgate Clinic"),
			"held with what it was last stored with",
		);
		assert_eq!(
			detail.0["dev-north"].detail.as_ref().unwrap()["minutes_since_success"],
			2875.4
		);

		// Still broken: still nothing recovered.
		let issue = file(&mut conn, scope, CheckOutcome::Broken).await;
		assert_eq!(stored(&issue).0.len(), 2);

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
			stored(&issue).0.keys().collect::<Vec<_>>(),
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
			stored(&issue).0["dev-harbour"].effective,
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
				detail: Some(database::check_detail! {"error": "timeout"}),
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

/// Only an object is a check's `detail` or its instances; under either name
/// anything else is one of a plain check's fields.
// spec: STA#health-and-detail
#[test]
fn non_object_structure_names_read_as_fields() {
	let entry = serde_json::json!({
		"check": "version_drift",
		"result": "passed",
		"instances": [{ "name": "api" }],
		"detail": "fine",
		"summary": "ok",
	});
	let reported =
		database::issues::ReportedCheck::from_entry(entry.as_object().expect("an object"))
			.expect("a readable check");
	let CheckOutcome::Instances(instances) = &reported.outcome else {
		panic!("a passed check is not broken");
	};
	assert_eq!(instances.len(), 1, "a plain check is its one instance");
	assert_eq!(instances[0].observed, CheckResult::Passed);
	assert_eq!(
		serde_json::Value::Object(reported.detail),
		serde_json::json!({ "instances": [{ "name": "api" }], "detail": "fine", "summary": "ok" }),
	);
}
