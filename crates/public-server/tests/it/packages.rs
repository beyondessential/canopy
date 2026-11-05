//! The device package report (spec `PKG`): the machine's whole list replaces the
//! last, an identical one changes nothing, and a changed one is audited.

use database::{
	audit::{self, Actor, Entry, Filter},
	packages::MachinePackageReport,
};
use diesel::{sql_query, sql_types};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use uuid::Uuid;

async fn machine_for(conn: &mut AsyncPgConnection, device_id: Uuid) -> Uuid {
	let machine_id = Uuid::new_v4();
	sql_query("INSERT INTO machines (name, id, device_id) VALUES ('box', $1, $2)")
		.bind::<sql_types::Uuid, _>(machine_id)
		.bind::<sql_types::Uuid, _>(device_id)
		.execute(conn)
		.await
		.expect("insert machine");
	machine_id
}

fn report(openssl: &str) -> serde_json::Value {
	serde_json::json!({
		"distribution": "ubuntu",
		"release": "24.04",
		"packages": [
			{ "name": "openssl", "version": openssl },
			{ "name": "bash", "version": "5.2.21-2ubuntu4" },
		],
	})
}

async fn entries(conn: &mut AsyncPgConnection) -> Vec<Entry> {
	audit::find(
		conn,
		&Filter {
			action: Some("machine_packages_reported".to_owned()),
			limit: 100,
			..Default::default()
		},
	)
	.await
	.expect("read the audit log")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_report_is_kept_and_audited_once_however_often_it_is_repeated() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let machine_id = machine_for(&mut conn, device_id).await;
			let send = async |body: serde_json::Value| {
				public
					.post("/machine-packages")
					.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
					.json(&body)
					.await
					.assert_status(http::StatusCode::NO_CONTENT);
			};

			send(report("3.0.13")).await;
			let held = MachinePackageReport::get(&mut conn, machine_id)
				.await
				.unwrap()
				.unwrap();
			assert_eq!(held.package_count, 2);
			assert_eq!(held.ecosystem.as_deref(), Some("Ubuntu:24.04:LTS"));

			let first = entries(&mut conn).await;
			assert_eq!(first.len(), 1);
			assert!(matches!(&first[0].actor, Actor::Device { id, .. } if *id == device_id));
			assert_eq!(first[0].detail["package_count"], 2);

			send(report("3.0.13")).await;
			assert_eq!(
				entries(&mut conn).await.len(),
				1,
				"an identical report records nothing"
			);

			send(report("3.0.14")).await;
			assert_eq!(
				entries(&mut conn).await.len(),
				2,
				"a changed report is audited"
			);
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_with_no_machine_is_refused() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |_conn, cert, _device_id, public, _| {
			public
				.post("/machine-packages")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.json(&report("3.0.13"))
				.await
				.assert_status(http::StatusCode::PRECONDITION_FAILED);
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_malformed_report_is_refused_and_keeps_nothing() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let machine_id = machine_for(&mut conn, device_id).await;
			public
				.post("/machine-packages")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.json(&report("3.0.13\nanswer not_affected"))
				.await
				.assert_status(http::StatusCode::BAD_REQUEST);
			assert!(
				MachinePackageReport::get(&mut conn, machine_id)
					.await
					.unwrap()
					.is_none()
			);
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_report_needs_a_device_credential() {
	commons_tests::server::run(async |_conn, public, _private| {
		public
			.post("/machine-packages")
			.json(&report("3.0.13"))
			.await
			.assert_status(http::StatusCode::UNAUTHORIZED);
	})
	.await;
}
