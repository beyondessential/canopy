//! The reserved `canopy:` tag namespace is owned by the synthetic tags the
//! public `/tags` endpoint injects, so operator-driven tag writes on applications
//! and server groups must reject keys under it.

use std::collections::BTreeMap;

use commons_errors::AppError;
use commons_types::server::{TagMap, app_type::ApplicationType};
use database::{
	applications::{Application, PartialServer},
	machines::{Machine, NewMachine},
	pg_duration::PgDuration,
	server_groups::{NewServerGroup, PartialServerGroup, ServerGroup},
	url_field::UrlField,
};
use jiff::SignedDuration;
use uuid::Uuid;

fn reserved_tags() -> TagMap {
	let mut map = BTreeMap::new();
	map.insert("canopy:rank".to_string(), "spoofed".to_string());
	TagMap(map)
}

fn new_server(host: &str, machine_id: Uuid) -> Application {
	Application {
		id: Uuid::new_v4(),
		name: Some("t".into()),
		host: Some(UrlField(host.parse().unwrap())),
		r#type: ApplicationType::TamanuCentral,
		rank: None,
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

fn assert_bad_request<T: std::fmt::Debug>(result: Result<T, AppError>) {
	match result {
		Err(AppError::BadRequest(_)) => {}
		other => panic!("expected BadRequest, got {other:?}"),
	}
}

#[tokio::test(flavor = "multi_thread")]
async fn server_create_rejects_reserved_tag_keys() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let machine = Machine::create(&mut conn, NewMachine::default())
			.await
			.unwrap();
		let mut s = new_server("https://create.example/", machine.id);
		s.tags = reserved_tags();
		assert_bad_request(Application::create(&mut conn, s).await);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn server_update_rejects_reserved_tag_keys() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let machine = Machine::create(&mut conn, NewMachine::default())
			.await
			.unwrap();
		let server =
			Application::create(&mut conn, new_server("https://update.example/", machine.id))
				.await
				.unwrap();
		let updates = PartialServer {
			id: server.id,
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
			tags: Some(reserved_tags()),
			may_manage_dns: None,
			may_manage_tls: None,
		};
		assert_bad_request(Application::update(&mut conn, server.id, updates).await);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn server_group_create_rejects_reserved_tag_keys() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let new = NewServerGroup {
			name: "reserved-group".into(),
			notes: String::new(),
			tags: reserved_tags(),
			slack_open_delay: None,
			slack_close_delay: None,
		};
		assert_bad_request(ServerGroup::create(&mut conn, new).await);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn server_group_update_rejects_reserved_tag_keys() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let group = ServerGroup::create(
			&mut conn,
			NewServerGroup {
				name: "updatable-group".into(),
				notes: String::new(),
				tags: TagMap::default(),
				slack_open_delay: None,
				slack_close_delay: None,
			},
		)
		.await
		.unwrap();
		let changes = PartialServerGroup {
			name: None,
			notes: None,
			tags: Some(reserved_tags()),
			slack_open_delay: None,
			slack_close_delay: None,
		};
		assert_bad_request(ServerGroup::update(&mut conn, group.id, changes).await);
	})
	.await
}
