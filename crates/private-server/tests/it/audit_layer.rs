//! The audit layer and extractor (spec `AUD`), exercised on routes built for the
//! purpose so each behaviour is seen on its own.
//!
//! The routes declare real catalogue actions; what they do is the minimum to
//! show whether a change stood or fell with its entry.

use std::sync::Arc;

use axum::{Json, Router, middleware};
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, Result};
use commons_servers::audit::{
	ActorSource, Audit, AuditMap, AuditState, Undeclared, enforce,
};
use commons_tests::{axum_test::TestServer, db::TestDb};
use database::{
	Db,
	admins::Admin,
	audit::{self, Action, Actor, Filter, Record, RecordKind, Status},
};
use uuid::Uuid;

fn record(name: &str) -> Record {
	Record::new(RecordKind::MaintenanceWindow, Uuid::new_v4(), name)
}

fn lifted(name: &str) -> Action {
	Action::MaintenanceLifted {
		window: record(name),
		target: Record::new(RecordKind::Machine, Uuid::new_v4(), "box"),
		rank: None,
		already_ended: false,
	}
}

#[utoipa::path(post, path = "/silent", responses((status = 200)))]
async fn silent() -> Result<Json<()>> {
	Ok(Json(()))
}

#[utoipa::path(post, path = "/records", responses((status = 200)))]
async fn records(audit: Audit) -> Result<Json<()>> {
	let mut tx = audit.begin().await?;
	Admin::add(&mut tx, "kept@example.com").await?;
	tx.record(lifted("one")).await?;
	Ok(Json(()))
}

#[utoipa::path(post, path = "/bulk", responses((status = 200)))]
async fn bulk(audit: Audit) -> Result<Json<()>> {
	let mut tx = audit.begin().await?;
	for name in ["a", "b", "c"] {
		tx.record(lifted(name)).await?;
	}
	Ok(Json(()))
}

#[utoipa::path(post, path = "/forgets", responses((status = 200)))]
async fn forgets(audit: Audit) -> Result<Json<()>> {
	let mut tx = audit.begin().await?;
	Admin::add(&mut tx, "forgotten@example.com").await?;
	Ok(Json(()))
}

#[utoipa::path(post, path = "/undeclared", responses((status = 200)))]
async fn undeclared(audit: Audit) -> Result<Json<()>> {
	let mut tx = audit.begin().await?;
	Admin::add(&mut tx, "undeclared@example.com").await?;
	tx.record(Action::MaintenanceDeclared {
		window: record("w"),
		target: Record::new(RecordKind::Machine, Uuid::new_v4(), "box"),
		upgrade_plan: None,
		rank: None,
		expected_end: jiff::Timestamp::now(),
		note: None,
	})
	.await?;
	Ok(Json(()))
}

#[utoipa::path(post, path = "/errors", responses((status = 200)))]
async fn errors(audit: Audit) -> Result<Json<()>> {
	let mut tx = audit.begin().await?;
	Admin::add(&mut tx, "errored@example.com").await?;
	tx.record(lifted("errored")).await?;
	Err(AppError::BadRequest("no".into()))
}

/// Commits an entry ahead, then fails without settling it: an outside effect
/// that may have taken place.
#[utoipa::path(post, path = "/ahead_then_errors", responses((status = 200)))]
async fn ahead_then_errors(audit: Audit) -> Result<Json<()>> {
	let _pending = audit.commit_ahead(lifted("ahead")).await?;
	let mut tx = audit.begin().await?;
	Admin::add(&mut tx, "ahead@example.com").await?;
	Err(AppError::BadRequest("no".into()))
}

/// Commits ahead, learns the effect took place, and settles done with what it
/// learned, in the same transaction as the rest of its work.
#[utoipa::path(post, path = "/ahead_then_done", responses((status = 200)))]
async fn ahead_then_done(audit: Audit) -> Result<Json<()>> {
	let pending = audit.commit_ahead(lifted("effect")).await?;
	let mut tx = audit.begin().await?;
	Admin::add(&mut tx, "effect@example.com").await?;
	tx.settle_done(&pending, Some(serde_json::json!({ "access_key": "AKIA" })))
		.await?;
	Ok(Json(()))
}

/// Commits ahead, knows the effect did not take place, and settles failed
/// before the handler's own work is rolled back.
#[utoipa::path(post, path = "/ahead_then_failed", responses((status = 200)))]
async fn ahead_then_failed(audit: Audit) -> Result<Json<()>> {
	let pending = audit.commit_ahead(lifted("refused")).await?;
	let mut tx = audit.begin().await?;
	Admin::add(&mut tx, "refused@example.com").await?;
	audit.settle_failed(&pending, None).await?;
	Err(AppError::BadRequest("refused".into()))
}

/// An audited read: entry first, then the read, which the layer settles.
#[utoipa::path(post, path = "/read", responses((status = 200)))]
async fn read(audit: Audit) -> Result<Json<()>> {
	audit.read(lifted("read")).await?;
	Ok(Json(()))
}

#[utoipa::path(post, path = "/read_errors", responses((status = 200)))]
async fn read_errors(audit: Audit) -> Result<Json<()>> {
	audit.read(lifted("read_errors")).await?;
	Err(AppError::BadRequest("the read failed".into()))
}

#[utoipa::path(post, path = "/device", responses((status = 200)))]
async fn device(audit: Audit) -> Result<Json<()>> {
	let id = Uuid::new_v4();
	audit.acting_as(Actor::Device {
		id,
		name: None,
		role: commons_types::device::DeviceRole::Machine,
		application: None,
	});
	let mut tx = audit.begin().await?;
	tx.record(lifted("device")).await?;
	Ok(Json(()))
}

fn app(db: Db, actor: ActorSource) -> TestServer {
	let (router, spec) = OpenApiRouter::new()
		.routes(routes!(write, silent: silent))
		.routes(routes!(write, audit(MaintenanceLifted): records))
		.routes(routes!(write, audit(MaintenanceLifted): bulk))
		.routes(routes!(write, audit(MaintenanceLifted): forgets))
		.routes(routes!(write, audit(MaintenanceLifted): undeclared))
		.routes(routes!(write, audit(MaintenanceLifted): errors))
		.routes(routes!(write, audit(MaintenanceLifted): ahead_then_errors))
		.routes(routes!(write, audit(MaintenanceLifted): ahead_then_done))
		.routes(routes!(write, audit(MaintenanceLifted): ahead_then_failed))
		.routes(routes!(write, audit(MaintenanceLifted): read))
		.routes(routes!(write, audit(MaintenanceLifted): read_errors))
		.routes(routes!(write, audit(MaintenanceLifted): device))
		.split_for_parts();
	let state = AuditState {
		db,
		map: Arc::new(AuditMap::from_openapi(&spec)),
		actor,
		undeclared: Undeclared::Refuse,
		mount: "",
	};
	TestServer::new(Router::from(router).layer(middleware::from_fn_with_state(state, enforce)))
}

async fn entries(conn: &mut database::diesel_async::AsyncPgConnection) -> Vec<audit::Entry> {
	audit::find(
		conn,
		&Filter {
			limit: 100,
			..Default::default()
		},
	)
	.await
	.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_silent_route_records_nothing_and_its_change_stands() {
	TestDb::run(async |mut conn, url| {
		let server = app(database::init_to(&url), ActorSource::Operator);
		server.post("/silent").await.assert_status_ok();
		assert!(entries(&mut conn).await.is_empty());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_audited_route_commits_its_change_with_its_entry_naming_the_operator() {
	TestDb::run(async |mut conn, url| {
		let server = app(database::init_to(&url), ActorSource::Operator);
		server.post("/records").await.assert_status_ok();

		assert!(Admin::check_email(&mut conn, "kept@example.com").await.unwrap());
		let log = entries(&mut conn).await;
		assert_eq!(log.len(), 1);
		assert_eq!(log[0].action, "maintenance_lifted");
		assert_eq!(log[0].status, Status::Done);
		assert!(matches!(
			&log[0].actor,
			Actor::Operator { login, .. } if login == "admin@localhost"
		));
		assert_eq!(log[0].records.len(), 2);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bulk_change_records_an_entry_for_each_action() {
	TestDb::run(async |mut conn, url| {
		let server = app(database::init_to(&url), ActorSource::Operator);
		server.post("/bulk").await.assert_status_ok();
		assert_eq!(entries(&mut conn).await.len(), 3);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_audited_handler_that_records_nothing_fails_and_changes_nothing() {
	TestDb::run(async |mut conn, url| {
		let server = app(database::init_to(&url), ActorSource::Operator);
		server
			.post("/forgets")
			.await
			.assert_status(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
		assert!(
			!Admin::check_email(&mut conn, "forgotten@example.com")
				.await
				.unwrap()
		);
		assert!(entries(&mut conn).await.is_empty());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn recording_an_action_the_route_does_not_declare_fails_and_changes_nothing() {
	TestDb::run(async |mut conn, url| {
		let server = app(database::init_to(&url), ActorSource::Operator);
		server
			.post("/undeclared")
			.await
			.assert_status(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
		assert!(
			!Admin::check_email(&mut conn, "undeclared@example.com")
				.await
				.unwrap()
		);
		assert!(entries(&mut conn).await.is_empty());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_audited_action_that_errors_leaves_no_entry_and_no_change() {
	TestDb::run(async |mut conn, url| {
		let server = app(database::init_to(&url), ActorSource::Operator);
		server
			.post("/errors")
			.await
			.assert_status(axum::http::StatusCode::BAD_REQUEST);
		assert!(
			!Admin::check_email(&mut conn, "errored@example.com")
				.await
				.unwrap()
		);
		assert!(entries(&mut conn).await.is_empty());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_entry_committed_ahead_stands_when_the_handler_errors_and_is_hidden_until_it_lapses() {
	TestDb::run(async |mut conn, url| {
		let server = app(database::init_to(&url), ActorSource::Operator);
		server
			.post("/ahead_then_errors")
			.await
			.assert_status(axum::http::StatusCode::BAD_REQUEST);

		assert!(
			!Admin::check_email(&mut conn, "ahead@example.com")
				.await
				.unwrap(),
			"the handler's own change rolled back"
		);
		assert!(
			entries(&mut conn).await.is_empty(),
			"a pending entry is not presented while it can still be settled"
		);

		// An hour on, nothing can settle it any more, and it reads as unconfirmed.
		diesel_async_run(
			&mut conn,
			"INSERT INTO audit_entries (recorded_at, actor_kind, actor_id, action, outcome) \
			 SELECT recorded_at - interval '2 hours', actor_kind, actor_id, 'maintenance_lifted', 'pending' \
			 FROM audit_entries WHERE outcome = 'pending'",
		)
		.await;
		let log = entries(&mut conn).await;
		assert_eq!(log.len(), 1);
		assert_eq!(log[0].status, Status::Unconfirmed);
	})
	.await
}

async fn diesel_async_run(conn: &mut database::diesel_async::AsyncPgConnection, sql: &str) {
	use commons_tests::diesel_async::SimpleAsyncConnection;
	conn.batch_execute(sql).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn settling_done_commits_with_the_work_and_adds_what_the_action_learned() {
	TestDb::run(async |mut conn, url| {
		let server = app(database::init_to(&url), ActorSource::Operator);
		server.post("/ahead_then_done").await.assert_status_ok();

		assert!(Admin::check_email(&mut conn, "effect@example.com").await.unwrap());
		let log = entries(&mut conn).await;
		assert_eq!(log.len(), 1);
		assert_eq!(log[0].status, Status::Done);
		assert_eq!(log[0].detail["access_key"], "AKIA");
		assert_eq!(
			log[0].detail["already_ended"], false,
			"settling only adds to the detail the action recorded"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failure_settled_stands_though_the_handler_rolls_back() {
	TestDb::run(async |mut conn, url| {
		let server = app(database::init_to(&url), ActorSource::Operator);
		server
			.post("/ahead_then_failed")
			.await
			.assert_status(axum::http::StatusCode::BAD_REQUEST);

		assert!(
			!Admin::check_email(&mut conn, "refused@example.com")
				.await
				.unwrap()
		);
		let log = entries(&mut conn).await;
		assert_eq!(log.len(), 1);
		assert_eq!(log[0].status, Status::Failed);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_audited_read_is_recorded_before_it_reads_and_settled_by_its_outcome() {
	TestDb::run(async |mut conn, url| {
		let server = app(database::init_to(&url), ActorSource::Operator);
		server.post("/read").await.assert_status_ok();
		server
			.post("/read_errors")
			.await
			.assert_status(axum::http::StatusCode::BAD_REQUEST);

		let log = entries(&mut conn).await;
		assert_eq!(log.len(), 2, "the read that failed is recorded too");
		let by_name = |name: &str| {
			log.iter()
				.find(|entry| entry.records.iter().any(|record| record.name == name))
				.unwrap()
		};
		assert_eq!(by_name("read").status, Status::Done);
		assert_eq!(by_name("read_errors").status, Status::Failed);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_handler_names_its_own_actor() {
	TestDb::run(async |mut conn, url| {
		let server = app(database::init_to(&url), ActorSource::Handler);
		server.post("/device").await.assert_status_ok();

		let log = entries(&mut conn).await;
		assert_eq!(log.len(), 1);
		assert!(matches!(
			&log[0].actor,
			Actor::Device { role, .. } if *role == commons_types::device::DeviceRole::Machine
		));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_handler_that_never_names_an_actor_cannot_record() {
	TestDb::run(async |mut conn, url| {
		let server = app(database::init_to(&url), ActorSource::Handler);
		server
			.post("/records")
			.await
			.assert_status(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
		assert!(entries(&mut conn).await.is_empty());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_entry_that_cannot_be_written_refuses_the_action() {
	TestDb::run(async |mut conn, url| {
		// With no range to write into, no audited action can be carried out.
		diesel_async_run(
			&mut conn,
			"DO $$ DECLARE r record; BEGIN \
			   FOR r IN SELECT c.relname FROM pg_inherits i \
			     JOIN pg_class c ON c.oid = i.inhrelid \
			     JOIN pg_class p ON p.oid = i.inhparent WHERE p.relname = 'audit_entries' \
			   LOOP EXECUTE format('DROP TABLE %I', r.relname); END LOOP; END $$",
		)
		.await;

		let server = app(database::init_to(&url), ActorSource::Operator);
		server
			.post("/records")
			.await
			.assert_status(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
		assert!(
			!Admin::check_email(&mut conn, "kept@example.com")
				.await
				.unwrap(),
			"the change did not stand without its entry"
		);

		let runway = database::partitions::runway(&mut conn).await.unwrap();
		assert!(
			runway
				.iter()
				.any(|history| history.parent == "audit_entries" && history.critical()),
			"the runway reports the log as out of range"
		);
		assert!(
			database::self_alerts::sweep_partition_runway(&mut conn)
				.await
				.unwrap()
				.is_some(),
			"the runway self-alert is raised"
		);
	})
	.await
}
