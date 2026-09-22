//! Model-level tests for archival and the enrolment-token lifecycle. A ticket
//! admits a box, so it is minted against the machine, and the identity it
//! binds is the box's: archiving the box releases it, archiving one workload
//! on the box does not.

use commons_types::server::{TagMap, app_type::ApplicationType};
use database::{
	Device, DeviceKey,
	applications::Application,
	machine_enrollment_tokens::MachineEnrollmentToken,
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

#[tokio::test(flavor = "multi_thread")]
async fn archiving_a_machine_releases_its_identity_and_takes_its_applications() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		// A box bound to a device with an active key, with a workload on it.
		let device = Device::create(&mut conn, b"key-bytes-1".to_vec())
			.await
			.unwrap();
		Device::trust(
			&mut conn,
			device.id,
			commons_types::device::DeviceRole::Machine,
		)
		.await
		.unwrap();
		let machine = Machine::create(&mut conn, NewMachine::default())
			.await
			.unwrap();
		Machine::bind_device(&mut conn, machine.id, device.id)
			.await
			.unwrap();
		let server = Application::create(
			&mut conn,
			new_server("https://archive-me.example/", machine.id),
		)
		.await
		.unwrap();

		Machine::archive(&mut conn, machine.id).await.unwrap();

		// The box is archived and its identity released, so coming back means
		// enrolling again.
		let after = Machine::get_by_id(&mut conn, machine.id).await.unwrap();
		assert!(after.deleted_at.is_some());
		assert!(after.device_id.is_none(), "device released");
		assert!(after.registered_at.is_none(), "enrolment cleared");

		// The workload on it went with the box.
		let app = Application::get_by_id(&mut conn, server.id).await.unwrap();
		assert!(
			app.deleted_at.is_some(),
			"application archived with its box"
		);

		// Device demoted + key deactivated.
		let keys = DeviceKey::find_by_device(&mut conn, device.id)
			.await
			.unwrap();
		assert!(keys.is_empty(), "active keys gone (deactivated)");
		// from_key (active-only) no longer resolves the device.
		assert!(
			Device::from_key(&mut conn, b"key-bytes-1")
				.await
				.unwrap()
				.is_none()
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn archiving_an_application_hides_the_row_and_leaves_the_identity_alone() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let device = Device::create(&mut conn, b"key-bytes-2".to_vec())
			.await
			.unwrap();
		Device::trust(
			&mut conn,
			device.id,
			commons_types::device::DeviceRole::Machine,
		)
		.await
		.unwrap();
		let machine = Machine::create(&mut conn, NewMachine::default())
			.await
			.unwrap();
		Machine::bind_device(&mut conn, machine.id, device.id)
			.await
			.unwrap();
		let server = Application::create(
			&mut conn,
			new_server("https://archive-me.example/", machine.id),
		)
		.await
		.unwrap();

		Application::soft_delete(&mut conn, server.id)
			.await
			.unwrap();

		// Hidden from live listings, still resolvable by id.
		assert!(
			Application::get_all(&mut conn, 0, None)
				.await
				.unwrap()
				.iter()
				.all(|x| x.id != server.id),
			"archived server hidden from get_all"
		);
		let after = Application::get_by_id(&mut conn, server.id).await.unwrap();
		assert!(after.deleted_at.is_some());

		// Retiring one workload says nothing about the box it ran on: the
		// identity stays bound and its key stays live, because the box may
		// still be carrying other workloads.
		let box_after = Machine::get_by_id(&mut conn, machine.id).await.unwrap();
		assert_eq!(box_after.device_id, Some(device.id), "identity untouched");
		assert!(
			Device::from_key(&mut conn, b"key-bytes-2")
				.await
				.unwrap()
				.is_some(),
			"key still active"
		);

		// Recreating a server at the same host is allowed once archived.
		let replacement = Machine::create(&mut conn, NewMachine::default())
			.await
			.unwrap();
		Application::create(
			&mut conn,
			new_server("https://archive-me.example/", replacement.id),
		)
		.await
		.expect("host freed for reuse after archival");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn token_reissue_invalidates_prior_and_consume_is_single_use() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let machine = Machine::create(&mut conn, NewMachine::default())
			.await
			.unwrap();
		// No application on it: a ticket admits the box, and what runs on the
		// box only exists once the enrolled agent reports it.

		let (_t1, first) =
			MachineEnrollmentToken::mint(&mut conn, machine.id, SignedDuration::from_hours(1))
				.await
				.unwrap();
		// Reissue: the first token must no longer be active.
		let (t2, second) =
			MachineEnrollmentToken::mint(&mut conn, machine.id, SignedDuration::from_hours(1))
				.await
				.unwrap();
		assert!(
			MachineEnrollmentToken::find_active(&mut conn, machine.id, &first)
				.await
				.is_err(),
			"reissue invalidated the prior token"
		);
		assert!(
			MachineEnrollmentToken::find_active(&mut conn, machine.id, &second)
				.await
				.is_ok(),
			"reissued token is active"
		);

		// Consume is single-use.
		MachineEnrollmentToken::consume(&mut conn, machine.id, &t2.token_hash)
			.await
			.unwrap();
		assert!(
			MachineEnrollmentToken::consume(&mut conn, machine.id, &t2.token_hash)
				.await
				.is_err(),
			"second consume fails"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn revoke_invalidates_the_active_token() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let machine = Machine::create(&mut conn, NewMachine::default())
			.await
			.unwrap();
		let (_t, token) =
			MachineEnrollmentToken::mint(&mut conn, machine.id, SignedDuration::from_hours(1))
				.await
				.unwrap();
		assert!(
			MachineEnrollmentToken::active_for(&mut conn, machine.id)
				.await
				.unwrap()
				.is_some()
		);

		MachineEnrollmentToken::revoke(&mut conn, machine.id)
			.await
			.unwrap();

		assert!(
			MachineEnrollmentToken::active_for(&mut conn, machine.id)
				.await
				.unwrap()
				.is_none(),
			"no active token after revoke"
		);
		assert!(
			MachineEnrollmentToken::find_active(&mut conn, machine.id, &token)
				.await
				.is_err(),
			"revoked token can't be presented"
		);
	})
	.await;
}
