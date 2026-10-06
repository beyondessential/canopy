//! The `2026-10-05-151834-0000_shared_rank` migration gives every application
//! on a box the rank its siblings carry, holds that in the schema, and moves
//! each incident that targeted a group itself onto the group's headline
//! environment.
//!
//! Replays it for real: reverts it and the migrations after it, seeds the
//! shapes the deployed database holds, then re-applies them in order. The
//! later `machine_rank` migration keeps a box's rank and its applications'
//! together by trigger, which would rewrite the mixed ranks seeded here before
//! this migration ever saw them.
//!
//! spec: GRP#environments

use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::{AsyncPgConnection, RunQueryDsl, SimpleAsyncConnection as _};
use uuid::Uuid;

const UP: &str = include_str!("../../../../migrations/2026-10-05-151834-0000_shared_rank/up.sql");
const DOWN: &str =
	include_str!("../../../../migrations/2026-10-05-151834-0000_shared_rank/down.sql");
const MACHINE_RANK_UP: &str =
	include_str!("../../../../migrations/2026-10-06-040849-0000_machine_rank/up.sql");
const MACHINE_RANK_DOWN: &str =
	include_str!("../../../../migrations/2026-10-06-040849-0000_machine_rank/down.sql");

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

#[derive(QueryableByName)]
struct MaybeRank {
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	rank: Option<String>,
}

#[derive(QueryableByName)]
struct Count {
	#[diesel(sql_type = sql_types::BigInt)]
	n: i64,
}

async fn revert(conn: &mut AsyncPgConnection) {
	conn.batch_execute(MACHINE_RANK_DOWN)
		.await
		.expect("revert machine_rank");
	conn.batch_execute(DOWN).await.expect("revert");
}

async fn apply(conn: &mut AsyncPgConnection) {
	conn.batch_execute(UP).await.expect("apply");
	conn.batch_execute(MACHINE_RANK_UP)
		.await
		.expect("apply machine_rank");
}

async fn group(conn: &mut AsyncPgConnection) -> Uuid {
	sql_query("INSERT INTO server_groups (name) VALUES ('site') RETURNING id")
		.get_result::<RowId>(conn)
		.await
		.expect("group")
		.id
}

async fn machine(conn: &mut AsyncPgConnection, group: Uuid) -> Uuid {
	sql_query("INSERT INTO machines (name, group_id) VALUES ('box', $1) RETURNING id")
		.bind::<sql_types::Uuid, _>(group)
		.get_result::<RowId>(conn)
		.await
		.expect("machine")
		.id
}

async fn application(
	conn: &mut AsyncPgConnection,
	group: Uuid,
	machine: Uuid,
	kind: &str,
	rank: Option<&str>,
) -> Uuid {
	sql_query(
		"INSERT INTO applications (type, host, group_id, rank, machine_id) \
		 VALUES ($1, 'http://' || gen_random_uuid() || '.invalid/', $2, $3, $4) RETURNING id",
	)
	.bind::<sql_types::Text, _>(kind)
	.bind::<sql_types::Uuid, _>(group)
	.bind::<sql_types::Nullable<sql_types::Text>, _>(rank)
	.bind::<sql_types::Uuid, _>(machine)
	.get_result::<RowId>(conn)
	.await
	.expect("application")
	.id
}

async fn rank_of(conn: &mut AsyncPgConnection, application: Uuid) -> Option<String> {
	sql_query("SELECT rank FROM applications WHERE id = $1")
		.bind::<sql_types::Uuid, _>(application)
		.get_result::<MaybeRank>(conn)
		.await
		.expect("rank")
		.rank
}

/// An incident as the deployed database holds one: `rank` null for a group's
/// own.
async fn incident(
	conn: &mut AsyncPgConnection,
	group: Uuid,
	rank: Option<&str>,
	open: bool,
) -> Uuid {
	sql_query(
		"INSERT INTO incidents (server_group_id, rank, opened_at, closed_at) \
		 VALUES ($1, $2, NOW() - INTERVAL '2 hours', \
		         CASE WHEN $3 THEN NULL ELSE NOW() - INTERVAL '1 hour' END) RETURNING id",
	)
	.bind::<sql_types::Uuid, _>(group)
	.bind::<sql_types::Nullable<sql_types::Text>, _>(rank)
	.bind::<sql_types::Bool, _>(open)
	.get_result::<RowId>(conn)
	.await
	.expect("incident")
	.id
}

async fn incident_state(
	conn: &mut AsyncPgConnection,
	incident: Uuid,
) -> (Option<String>, bool, bool) {
	#[derive(QueryableByName)]
	struct State {
		#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
		rank: Option<String>,
		#[diesel(sql_type = sql_types::Bool)]
		open: bool,
		#[diesel(sql_type = sql_types::Bool)]
		old: bool,
	}
	let state: State = sql_query(
		"SELECT rank, closed_at IS NULL AS open, opened_at < NOW() - INTERVAL '90 minutes' AS old \
		 FROM incidents WHERE id = $1",
	)
	.bind::<sql_types::Uuid, _>(incident)
	.get_result(conn)
	.await
	.expect("incident state");
	(state.rank, state.open, state.old)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sidecar_takes_the_rank_of_the_central_beside_it() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		revert(&mut conn).await;
		let group = group(&mut conn).await;
		let machine = machine(&mut conn, group).await;
		let central = application(
			&mut conn,
			group,
			machine,
			"tamanu-central",
			Some("production"),
		)
		.await;
		let postgres = application(&mut conn, group, machine, "postgres", None).await;
		let other = self::machine(&mut conn, group).await;
		let demo = application(&mut conn, group, other, "tamanu-central", Some("demo")).await;
		let demo_database = application(&mut conn, group, other, "postgres", None).await;
		let alone = self::machine(&mut conn, group).await;
		let pending = application(&mut conn, group, alone, "postgres", None).await;

		apply(&mut conn).await;

		assert_eq!(
			rank_of(&mut conn, central).await.as_deref(),
			Some("production")
		);
		assert_eq!(
			rank_of(&mut conn, postgres).await.as_deref(),
			Some("production")
		);
		assert_eq!(rank_of(&mut conn, demo).await.as_deref(), Some("demo"));
		assert_eq!(
			rank_of(&mut conn, demo_database).await.as_deref(),
			Some("demo")
		);
		assert_eq!(
			rank_of(&mut conn, pending).await,
			None,
			"a box with nothing ranked stays pending"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_box_carrying_two_ranks_takes_the_higher_and_spellings_are_made_canonical() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		revert(&mut conn).await;
		let group = group(&mut conn).await;
		let machine = machine(&mut conn, group).await;
		let test = application(&mut conn, group, machine, "tamanu-facility", Some("test")).await;
		let legacy = application(&mut conn, group, machine, "tamanu-central", Some("Live")).await;
		let staging = self::machine(&mut conn, group).await;
		let staged =
			application(&mut conn, group, staging, "tamanu-central", Some("staging")).await;

		apply(&mut conn).await;

		assert_eq!(
			rank_of(&mut conn, test).await.as_deref(),
			Some("production")
		);
		assert_eq!(
			rank_of(&mut conn, legacy).await.as_deref(),
			Some("production")
		);
		assert_eq!(rank_of(&mut conn, staged).await.as_deref(), Some("clone"));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_constraint_holds_after_the_migration() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		revert(&mut conn).await;
		let group = group(&mut conn).await;
		let machine = machine(&mut conn, group).await;
		application(
			&mut conn,
			group,
			machine,
			"tamanu-central",
			Some("production"),
		)
		.await;
		apply(&mut conn).await;
		// The later machine_rank triggers give an application written onto a
		// ranked box the box's rank, so the constraint this migration installs
		// is only reached with them out of the way.
		conn.batch_execute("ALTER TABLE applications DISABLE TRIGGER USER")
			.await
			.expect("bypass the triggers");

		let refused = sql_query(
			"INSERT INTO applications (type, host, group_id, rank, machine_id) \
			 VALUES ('postgres', 'http://second.invalid/', $1, 'test', $2)",
		)
		.bind::<sql_types::Uuid, _>(group)
		.bind::<sql_types::Uuid, _>(machine)
		.execute(&mut conn)
		.await;
		assert!(refused.is_err());

		let ungrouped =
			sql_query("INSERT INTO incidents (server_group_id, opened_at) VALUES ($1, NOW())")
				.bind::<sql_types::Uuid, _>(group)
				.execute(&mut conn)
				.await;
		assert!(
			ungrouped.is_err(),
			"an incident with a group needs a rank, since none targets the group itself"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_open_group_incident_moves_to_the_headline_environment_keeping_its_timeline() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		revert(&mut conn).await;
		let group = group(&mut conn).await;
		let machine = machine(&mut conn, group).await;
		let central = application(
			&mut conn,
			group,
			machine,
			"tamanu-central",
			Some("production"),
		)
		.await;
		let test = self::machine(&mut conn, group).await;
		application(&mut conn, group, test, "tamanu-central", Some("test")).await;
		let own = incident(&mut conn, group, None, true).await;
		let issue: RowId = sql_query(
			"INSERT INTO issues (application_id, source, ref, check_name, observed_result, \
			   effective_result, message, active, first_seen, last_seen) \
			 VALUES ($1, 'alertd', 'unreachable', 'unreachable', 'failed', 'failed', 'm', true, \
			   NOW(), NOW()) RETURNING id",
		)
		.bind::<sql_types::Uuid, _>(central)
		.get_result(&mut conn)
		.await
		.expect("issue");
		sql_query(
			"INSERT INTO incident_issues (incident_id, issue_id, joined_at) VALUES ($1, $2, NOW())",
		)
		.bind::<sql_types::Uuid, _>(own)
		.bind::<sql_types::Uuid, _>(issue.id)
		.execute(&mut conn)
		.await
		.expect("member");

		apply(&mut conn).await;

		let (rank, open, old) = incident_state(&mut conn, own).await;
		assert_eq!(rank.as_deref(), Some("production"));
		assert!(open, "it is still the same open incident");
		assert!(old, "and keeps the time it opened");

		database::issues::reconcile_open_incidents(&mut conn)
			.await
			.expect("reconcile");
		assert!(
			incident_state(&mut conn, own).await.1,
			"the monitor's startup reconcile leaves it open while it still has a failure"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_group_incident_beside_an_open_headline_incident_closes_and_releases_its_members() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		revert(&mut conn).await;
		let group = group(&mut conn).await;
		let machine = machine(&mut conn, group).await;
		application(
			&mut conn,
			group,
			machine,
			"tamanu-central",
			Some("production"),
		)
		.await;
		let production = incident(&mut conn, group, Some("production"), true).await;
		let own = incident(&mut conn, group, None, true).await;
		let issue: RowId = sql_query(
			"INSERT INTO issues (server_group_id, source, ref, check_name, observed_result, \
			   effective_result, message, active, first_seen, last_seen) \
			 VALUES ($1, 'canopy', 'backup', 'backup', 'failed', 'failed', 'm', true, NOW(), NOW()) \
			 RETURNING id",
		)
		.bind::<sql_types::Uuid, _>(group)
		.get_result(&mut conn)
		.await
		.expect("issue");
		sql_query(
			"INSERT INTO incident_issues (incident_id, issue_id, joined_at) VALUES ($1, $2, NOW())",
		)
		.bind::<sql_types::Uuid, _>(own)
		.bind::<sql_types::Uuid, _>(issue.id)
		.execute(&mut conn)
		.await
		.expect("member");

		apply(&mut conn).await;

		assert!(
			incident_state(&mut conn, production).await.1,
			"production stays open"
		);
		assert!(
			!incident_state(&mut conn, own).await.1,
			"at most one is open per environment, so the group's own closes"
		);
		let live: Count = sql_query(
			"SELECT COUNT(*) AS n FROM incident_issues WHERE incident_id = $1 AND left_at IS NULL",
		)
		.bind::<sql_types::Uuid, _>(own)
		.get_result(&mut conn)
		.await
		.expect("count");
		assert_eq!(live.n, 0, "its issue is free to join production's incident");

		let reconciled = database::issues::reconcile_open_incidents(&mut conn)
			.await
			.expect("reconcile");
		assert_eq!(reconciled, 1);
		let joined: Count = sql_query(
			"SELECT COUNT(*) AS n FROM incident_issues WHERE incident_id = $1 AND left_at IS NULL",
		)
		.bind::<sql_types::Uuid, _>(production)
		.get_result(&mut conn)
		.await
		.expect("count");
		assert_eq!(
			joined.n, 1,
			"the monitor's startup reconcile moves the issue across"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_group_incident_with_nothing_ranked_closes_and_its_history_lands_on_production() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		revert(&mut conn).await;
		let group = group(&mut conn).await;
		let machine = machine(&mut conn, group).await;
		application(&mut conn, group, machine, "postgres", None).await;
		let open = incident(&mut conn, group, None, true).await;
		let past = incident(&mut conn, group, None, false).await;

		apply(&mut conn).await;

		assert!(
			!incident_state(&mut conn, open).await.1,
			"no environment to move to"
		);
		assert_eq!(
			incident_state(&mut conn, past).await.0.as_deref(),
			Some("production"),
			"a closed incident keeps a place in the group's history"
		);
	})
	.await
}

/// Slack hears of the close: an open that was delivered gets its resolve, and
/// one still waiting to be delivered is cancelled with no resolve.
#[tokio::test(flavor = "multi_thread")]
async fn closing_a_group_incident_tells_slack() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		revert(&mut conn).await;
		let mut open = Vec::new();
		for _ in 0..2 {
			let group = group(&mut conn).await;
			let machine = machine(&mut conn, group).await;
			application(&mut conn, group, machine, "postgres", None).await;
			open.push(incident(&mut conn, group, None, true).await);
		}
		let (announced, unannounced) = (open[0], open[1]);
		for (incident, delivered) in [(announced, true), (unannounced, false)] {
			sql_query(
				"INSERT INTO slack_outbox (kind, incident_id, payload, deliver_after, delivered_at) \
				 VALUES ('incident_open', $1, '{}'::jsonb, NOW(), \
				         CASE WHEN $2 THEN NOW() ELSE NULL END)",
			)
			.bind::<sql_types::Uuid, _>(incident)
			.bind::<sql_types::Bool, _>(delivered)
			.execute(&mut conn)
			.await
			.expect("outbox row");
		}

		apply(&mut conn).await;

		let resolves = |incident: Uuid| {
			sql_query(
				"SELECT COUNT(*) AS n FROM slack_outbox \
				 WHERE incident_id = $1 AND kind = 'incident_resolve'",
			)
			.bind::<sql_types::Uuid, _>(incident)
		};
		assert_eq!(
			resolves(announced)
				.get_result::<Count>(&mut conn)
				.await
				.unwrap()
				.n,
			1,
			"the delivered open is resolved"
		);
		assert_eq!(
			resolves(unannounced)
				.get_result::<Count>(&mut conn)
				.await
				.unwrap()
				.n,
			0,
			"Slack never heard of the other, so there is nothing to resolve"
		);
		let cancelled: Count = sql_query(
			"SELECT COUNT(*) AS n FROM slack_outbox \
			 WHERE incident_id = $1 AND kind = 'incident_open' AND gave_up_at IS NOT NULL",
		)
		.bind::<sql_types::Uuid, _>(unannounced)
		.get_result(&mut conn)
		.await
		.unwrap();
		assert_eq!(cancelled.n, 1);
	})
	.await
}
