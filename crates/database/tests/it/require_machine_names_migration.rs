//! The `2026-10-05-072408-0000_require_machine_names` migration makes a
//! machine's name mandatory, naming any machine that lacks one from what the
//! box is known by first.
//!
//! Replays it for real: reverts the migration, strips names in the shapes they
//! could have had before it, then re-applies it.

use diesel::sql_types;
use diesel_async::{RunQueryDsl, SimpleAsyncConnection as _};
use uuid::Uuid;

const UP: &str =
	include_str!("../../../../migrations/2026-10-05-072408-0000_require_machine_names/up.sql");
const DOWN: &str =
	include_str!("../../../../migrations/2026-10-05-072408-0000_require_machine_names/down.sql");

#[derive(diesel::QueryableByName)]
struct Name {
	#[diesel(sql_type = sql_types::Text)]
	name: String,
}

async fn name_of(conn: &mut diesel_async::AsyncPgConnection, id: Uuid) -> String {
	diesel::sql_query("SELECT name FROM machines WHERE id = $1")
		.bind::<sql_types::Uuid, _>(id)
		.get_result::<Name>(conn)
		.await
		.expect("machine")
		.name
}

// spec: FLT#naming
#[tokio::test(flavor = "multi_thread")]
async fn an_unnamed_machine_is_named_from_what_the_box_is_known_by() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let [
			named,
			by_hostname,
			by_tailnet,
			by_application,
			blank,
			placeholder,
		] = std::array::from_fn::<_, 6, _>(|_| Uuid::new_v4());
		let [device, other_device] = std::array::from_fn::<_, 2, _>(|_| Uuid::new_v4());

		conn.batch_execute(&format!(
			"INSERT INTO devices (id, role, tailscale_node_name) \
			 VALUES ('{device}', 'machine', 'node-7.tailnet.ts.net'), \
			 ('{other_device}', 'machine', 'node-8.tailnet.ts.net'); \
			 INSERT INTO machines (id, name) VALUES \
			 ('{named}', 'kept'), ('{by_hostname}', 'x'), ('{by_tailnet}', 'x'), \
			 ('{by_application}', 'x'), ('{blank}', 'x'), ('{placeholder}', 'x'); \
			 UPDATE machines SET device_id = '{device}' WHERE id = '{by_tailnet}'; \
			 UPDATE machines SET device_id = '{other_device}' WHERE id = '{by_hostname}'; \
			 INSERT INTO machine_reported_detail (machine_id, source, extra, reported_at) VALUES \
			 ('{by_hostname}', 'old', '{{\"hostname\": \"stale-host\"}}', NOW() - INTERVAL '1 day'), \
			 ('{by_hostname}', 'new', '{{\"hostname\": \"fresh-host\"}}', NOW()), \
			 ('{by_tailnet}', 'bestool', '{{\"hostname\": \"  \"}}', NOW()); \
			 INSERT INTO applications (type, name, machine_id, created_at) VALUES \
			 ('tamanu-central', NULL, '{by_application}', NOW() - INTERVAL '2 days'), \
			 ('tamanu-facility', 'central-ward', '{by_application}', NOW() - INTERVAL '1 day'), \
			 ('tamanu-facility', 'later', '{by_application}', NOW()), \
			 ('tamanu-central', NULL, '{placeholder}', NOW());"
		))
		.await
		.expect("seed");

		// Back to the shape before names were required, then take them away.
		conn.batch_execute(DOWN).await.expect("revert");
		conn.batch_execute(&format!(
			"UPDATE machines SET name = NULL \
			 WHERE id IN ('{by_hostname}', '{by_tailnet}', '{by_application}', '{placeholder}'); \
			 UPDATE machines SET name = '  ' WHERE id = '{blank}';"
		))
		.await
		.expect("strip names");

		conn.batch_execute(UP).await.expect("re-apply");

		assert_eq!(name_of(&mut conn, named).await, "kept");
		assert_eq!(name_of(&mut conn, by_hostname).await, "fresh-host");
		assert_eq!(
			name_of(&mut conn, by_tailnet).await,
			"node-7.tailnet.ts.net"
		);
		assert_eq!(name_of(&mut conn, by_application).await, "central-ward");
		assert_eq!(name_of(&mut conn, blank).await, "Unnamed machine");
		assert_eq!(name_of(&mut conn, placeholder).await, "Unnamed machine");

		let refused = conn
			.batch_execute(&format!(
				"UPDATE machines SET name = ' ' WHERE id = '{named}'"
			))
			.await;
		assert!(refused.is_err(), "a blank name is refused once required");
	})
	.await
}
