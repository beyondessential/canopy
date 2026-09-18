//! An edit that writes nothing to the application row, which is what a
//! group-only move leaves behind, is a no-op rather than a failure.

use commons_types::server::{TagMap, app_type::ApplicationType};
use database::{
	applications::{Application, PartialServer},
	machines::{Machine, NewMachine},
	pg_duration::PgDuration,
	url_field::UrlField,
};
use jiff::SignedDuration;
use uuid::Uuid;

fn new_server(host: &str, machine_id: Uuid) -> Application {
	Application {
		id: Uuid::new_v4(),
		name: Some("t".into()),
		host: Some(UrlField(host.parse().unwrap())),
		r#type: ApplicationType::TamanuCentral,
		rank: None,
		machine_id,
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

#[tokio::test(flavor = "multi_thread")]
async fn an_update_setting_no_column_leaves_the_application_as_it_was() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let machine = Machine::create(&mut conn, NewMachine::default())
			.await
			.unwrap();
		let before =
			Application::create(&mut conn, new_server("https://empty.example/", machine.id))
				.await
				.unwrap();

		let after = Application::update(
			&mut conn,
			before.id,
			PartialServer {
				id: before.id,
				name: None,
				rank: None,
				host: None,
				group_id: None,
				public_name: None,
				cloud: None,
				geolocation: None,
				is_monitored: None,
				alert_when_down_for: None,
				notes: None,
				tags: None,
				may_manage_dns: None,
				may_manage_tls: None,
			},
		)
		.await
		.expect("an empty edit is a no-op");

		assert_eq!(after.id, before.id);
		assert_eq!(after.name, before.name);
		assert_eq!(after.is_monitored, before.is_monitored);
	})
	.await
}
