use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::RunQueryDsl;
use uuid::Uuid;

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

#[derive(QueryableByName)]
struct RowDeviceId {
	#[diesel(sql_type = sql_types::Nullable<sql_types::Uuid>)]
	device_id: Option<Uuid>,
}

#[tokio::test(flavor = "multi_thread")]
async fn deleting_device_nulls_status_device_id() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		// Prepare unique values
		let host_uuid = Uuid::new_v4();
		let host = format!("http://test.invalid/{}", host_uuid);

		// 1) Insert a device and keep its id
		let device_row: RowId = sql_query(
			r#"
				INSERT INTO devices (role)
				VALUES ('machine')
				RETURNING id
			"#,
		)
		.get_result(&mut conn)
		.await
		.expect("insert device");
		let device_id = device_row.id;

		// 2) Insert a server (minimal fields)
		let server_row: RowId = sql_query(
			r#"
				WITH m AS (INSERT INTO machines (name) VALUES ('box') RETURNING id) INSERT INTO applications (type, host, machine_id) SELECT 'tamanu-central', $1, m.id FROM m
				RETURNING id
			"#,
		)
		.bind::<sql_types::Text, _>(host)
		.get_result(&mut conn)
		.await
		.expect("insert server");
		let server_id = server_row.id;

		// 3) Insert a status referencing that device and server
		let status_row: RowId = sql_query(
			r#"
				INSERT INTO statuses (server_id, device_id, version, extra)
				VALUES ($1, $2, '1.0.0', '{}'::jsonb)
				RETURNING id
			"#,
		)
		.bind::<sql_types::Uuid, _>(server_id)
		.bind::<sql_types::Nullable<sql_types::Uuid>, _>(Some(device_id))
		.get_result(&mut conn)
		.await
		.expect("insert status");
		let status_id = status_row.id;

		// 4) Delete the device
		sql_query("DELETE FROM devices WHERE id = $1")
			.bind::<sql_types::Uuid, _>(device_id)
			.execute(&mut conn)
			.await
			.expect("delete device");

		// 5) Verify the status still exists but device_id is now NULL
		let device_fk: RowDeviceId = sql_query("SELECT device_id FROM statuses WHERE id = $1")
			.bind::<sql_types::Uuid, _>(status_id)
			.get_result(&mut conn)
			.await
			.expect("fetch status device_id");

		assert!(
			device_fk.device_id.is_none(),
			"Expected statuses.device_id to be NULL after deleting the device"
		);
	})
	.await
}
