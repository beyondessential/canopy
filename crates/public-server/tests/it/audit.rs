//! What a status push records in the audit log (spec `AUD`): each application
//! it brings and each check it registers, and nothing otherwise.

use database::audit::{self, Actor, Entry, Filter};
use diesel::sql_query;
use diesel::sql_types;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use uuid::Uuid;

async fn machine_for(conn: &mut AsyncPgConnection, device_id: Uuid) -> Uuid {
	let group_id = Uuid::new_v4();
	sql_query("INSERT INTO server_groups (id, name) VALUES ($1, 'audited-group')")
		.bind::<sql_types::Uuid, _>(group_id)
		.execute(conn)
		.await
		.expect("insert group");
	let machine_id = Uuid::new_v4();
	sql_query("INSERT INTO machines (name, id, group_id, device_id) VALUES ('box', $1, $2, $3)")
		.bind::<sql_types::Uuid, _>(machine_id)
		.bind::<sql_types::Uuid, _>(group_id)
		.bind::<sql_types::Uuid, _>(device_id)
		.execute(conn)
		.await
		.expect("insert machine");
	machine_id
}

fn push() -> serde_json::Value {
	serde_json::json!({
		"source": "alertd",
		"machine": {
			"health": [{ "check": "disk_free", "result": "passed" }],
			"detail": {},
		},
		"applications": {
			"central": {
				"type": "tamanu-central",
				"health": [{ "check": "db", "result": "passed" }],
				"detail": {},
			},
			"facility": {
				"type": "tamanu-facility",
				"health": [{ "check": "db", "result": "passed" }],
				"detail": {},
			},
		},
	})
}

async fn entries(conn: &mut AsyncPgConnection, action: &str) -> Vec<Entry> {
	audit::find(
		conn,
		&Filter {
			action: Some(action.to_owned()),
			limit: 100,
			..Default::default()
		},
	)
	.await
	.expect("read the audit log")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_push_records_each_application_and_check_it_creates_and_nothing_after() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let machine_id = machine_for(&mut conn, device_id).await;

			let send = async || {
				public
					.post(&format!("/status/{machine_id}"))
					.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
					.json(&push())
					.await
					.assert_status_ok();
			};

			send().await;
			let arrivals = entries(&mut conn, "application_arrived").await;
			assert_eq!(arrivals.len(), 2, "one arrival for each new application");
			for arrival in &arrivals {
				assert!(matches!(
					&arrival.actor,
					Actor::Device { id, .. } if *id == device_id
				));
				assert!(arrival.records.iter().any(|record| record.name == "box"));
			}

			// The box's disk check, and the one name two application types each
			// report, which is two checks.
			let registrations = entries(&mut conn, "check_registered").await;
			assert_eq!(registrations.len(), 3, "{registrations:?}");

			send().await;
			assert_eq!(entries(&mut conn, "application_arrived").await.len(), 2);
			assert_eq!(
				entries(&mut conn, "check_registered").await.len(),
				3,
				"a push that creates nothing records nothing"
			);
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ignored_source_records_no_arrival_for_what_the_box_already_runs() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			use commons_types::source::IngestMode;
			use database::source_policies::SourcePolicy;

			let machine_id = machine_for(&mut conn, device_id).await;
			let send = async || {
				public
					.post(&format!("/status/{machine_id}"))
					.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
					.json(&push())
					.await
					.assert_status_ok();
			};

			send().await;
			assert_eq!(entries(&mut conn, "application_arrived").await.len(), 2);

			SourcePolicy::set_ingest(&mut conn, "alertd", IngestMode::Ignore)
				.await
				.expect("set ignore");
			send().await;
			send().await;
			assert_eq!(
				entries(&mut conn, "application_arrived").await.len(),
				2,
				"an ignored push resolves the applications already held without bringing them"
			);
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn yanking_is_recorded_once_and_yanking_again_or_an_unknown_version_changes_nothing() {
	commons_tests::server::run_with_device_auth(
		"admin",
		async |mut conn, cert, _device_id, public, _| {
			sql_query(
				"INSERT INTO versions (major, minor, patch, changelog, status) \
				 VALUES (1, 0, 1, 'yankable', 'published')",
			)
			.execute(&mut conn)
			.await
			.expect("insert version");

			for version in ["1.0.1", "1.0.1", "9.9.9"] {
				public
					.delete(&format!("/versions/{version}"))
					.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
					.await
					.assert_status_ok();
			}
			assert_eq!(entries(&mut conn, "version_removed").await.len(), 1);
		},
	)
	.await
}
