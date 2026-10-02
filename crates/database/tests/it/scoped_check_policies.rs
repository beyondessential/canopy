//! Scoped check policies: transforms applied after the fleet catalog —
//! fleet, then group, then server — with the operator silence as a
//! scoped skipped-ceiling. Grade-time behaviour is exercised through
//! `file_check`; the silence CRUD through the `silenced_refs` facade.

use commons_types::namespace::Namespace;

use crate::helpers::app_ns;
use commons_types::status::CheckResult;
use database::{
	check_policies::{CheckPolicy, EvaluationContext, FilingScope, ScopedCheckPolicy},
	issues::{CheckFiling, Issue, Scope, file_check},
	silenced_refs::ServerSilencedRef,
	statuses::CANOPY_SOURCE,
};
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::RunQueryDsl;
use uuid::Uuid;

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

async fn insert_group(conn: &mut diesel_async::AsyncPgConnection) -> Uuid {
	let row: RowId = sql_query("INSERT INTO server_groups (name) VALUES ('g') RETURNING id")
		.get_result(conn)
		.await
		.expect("insert group");
	row.id
}

async fn insert_server(conn: &mut diesel_async::AsyncPgConnection, group_id: Option<Uuid>) -> Uuid {
	let row: RowId = sql_query(
		"WITH m AS (INSERT INTO machines (group_id) VALUES ($1) RETURNING id) INSERT INTO applications (type, host, group_id, machine_id) SELECT 'tamanu-central', 'http://scoped.invalid/', $1, m.id FROM m RETURNING id",
	)
	.bind::<sql_types::Nullable<sql_types::Uuid>, _>(group_id)
	.get_result(conn)
	.await
	.expect("insert server");
	row.id
}

fn filing(server_id: Uuid, check: &str, observed: CheckResult) -> CheckFiling<'_> {
	CheckFiling {
		source: CANOPY_SOURCE,
		scope: Scope::Application(server_id),
		device_id: None,
		check,
		observed,
		title: Some("Scoped test"),
		message: "scoped test filing",
		detail: None,
		default_ceiling: CheckResult::Failed,
		default_escalates: false,
		documentation: None,
	}
}

async fn state_for(
	conn: &mut diesel_async::AsyncPgConnection,
	server_id: Uuid,
	check: &str,
) -> Issue {
	Issue::list_by_source_ref(conn, CANOPY_SOURCE, check, &[server_id])
		.await
		.expect("list")
		.into_iter()
		.next()
		.expect("state row filed")
}

#[tokio::test(flavor = "multi_thread")]
async fn server_silence_grades_filings_to_skipped() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn, None).await;
		ServerSilencedRef::add(&mut conn, server_id, CANOPY_SOURCE, "noisy", None, None)
			.await
			.expect("silence");

		file_check(&mut conn, filing(server_id, "noisy", CheckResult::Failed))
			.await
			.expect("file");

		let state = state_for(&mut conn, server_id, "noisy").await;
		assert_eq!(state.observed_result, Some(CheckResult::Failed));
		assert_eq!(
			state.effective_result,
			Some(CheckResult::Skipped),
			"a silenced check records its observation but grades to skipped"
		);
		assert!(!state.active);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn group_silence_covers_member_servers() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group_id = insert_group(&mut conn).await;
		let server_id = insert_server(&mut conn, Some(group_id)).await;
		ScopedCheckPolicy::silence(
			&mut conn,
			Scope::Group(group_id),
			CANOPY_SOURCE,
			&Namespace::Flat,
			"noisy",
			None,
			Some("op"),
		)
		.await
		.expect("group silence");

		file_check(&mut conn, filing(server_id, "noisy", CheckResult::Failed))
			.await
			.expect("file");
		let state = state_for(&mut conn, server_id, "noisy").await;
		assert_eq!(state.effective_result, Some(CheckResult::Skipped));

		// Lifting the group silence restores normal grading on the next
		// filing.
		ScopedCheckPolicy::unsilence(
			&mut conn,
			Scope::Group(group_id),
			CANOPY_SOURCE,
			&Namespace::Flat,
			"noisy",
			None,
		)
		.await
		.expect("unsilence");
		file_check(&mut conn, filing(server_id, "noisy", CheckResult::Failed))
			.await
			.expect("file again");
		let state = state_for(&mut conn, server_id, "noisy").await;
		assert_eq!(state.effective_result, Some(CheckResult::Failed));
		assert!(state.active);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn server_scoped_rule_can_upgrade_past_the_fleet_ceiling() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn, None).await;

		// Fleet catalog grades this check down to warning...
		CheckPolicy::register(
			&mut conn,
			CANOPY_SOURCE,
			&Namespace::Flat,
			"tiered",
			CheckResult::Warning,
			false,
			None,
		)
		.await
		.expect("register");

		// ...but a server-scoped rule upgrades observed failures back to
		// failed for this one server (the backend admits arbitrary scoped
		// transforms; only silences are surfaced in the UI).
		sql_query(
			r#"
				INSERT INTO scoped_check_policies (source, check_name, application_id, rules)
				VALUES ($1, 'tiered', $2,
					'{"if": [{"==": [{"var": "check.result"}, "failed"]}, "failed"]}'::jsonb)
			"#,
		)
		.bind::<sql_types::Text, _>(CANOPY_SOURCE)
		.bind::<sql_types::Uuid, _>(server_id)
		.execute(&mut conn)
		.await
		.expect("scoped rule");

		let ctx = EvaluationContext {
			status_extra: &serde_json::Map::new(),
			check_extra: &serde_json::Map::from_iter([(
				"result".to_string(),
				serde_json::Value::String("failed".into()),
			)]),
			tags: &Default::default(),
		};
		let graded = CheckPolicy::apply_scoped(
			&mut conn,
			CANOPY_SOURCE,
			&Namespace::Flat,
			"tiered",
			CheckResult::Failed,
			&ctx,
			FilingScope {
				application_id: Some(server_id),
				..Default::default()
			},
		)
		.await
		.expect("apply");
		assert_eq!(
			graded.effective,
			CheckResult::Failed,
			"the server-scoped rule has the last word over the fleet ceiling"
		);

		// A different server without the scoped rule keeps the fleet grade.
		let other = insert_server(&mut conn, None).await;
		let graded = CheckPolicy::apply_scoped(
			&mut conn,
			CANOPY_SOURCE,
			&Namespace::Flat,
			"tiered",
			CheckResult::Failed,
			&ctx,
			FilingScope {
				application_id: Some(other),
				..Default::default()
			},
		)
		.await
		.expect("apply other");
		assert_eq!(graded.effective, CheckResult::Warning);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn silence_on_a_scoped_rule_row_keeps_the_rules() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn, None).await;
		let (subject, application_type) = app_ns().to_columns();
		sql_query(
			r#"
				INSERT INTO scoped_check_policies
					(source, subject, application_type, check_name, application_id, rules)
				VALUES ('alertd', $2, $3, 'ruled', $1,
					'{"if": [{"==": [{"var": "check.result"}, "failed"]}, "failed"]}'::jsonb)
			"#,
		)
		.bind::<sql_types::Uuid, _>(server_id)
		.bind::<sql_types::Nullable<sql_types::Text>, _>(subject)
		.bind::<sql_types::Nullable<sql_types::Text>, _>(application_type)
		.execute(&mut conn)
		.await
		.expect("scoped rule");

		ScopedCheckPolicy::silence(
			&mut conn,
			Scope::Application(server_id),
			"alertd",
			&app_ns(),
			"ruled",
			None,
			Some("op"),
		)
		.await
		.expect("silence");
		let row = ScopedCheckPolicy::get(
			&mut conn,
			Scope::Application(server_id),
			"alertd",
			&app_ns(),
			"ruled",
			None,
		)
		.await
		.expect("get")
		.expect("row exists");
		assert_eq!(row.ceiling.as_deref(), Some("skipped"));
		assert!(row.rules.is_some(), "silencing keeps the scoped rules");

		// Unsilencing lifts the ceiling but keeps the rules row.
		ScopedCheckPolicy::unsilence(
			&mut conn,
			Scope::Application(server_id),
			"alertd",
			&app_ns(),
			"ruled",
			None,
		)
		.await
		.expect("unsilence");
		let row = ScopedCheckPolicy::get(
			&mut conn,
			Scope::Application(server_id),
			"alertd",
			&app_ns(),
			"ruled",
			None,
		)
		.await
		.expect("get")
		.expect("row still exists");
		assert_eq!(row.ceiling, None);
		assert!(row.rules.is_some());

		// A plain silence row deletes outright on unsilence.
		ScopedCheckPolicy::silence(
			&mut conn,
			Scope::Application(server_id),
			"alertd",
			&app_ns(),
			"plain",
			None,
			None,
		)
		.await
		.expect("plain silence");
		ScopedCheckPolicy::unsilence(
			&mut conn,
			Scope::Application(server_id),
			"alertd",
			&app_ns(),
			"plain",
			None,
		)
		.await
		.expect("plain unsilence");
		assert!(
			ScopedCheckPolicy::get(
				&mut conn,
				Scope::Application(server_id),
				"alertd",
				&app_ns(),
				"plain",
				None
			)
			.await
			.expect("get")
			.is_none()
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn decommission_clears_the_checks_silences() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn, None).await;
		CheckPolicy::upsert_default(&mut conn, "alertd", &app_ns(), "noisy")
			.await
			.expect("seed catalog");
		ScopedCheckPolicy::silence(
			&mut conn,
			Scope::Application(server_id),
			"alertd",
			&app_ns(),
			"noisy",
			None,
			Some("op"),
		)
		.await
		.expect("silence");

		CheckPolicy::decommission(&mut conn, "alertd", &app_ns(), "noisy", "op")
			.await
			.expect("decommission");

		// The silence row is deleted outright, not just hidden.
		assert!(
			ScopedCheckPolicy::get(
				&mut conn,
				Scope::Application(server_id),
				"alertd",
				&app_ns(),
				"noisy",
				None
			)
			.await
			.expect("get")
			.is_none(),
			"decommissioning a check clears its silences",
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn list_silences_excludes_orphaned_check_silences() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn, None).await;
		CheckPolicy::upsert_default(&mut conn, "bestool-alertd", &app_ns(), "sync")
			.await
			.expect("seed catalog");
		ScopedCheckPolicy::silence(
			&mut conn,
			Scope::Application(server_id),
			"bestool-alertd",
			&app_ns(),
			"sync",
			None,
			Some("op"),
		)
		.await
		.expect("silence");
		assert_eq!(
			ScopedCheckPolicy::list_silences(&mut conn, Scope::Application(server_id))
				.await
				.expect("list")
				.len(),
			1,
		);

		// Orphan the check: its catalog row goes, its silence row lingers.
		sql_query("DELETE FROM check_policies WHERE source = 'bestool-alertd'")
			.execute(&mut conn)
			.await
			.expect("orphan the check");

		assert!(
			ScopedCheckPolicy::list_silences(&mut conn, Scope::Application(server_id))
				.await
				.expect("list")
				.is_empty(),
			"a silence for a dead (orphaned) check is not listed",
		);
	})
	.await
}
