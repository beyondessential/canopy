//! An incident targets one of a group's environments, meaning its members at
//! one rank, rather than the group as a whole, so a site's test box and its
//! production central are separate incidents. A group's own checks belong to its
//! headline environment, and a pending member, in no environment yet, belongs to
//! no incident.

use commons_types::{server::rank::ServerRank, status::CheckResult};
use database::issues::{NewEvent, Scope};
use database::slack_outbox::KIND_INCIDENT_OPEN;
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::RunQueryDsl;
use uuid::Uuid;

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

#[derive(QueryableByName)]
struct Count {
	#[diesel(sql_type = sql_types::BigInt)]
	n: i64,
}

#[derive(QueryableByName)]
struct Payload {
	#[diesel(sql_type = sql_types::Jsonb)]
	payload: serde_json::Value,
}

#[derive(QueryableByName)]
struct MaybeRank {
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	rank: Option<String>,
}

async fn insert_group(conn: &mut diesel_async::AsyncPgConnection) -> Uuid {
	let row: RowId = sql_query("INSERT INTO server_groups (name) VALUES ('site') RETURNING id")
		.get_result(conn)
		.await
		.expect("group");
	row.id
}

async fn insert_machine(conn: &mut diesel_async::AsyncPgConnection, group: Option<Uuid>) -> Uuid {
	let row: RowId =
		sql_query("INSERT INTO machines (name, group_id) VALUES ('box', $1) RETURNING id")
			.bind::<sql_types::Nullable<sql_types::Uuid>, _>(group)
			.get_result(conn)
			.await
			.expect("machine");
	row.id
}

async fn insert_application(
	conn: &mut diesel_async::AsyncPgConnection,
	machine: Uuid,
	group: Option<Uuid>,
	rank: Option<&str>,
	host: &str,
) -> Uuid {
	let row: RowId = sql_query(
		"INSERT INTO applications (type, host, group_id, rank, machine_id, is_monitored) \
		 VALUES ('tamanu-central', $1, $2, $3, $4, true) RETURNING id",
	)
	.bind::<sql_types::Text, _>(host)
	.bind::<sql_types::Nullable<sql_types::Uuid>, _>(group)
	.bind::<sql_types::Nullable<sql_types::Text>, _>(rank)
	.bind::<sql_types::Uuid, _>(machine)
	.get_result(conn)
	.await
	.expect("application");
	row.id
}

/// One ranked application on a machine of its own.
async fn insert_ranked_member(
	conn: &mut diesel_async::AsyncPgConnection,
	group: Uuid,
	rank: Option<&str>,
	host: &str,
) -> Uuid {
	let machine = insert_machine(conn, Some(group)).await;
	insert_application(conn, machine, Some(group), rank, host).await
}

fn failed_stamp(check: &str) -> database::issues::CheckStateStamp {
	database::issues::CheckStateStamp {
		check: check.into(),
		observed: CheckResult::Failed,
		effective: CheckResult::Failed,
		escalates: false,
		detail: None,
		title: None,
		instanced: None,
	}
}

async fn fail_application(
	conn: &mut diesel_async::AsyncPgConnection,
	application: Uuid,
	check: &str,
) {
	NewEvent {
		source: "alertd".into(),
		r#ref: check.into(),
		description: None,
		message: format!("{check} is failing"),
		active: Some(true),
		occurred_at: None,
	}
	.save_with_state(conn, application, None, Some(&failed_stamp(check)), false)
	.await
	.expect("file application check");
}

async fn open_ranks(
	conn: &mut diesel_async::AsyncPgConnection,
	group: Uuid,
) -> Vec<Option<String>> {
	let rows: Vec<MaybeRank> = sql_query(
		"SELECT rank FROM incidents WHERE server_group_id = $1 AND closed_at IS NULL ORDER BY rank",
	)
	.bind::<sql_types::Uuid, _>(group)
	.load(conn)
	.await
	.expect("open incidents");
	rows.into_iter().map(|r| r.rank).collect()
}

/// The headline case: a test box going down and a production central going
/// down are trouble in two environments, so they are two incidents on their
/// own channels of urgency rather than one page for the site.
// spec: INC#targets
#[tokio::test(flavor = "multi_thread")]
async fn trouble_at_two_ranks_opens_two_incidents() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = insert_group(&mut conn).await;
		let production =
			insert_ranked_member(&mut conn, group, Some("production"), "http://prod.invalid/")
				.await;
		let test =
			insert_ranked_member(&mut conn, group, Some("test"), "http://test.invalid/").await;

		fail_application(&mut conn, test, "app_down").await;
		fail_application(&mut conn, production, "app_down").await;

		assert_eq!(
			open_ranks(&mut conn, group).await,
			vec![Some("production".to_string()), Some("test".to_string())],
			"one incident per environment in trouble",
		);
	})
	.await
}

/// Production trouble arriving while a lesser environment's incident is open
/// opens its own rather than joining what is already there, which is what
/// keeps a test failure from swallowing the page for a real outage.
// spec: INC#targets
#[tokio::test(flavor = "multi_thread")]
async fn production_trouble_does_not_join_an_open_test_incident() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = insert_group(&mut conn).await;
		let production =
			insert_ranked_member(&mut conn, group, Some("production"), "http://prod.invalid/")
				.await;
		let test =
			insert_ranked_member(&mut conn, group, Some("test"), "http://test.invalid/").await;

		fail_application(&mut conn, test, "app_down").await;
		fail_application(&mut conn, production, "app_down").await;

		let shared: Count = sql_query(
			"SELECT COUNT(*) AS n FROM incident_issues ii \
			 JOIN incidents inc ON inc.id = ii.incident_id \
			 WHERE inc.server_group_id = $1 AND inc.rank = 'production' AND ii.left_at IS NULL",
		)
		.bind::<sql_types::Uuid, _>(group)
		.get_result(&mut conn)
		.await
		.expect("count links");
		assert_eq!(
			shared.n, 1,
			"production's incident carries production's issue alone"
		);
	})
	.await
}

async fn fail_group_check(conn: &mut diesel_async::AsyncPgConnection, group: Uuid) {
	database::issues::raise_group_event_with_state(
		conn,
		group,
		"backup_stale",
		None,
		"the repository is stale",
		true,
		Some(&failed_stamp("backup_stale")),
	)
	.await
	.expect("file group check");
}

async fn live_members(conn: &mut diesel_async::AsyncPgConnection, group: Uuid) -> i64 {
	let live: Count = sql_query(
		"SELECT COUNT(*) AS n FROM incident_issues ii \
		 JOIN incidents inc ON inc.id = ii.incident_id \
		 WHERE inc.server_group_id = $1 AND ii.left_at IS NULL",
	)
	.bind::<sql_types::Uuid, _>(group)
	.get_result(conn)
	.await
	.expect("count links");
	live.n
}

/// The shape a database beside a site's production central had: the box going
/// dark takes its machine's checks, the central, and the database with it, and
/// that is one incident, not a production incident and a second one for the
/// database alone.
// spec: INC#targets
#[tokio::test(flavor = "multi_thread")]
async fn a_box_going_down_is_one_incident_for_everything_on_it() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = insert_group(&mut conn).await;
		let machine = insert_machine(&mut conn, Some(group)).await;
		let central = insert_application(
			&mut conn,
			machine,
			Some(group),
			Some("production"),
			"http://central.invalid/",
		)
		.await;
		let postgres = insert_application(
			&mut conn,
			machine,
			Some(group),
			Some("production"),
			"http://postgres.invalid/",
		)
		.await;

		database::issues::raise_machine_event_with_state(
			&mut conn,
			machine,
			"alertd",
			None,
			"unreachable",
			None,
			"the box stopped reporting",
			true,
			Some(&failed_stamp("unreachable")),
		)
		.await
		.expect("file machine check");
		fail_application(&mut conn, central, "unreachable").await;
		fail_application(&mut conn, postgres, "unreachable").await;

		assert_eq!(
			open_ranks(&mut conn, group).await,
			vec![Some("production".to_string())],
			"one incident, on the environment the box serves",
		);
		assert_eq!(live_members(&mut conn, group).await, 3);
	})
	.await
}

/// A group check asserts something held once for the group however many
/// environments it has, so it belongs to the group's headline environment
/// rather than to an incident of its own.
// spec: INC#targets
#[tokio::test(flavor = "multi_thread")]
async fn a_groups_own_check_joins_its_headline_environments_incident() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = insert_group(&mut conn).await;
		let production =
			insert_ranked_member(&mut conn, group, Some("production"), "http://prod.invalid/")
				.await;
		insert_ranked_member(&mut conn, group, Some("test"), "http://test.invalid/").await;

		fail_group_check(&mut conn, group).await;
		assert_eq!(
			open_ranks(&mut conn, group).await,
			vec![Some("production".to_string())],
			"a failing backup check opens the production incident",
		);

		fail_application(&mut conn, production, "app_down").await;
		assert_eq!(
			open_ranks(&mut conn, group).await,
			vec![Some("production".to_string())],
			"and a production failure joins it rather than opening another",
		);
		assert_eq!(live_members(&mut conn, group).await, 2);
	})
	.await
}

/// A group whose highest rank is demo has demo for its headline, so its own
/// checks join the demo environment's incident.
// spec: INC#targets
#[tokio::test(flavor = "multi_thread")]
async fn a_groups_own_check_follows_its_headline_rank() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = insert_group(&mut conn).await;
		insert_ranked_member(&mut conn, group, Some("demo"), "http://demo.invalid/").await;
		insert_ranked_member(&mut conn, group, Some("test"), "http://test.invalid/").await;

		fail_group_check(&mut conn, group).await;
		assert_eq!(
			open_ranks(&mut conn, group).await,
			vec![Some("demo".to_string())],
		);
	})
	.await
}

/// Nothing ranked means no environment, so no application is in one and a
/// group has no headline environment for its own checks to belong to: none of
/// their trouble belongs to a target, and none opens an incident.
// spec: INC#targets
#[tokio::test(flavor = "multi_thread")]
async fn pending_members_and_a_group_with_nothing_ranked_have_no_target() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = insert_group(&mut conn).await;
		let machine = insert_machine(&mut conn, Some(group)).await;
		let pending =
			insert_application(&mut conn, machine, Some(group), None, "http://new.invalid/").await;

		for scope in [
			Scope::Application(pending),
			Scope::Machine(machine),
			Scope::Group(group),
		] {
			assert_eq!(
				scope
					.resolve_incident_target(&mut conn)
					.await
					.expect("resolve"),
				None,
				"{scope:?} belongs to no target while nothing in the group is ranked",
			);
		}

		fail_application(&mut conn, pending, "app_down").await;
		database::issues::raise_machine_event_with_state(
			&mut conn,
			machine,
			"alertd",
			None,
			"disk_free",
			None,
			"disk 2% free",
			true,
			Some(&failed_stamp("disk_free")),
		)
		.await
		.expect("file machine check");
		fail_group_check(&mut conn, group).await;

		assert!(
			open_ranks(&mut conn, group).await.is_empty(),
			"a pending member's trouble opens no incident",
		);
	})
	.await
}

/// A pending box beside a ranked one is in no environment, so its trouble does
/// not reach the incident of the box next to it.
// spec: INC#targets
#[tokio::test(flavor = "multi_thread")]
async fn a_pending_box_beside_a_ranked_one_opens_nothing() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = insert_group(&mut conn).await;
		insert_ranked_member(&mut conn, group, Some("production"), "http://prod.invalid/").await;
		let pending = insert_ranked_member(&mut conn, group, None, "http://pending.invalid/").await;

		fail_application(&mut conn, pending, "app_down").await;
		assert!(open_ranks(&mut conn, group).await.is_empty());
	})
	.await
}

/// Ranking a pending box brings the trouble already on it into incident
/// membership, on the environment it was ranked into.
// spec: INC#membership
#[tokio::test(flavor = "multi_thread")]
async fn ranking_a_pending_box_opens_its_incident() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = insert_group(&mut conn).await;
		let machine = insert_machine(&mut conn, Some(group)).await;
		let pending =
			insert_application(&mut conn, machine, Some(group), None, "http://new.invalid/").await;
		fail_application(&mut conn, pending, "app_down").await;
		assert!(open_ranks(&mut conn, group).await.is_empty());

		database::machines::Machine::set_rank(&mut conn, machine, ServerRank::Test, Some("op"))
			.await
			.expect("rank the box");

		assert_eq!(
			open_ranks(&mut conn, group).await,
			vec![Some("test".to_string())],
		);
	})
	.await
}

/// Moving a failing box between environments moves the issue with it: the
/// incident it leaves closes with nothing holding it open.
// spec: INC#membership
#[tokio::test(flavor = "multi_thread")]
async fn ranking_a_box_up_moves_its_issues_into_the_new_environments_incident() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = insert_group(&mut conn).await;
		let machine = insert_machine(&mut conn, Some(group)).await;
		let central = insert_application(
			&mut conn,
			machine,
			Some(group),
			Some("test"),
			"http://central.invalid/",
		)
		.await;
		fail_application(&mut conn, central, "app_down").await;
		assert_eq!(
			open_ranks(&mut conn, group).await,
			vec![Some("test".to_string())],
		);

		database::machines::Machine::set_rank(
			&mut conn,
			machine,
			ServerRank::Production,
			Some("op"),
		)
		.await
		.expect("rank the box");

		assert_eq!(
			open_ranks(&mut conn, group).await,
			vec![Some("production".to_string())],
			"the test incident closes and production's opens",
		);
	})
	.await
}

/// The group's own checks follow its headline environment, so when the group's
/// only production box is ranked down the open backup issue moves to the
/// environment that is now the headline.
// spec: INC#membership
#[tokio::test(flavor = "multi_thread")]
async fn a_headline_change_moves_the_groups_own_issues() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = insert_group(&mut conn).await;
		let machine = insert_machine(&mut conn, Some(group)).await;
		insert_application(
			&mut conn,
			machine,
			Some(group),
			Some("production"),
			"http://prod.invalid/",
		)
		.await;
		insert_ranked_member(&mut conn, group, Some("test"), "http://test.invalid/").await;
		fail_group_check(&mut conn, group).await;
		assert_eq!(
			open_ranks(&mut conn, group).await,
			vec![Some("production".to_string())],
		);

		database::machines::Machine::set_rank(&mut conn, machine, ServerRank::Clone, Some("op"))
			.await
			.expect("rank the box down");

		assert_eq!(
			open_ranks(&mut conn, group).await,
			vec![Some("clone".to_string())],
			"the backup issue follows the new headline",
		);
	})
	.await
}

/// The group's own checks follow its headline environment however it moves:
/// archiving the box that carried it, or moving that box to another group,
/// leaves the open backup issue in the environment that is the headline now.
// spec: INC#membership
#[tokio::test(flavor = "multi_thread")]
async fn archiving_or_moving_the_headline_box_moves_the_groups_own_issues() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		for move_it in [false, true] {
			let group = insert_group(&mut conn).await;
			let machine = insert_machine(&mut conn, Some(group)).await;
			insert_application(
				&mut conn,
				machine,
				Some(group),
				Some("production"),
				&format!("http://prod-{move_it}.invalid/"),
			)
			.await;
			insert_ranked_member(
				&mut conn,
				group,
				Some("test"),
				&format!("http://test-{move_it}.invalid/"),
			)
			.await;
			fail_group_check(&mut conn, group).await;
			assert_eq!(
				open_ranks(&mut conn, group).await,
				vec![Some("production".to_string())],
			);

			if move_it {
				let elsewhere = insert_group(&mut conn).await;
				database::machines::Machine::update(
					&mut conn,
					machine,
					database::machines::MachineUpdate {
						group_id: Some(Some(elsewhere)),
						..Default::default()
					},
				)
				.await
				.expect("move the box");
			} else {
				database::machines::Machine::archive(&mut conn, machine)
					.await
					.expect("archive the box");
			}

			assert_eq!(
				open_ranks(&mut conn, group).await,
				vec![Some("test".to_string())],
				"the backup issue follows the headline (moved: {move_it})",
			);
		}
	})
	.await
}

/// Setting a rank moves the issue to the environment it now belongs to: it
/// leaves the incident on the target it has left, which closes with nothing
/// holding it open, and joins one on its new environment.
// spec: INC#membership
#[tokio::test(flavor = "multi_thread")]
async fn setting_a_rank_moves_the_issue_to_its_new_environment() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = insert_group(&mut conn).await;
		let member = insert_ranked_member(
			&mut conn,
			group,
			Some("production"),
			"http://moves.invalid/",
		)
		.await;

		fail_application(&mut conn, member, "app_down").await;
		assert_eq!(
			open_ranks(&mut conn, group).await,
			vec![Some("production".to_string())],
		);

		sql_query("UPDATE applications SET rank = 'test' WHERE id = $1")
			.bind::<sql_types::Uuid, _>(member)
			.execute(&mut conn)
			.await
			.expect("set rank");
		database::issues::reevaluate_open_issues_for_server(&mut conn, member)
			.await
			.expect("re-evaluate");

		assert_eq!(
			open_ranks(&mut conn, group).await,
			vec![Some("test".to_string())],
			"the issue's incident follows it, and the one it left closes",
		);
		let live: Count = sql_query(
			"SELECT COUNT(*) AS n FROM incident_issues ii \
			 JOIN incidents inc ON inc.id = ii.incident_id \
			 WHERE inc.rank = 'test' AND ii.left_at IS NULL",
		)
		.get_result(&mut conn)
		.await
		.expect("count links");
		assert_eq!(live.n, 1, "the issue is a live member of its new incident");
	})
	.await
}

/// A notification names the environment it is about, so an operator reading
/// the channel tells the site's production trouble from its test trouble. A
/// production environment reads as the group alone, and every other reads as
/// the group with its rank after it.
// spec: INC#notification
#[tokio::test(flavor = "multi_thread")]
async fn a_notice_names_the_environment_it_is_about() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = insert_group(&mut conn).await;
		let production =
			insert_ranked_member(&mut conn, group, Some("production"), "http://prod.invalid/")
				.await;
		let clone =
			insert_ranked_member(&mut conn, group, Some("clone"), "http://clone.invalid/").await;

		fail_application(&mut conn, production, "app_down").await;
		fail_application(&mut conn, clone, "app_down").await;

		let rows: Vec<Payload> = sql_query("SELECT payload FROM slack_outbox WHERE kind = $1")
			.bind::<sql_types::Text, _>(KIND_INCIDENT_OPEN)
			.load(&mut conn)
			.await
			.expect("outbox rows");
		let mut labels: Vec<String> = rows
			.iter()
			.map(|r| r.payload["server"].as_str().unwrap_or_default().to_string())
			.collect();
		labels.sort();
		assert_eq!(
			labels,
			vec!["site".to_string(), "site clone".to_string()],
			"production reads as the site, and clone reads as the site's clone",
		);
		// The application an issue is on is named on its line in the summary,
		// not in the target.
		let messages: Vec<&str> = rows
			.iter()
			.map(|r| r.payload["message"].as_str().unwrap_or_default())
			.collect();
		assert!(
			messages.iter().any(|m| m.contains("on Tamanu central")),
			"got {messages:?}"
		);
	})
	.await
}

#[derive(QueryableByName)]
struct Delay {
	#[diesel(sql_type = sql_types::Double)]
	delay_secs: f64,
}

/// The close notice names the environment too. Only the open was covered, and a
/// resolve naming the bare site would tell the channel a site's production
/// recovered when it was the test box.
// spec: INC#notification
#[tokio::test(flavor = "multi_thread")]
async fn a_resolve_names_the_environment_it_is_about() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = insert_group(&mut conn).await;
		let test =
			insert_ranked_member(&mut conn, group, Some("test"), "http://test.invalid/").await;

		fail_application(&mut conn, test, "app_down").await;
		// A resolve is withheld unless an open was actually delivered, since a
		// flap the channel never saw needs no all-clear.
		sql_query("UPDATE slack_outbox SET delivered_at = NOW() WHERE kind = $1")
			.bind::<sql_types::Text, _>(KIND_INCIDENT_OPEN)
			.execute(&mut conn)
			.await
			.expect("mark the open delivered");

		let incident: RowId =
			sql_query("SELECT id FROM incidents WHERE server_group_id = $1 AND closed_at IS NULL")
				.bind::<sql_types::Uuid, _>(group)
				.get_result(&mut conn)
				.await
				.expect("the open incident");
		database::issues::Incident::resolve(
			&mut conn,
			incident.id,
			"op",
			commons_types::issue::ResolvedReason::Fixed,
		)
		.await
		.expect("resolve");

		let rows: Vec<Payload> =
			sql_query("SELECT payload FROM slack_outbox WHERE kind = 'incident_resolve'")
				.load(&mut conn)
				.await
				.expect("outbox rows");
		assert_eq!(rows.len(), 1, "the close notifies once");
		assert_eq!(
			rows[0].payload["server"].as_str(),
			Some("site test"),
			"the all-clear is for the test environment, not for the site",
		);
	})
	.await
}

/// Canopy attaches no configuration to an environment, so its grace period is
/// its group's: an operator who widened a site's cooldown widened it for the
/// site's test box too, and a test incident does not page ahead of the group's
/// own delay.
// spec: INC#targets
#[tokio::test(flavor = "multi_thread")]
async fn an_environment_incident_waits_out_its_groups_grace() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = insert_group(&mut conn).await;
		sql_query(
			"UPDATE server_groups SET slack_open_delay = INTERVAL '45 minutes' WHERE id = $1",
		)
		.bind::<sql_types::Uuid, _>(group)
		.execute(&mut conn)
		.await
		.expect("widen the group's grace");
		let test =
			insert_ranked_member(&mut conn, group, Some("test"), "http://test.invalid/").await;

		fail_application(&mut conn, test, "app_down").await;

		let row: Delay = sql_query(
			"SELECT EXTRACT(EPOCH FROM (o.deliver_after - NOW()))::float8 AS delay_secs \
			 FROM slack_outbox o JOIN incidents i ON i.id = o.incident_id \
			 WHERE o.kind = $1 AND i.rank = 'test'",
		)
		.bind::<sql_types::Text, _>(KIND_INCIDENT_OPEN)
		.get_result(&mut conn)
		.await
		.expect("the queued open");
		assert!(
			row.delay_secs > 40.0 * 60.0,
			"the environment's notice sits in the group's 45 minute window, not the default; got {}s",
			row.delay_secs,
		);
	})
	.await
}
