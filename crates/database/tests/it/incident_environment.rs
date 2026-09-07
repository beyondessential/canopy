//! An incident targets one of a group's environments, meaning its
//! applications at one rank, rather than the group as a whole, so a site's
//! test box and its production central are separate incidents. A group's own
//! checks, and the members of a group with nothing ranked, target the group
//! itself.

use commons_types::{server::rank::ServerRank, status::CheckResult};
use database::issues::{IncidentTarget, NewEvent, Scope};
use database::slack_outbox::KIND_INCIDENT_OPEN;
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::RunQueryDsl;
use jiff::{SignedDuration, Timestamp};
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
	let row: RowId = sql_query("INSERT INTO machines (group_id) VALUES ($1) RETURNING id")
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

/// A group check asserts something held once for the group however many
/// environments it has, so it targets the group rather than any one of them.
// spec: INC#targets
#[tokio::test(flavor = "multi_thread")]
async fn a_groups_own_check_targets_the_group_beside_its_environments() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = insert_group(&mut conn).await;
		let production =
			insert_ranked_member(&mut conn, group, Some("production"), "http://prod.invalid/")
				.await;

		database::issues::raise_group_event_with_state(
			&mut conn,
			group,
			"backup_stale",
			None,
			"the repository is stale",
			true,
			Some(&failed_stamp("backup_stale")),
		)
		.await
		.expect("file group check");
		fail_application(&mut conn, production, "app_down").await;

		assert_eq!(
			open_ranks(&mut conn, group).await,
			vec![Some("production".to_string()), None],
			"the group's own trouble is its own incident, beside production's",
		);
	})
	.await
}

/// No rank means no environment, so an unranked application targets the group
/// whether or not the group has environments alongside it. Its trouble still
/// reaches an incident rather than being quietly filed nowhere.
// spec: INC#targets
#[tokio::test(flavor = "multi_thread")]
async fn an_unranked_application_targets_the_group() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let ranked_group = insert_group(&mut conn).await;
		insert_ranked_member(
			&mut conn,
			ranked_group,
			Some("production"),
			"http://prod.invalid/",
		)
		.await;
		let alongside =
			insert_ranked_member(&mut conn, ranked_group, None, "http://plain.invalid/").await;
		assert_eq!(
			Scope::Application(alongside)
				.resolve_incident_target(&mut conn)
				.await
				.expect("resolve"),
			Some((IncidentTarget::Group(ranked_group), true)),
			"a group's production environment is not where its unranked members land",
		);

		let bare_group = insert_group(&mut conn).await;
		let member =
			insert_ranked_member(&mut conn, bare_group, None, "http://bare.invalid/").await;
		assert_eq!(
			Scope::Application(member)
				.resolve_incident_target(&mut conn)
				.await
				.expect("resolve"),
			Some((IncidentTarget::Group(bare_group), true)),
		);

		fail_application(&mut conn, member, "app_down").await;
		assert_eq!(
			open_ranks(&mut conn, bare_group).await,
			vec![None],
			"an unranked member's trouble still opens an incident",
		);
	})
	.await
}

/// A box hosting nothing ranked is in no environment either, so its checks
/// answer to the group.
// spec: INC#targets
#[tokio::test(flavor = "multi_thread")]
async fn a_box_hosting_nothing_ranked_targets_the_group() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = insert_group(&mut conn).await;
		insert_ranked_member(&mut conn, group, Some("production"), "http://prod.invalid/").await;
		let machine = insert_machine(&mut conn, Some(group)).await;
		insert_application(
			&mut conn,
			machine,
			Some(group),
			None,
			"http://plain.invalid/",
		)
		.await;

		assert_eq!(
			Scope::Machine(machine)
				.resolve_incident_target(&mut conn)
				.await
				.expect("resolve"),
			Some((IncidentTarget::Group(group), true)),
		);
	})
	.await
}

/// The group is where an unranked member's incident lives, so the group's own
/// window is what quiets it. An environment window names a rank the member
/// does not have, so it cannot reach it and cannot go quiet on a box nobody is
/// working on.
// spec: MNT#what-a-window-suspends
#[tokio::test(flavor = "multi_thread")]
async fn a_group_window_suspends_an_unranked_members_incident() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = insert_group(&mut conn).await;
		let machine = insert_machine(&mut conn, Some(group)).await;
		let member = insert_application(
			&mut conn,
			machine,
			Some(group),
			None,
			"http://plain.invalid/",
		)
		.await;

		fail_application(&mut conn, member, "app_down").await;
		assert_eq!(open_ranks(&mut conn, group).await, vec![None]);

		database::maintenance_windows::MaintenanceWindow::declare(
			&mut conn,
			Scope::Group(group),
			None,
			Timestamp::now() + SignedDuration::from_hours(1),
			Some("upgrading"),
			Some("op"),
		)
		.await
		.expect("declare");

		assert!(
			open_ranks(&mut conn, group).await.is_empty(),
			"the group's window takes its unranked member out of the incident",
		);
	})
	.await
}

/// Rank is an application's, so a box takes the rank of the highest-ranked
/// workload on it: a shared host's disk filling is trouble for the most
/// important thing it runs.
// spec: INC#targets
#[tokio::test(flavor = "multi_thread")]
async fn a_box_takes_the_rank_of_its_highest_ranked_workload() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = insert_group(&mut conn).await;
		let machine = insert_machine(&mut conn, Some(group)).await;
		insert_application(
			&mut conn,
			machine,
			Some(group),
			Some("test"),
			"http://shared-test.invalid/",
		)
		.await;
		insert_application(
			&mut conn,
			machine,
			Some(group),
			Some("production"),
			"http://shared-prod.invalid/",
		)
		.await;

		assert_eq!(
			Scope::Machine(machine)
				.resolve_incident_target(&mut conn)
				.await
				.expect("resolve"),
			Some((
				IncidentTarget::Environment(group, ServerRank::Production),
				true
			)),
		);
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
		let test =
			insert_ranked_member(&mut conn, group, Some("test"), "http://test.invalid/").await;

		fail_application(&mut conn, production, "app_down").await;
		fail_application(&mut conn, test, "app_down").await;

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
			vec![
				"site test \u{b7} http://test.invalid/".to_string(),
				"site \u{b7} http://prod.invalid/".to_string(),
			],
			"production reads as the site, and test reads as the site's test",
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
