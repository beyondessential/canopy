//! A machine carries one rank, which the applications on it share, so a machine
//! is in the environment that rank names. An application is unranked only while
//! it is pending: new on a box that is not ranked yet.
//!
//! spec: GRP#environments

use commons_types::server::{app_type::ApplicationType, rank::ServerRank};
use database::{
	applications::Application,
	machines::{Machine, NewMachine},
	server_groups::ServerGroup,
};
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::{AsyncPgConnection, RunQueryDsl, SimpleAsyncConnection as _};
use uuid::Uuid;

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

async fn group(conn: &mut AsyncPgConnection) -> Uuid {
	sql_query("INSERT INTO server_groups (name) VALUES ('site') RETURNING id")
		.get_result::<RowId>(conn)
		.await
		.expect("group")
		.id
}

async fn machine(conn: &mut AsyncPgConnection, group: Uuid) -> Machine {
	Machine::create(
		conn,
		NewMachine {
			group_id: Some(group),
			..NewMachine::named("box")
		},
	)
	.await
	.expect("machine")
}

/// A live application inserted directly, as the schema sees it.
async fn insert(
	conn: &mut AsyncPgConnection,
	machine: &Machine,
	host: &str,
	rank: Option<&str>,
) -> Result<Uuid, diesel::result::Error> {
	sql_query(
		"INSERT INTO applications (type, host, group_id, rank, machine_id) \
		 VALUES ('tamanu-central', $1, $2, $3, $4) RETURNING id",
	)
	.bind::<sql_types::Text, _>(host)
	.bind::<sql_types::Nullable<sql_types::Uuid>, _>(machine.group_id)
	.bind::<sql_types::Nullable<sql_types::Text>, _>(rank)
	.bind::<sql_types::Uuid, _>(machine.id)
	.get_result::<RowId>(conn)
	.await
	.map(|row| row.id)
}

async fn arrive(conn: &mut AsyncPgConnection, machine: &Machine, key: &str) -> Application {
	Application::from_report_key(conn, machine, key, &ApplicationType::TamanuFacility, true)
		.await
		.expect("report")
		.expect("a recording push creates what it names")
}

async fn ranks_on(conn: &mut AsyncPgConnection, machine: &Machine) -> Vec<Option<ServerRank>> {
	let mut ranks: Vec<Option<ServerRank>> = machine
		.applications(conn)
		.await
		.expect("applications")
		.into_iter()
		.map(|application| application.rank)
		.collect();
	ranks.sort();
	ranks
}

/// The environment the box serves.
async fn serving(conn: &mut AsyncPgConnection, machine: Uuid) -> Option<ServerRank> {
	Machine::get_by_id(conn, machine)
		.await
		.expect("machine")
		.environment_rank()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_application_arriving_on_a_production_box_takes_production() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = group(&mut conn).await;
		let machine = machine(&mut conn, group).await;
		insert(
			&mut conn,
			&machine,
			"http://central.invalid/",
			Some("production"),
		)
		.await
		.expect("central");

		let arrived = arrive(&mut conn, &machine, "facility").await;
		assert_eq!(arrived.rank, Some(ServerRank::Production));
		assert_eq!(
			serving(&mut conn, machine.id).await,
			Some(ServerRank::Production)
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_application_arriving_where_nothing_is_ranked_is_pending() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = group(&mut conn).await;
		let machine = machine(&mut conn, group).await;

		let arrived = arrive(&mut conn, &machine, "facility").await;
		assert_eq!(arrived.rank, None, "the application is pending");
		assert_eq!(
			serving(&mut conn, machine.id).await,
			None,
			"and so is the machine"
		);
		assert!(
			ServerGroup::environment_ranks(&mut conn, &[group])
				.await
				.unwrap()
				.is_empty(),
			"a group with nothing ranked has no environments"
		);
		assert_eq!(
			ServerGroup::headline_rank(&mut conn, Some(group))
				.await
				.unwrap(),
			None
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_with_no_applications_can_be_ranked_and_what_arrives_takes_it() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = group(&mut conn).await;
		let machine = machine(&mut conn, group).await;
		assert_eq!(serving(&mut conn, machine.id).await, None);

		Machine::set_rank(&mut conn, machine.id, ServerRank::Test, Some("op"))
			.await
			.expect("rank the empty box");
		assert_eq!(
			serving(&mut conn, machine.id).await,
			Some(ServerRank::Test),
			"the box is ranked before anything on it has reported"
		);
		assert_eq!(
			Machine::get_by_id(&mut conn, machine.id)
				.await
				.unwrap()
				.rank,
			Some(ServerRank::Test)
		);

		let arrived = arrive(&mut conn, &machine, "facility").await;
		assert_eq!(
			arrived.rank,
			Some(ServerRank::Test),
			"what arrives on a ranked box is never pending"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_application_written_onto_a_ranked_box_takes_its_rank() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = group(&mut conn).await;
		let machine = machine(&mut conn, group).await;
		Machine::set_rank(&mut conn, machine.id, ServerRank::Demo, Some("op"))
			.await
			.expect("rank the empty box");

		insert(&mut conn, &machine, "http://unranked.invalid/", None)
			.await
			.expect("a raw insert naming no rank");
		insert(
			&mut conn,
			&machine,
			"http://other.invalid/",
			Some("production"),
		)
		.await
		.expect("a raw insert naming another rank");
		assert_eq!(
			ranks_on(&mut conn, &machine).await,
			vec![Some(ServerRank::Demo); 2],
			"the box's rank wins, as its group does"
		);
		assert_eq!(serving(&mut conn, machine.id).await, Some(ServerRank::Demo));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn ranking_an_application_by_any_writer_ranks_its_box() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = group(&mut conn).await;
		let machine = machine(&mut conn, group).await;
		let id = insert(&mut conn, &machine, "http://raw.invalid/", None)
			.await
			.expect("pending");
		assert_eq!(serving(&mut conn, machine.id).await, None);

		sql_query("UPDATE applications SET rank = 'clone' WHERE id = $1")
			.bind::<sql_types::Uuid, _>(id)
			.execute(&mut conn)
			.await
			.expect("a raw rank write");
		assert_eq!(
			serving(&mut conn, machine.id).await,
			Some(ServerRank::Clone),
			"the box cannot be left behind its applications"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn ranking_a_box_by_any_writer_ranks_its_live_applications() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = group(&mut conn).await;
		let machine = machine(&mut conn, group).await;
		insert(&mut conn, &machine, "http://one.invalid/", Some("test"))
			.await
			.expect("one");
		insert(&mut conn, &machine, "http://two.invalid/", Some("test"))
			.await
			.expect("two");
		let archived = insert(&mut conn, &machine, "http://gone.invalid/", Some("test"))
			.await
			.expect("gone");
		Application::soft_delete(&mut conn, archived)
			.await
			.expect("archive");

		sql_query("UPDATE machines SET rank = 'dev' WHERE id = $1")
			.bind::<sql_types::Uuid, _>(machine.id)
			.execute(&mut conn)
			.await
			.expect("a raw rank write");
		assert_eq!(
			ranks_on(&mut conn, &machine).await,
			vec![Some(ServerRank::Dev); 2],
			"the live applications move with the box"
		);
		assert_eq!(
			Application::get_by_id(&mut conn, archived)
				.await
				.unwrap()
				.rank,
			Some(ServerRank::Test),
			"an archived one keeps the rank it left at"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_box_created_in_the_same_statement_as_its_ranked_application_is_ranked() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = group(&mut conn).await;
		let machine = sql_query(
			"WITH m AS (INSERT INTO machines (name, group_id) VALUES ('box', $1) RETURNING id) \
			 INSERT INTO applications (type, host, group_id, rank, machine_id) \
			 SELECT 'tamanu-central', 'http://central.invalid/', $1, 'production', id FROM m \
			 RETURNING machine_id AS id",
		)
		.bind::<sql_types::Uuid, _>(group)
		.get_result::<RowId>(&mut conn)
		.await
		.expect("box and application in one statement")
		.id;
		assert_eq!(
			serving(&mut conn, machine).await,
			Some(ServerRank::Production)
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn restoring_onto_an_unranked_empty_box_ranks_the_box() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = group(&mut conn).await;
		let machine = machine(&mut conn, group).await;
		let central = insert(&mut conn, &machine, "http://central.invalid/", Some("demo"))
			.await
			.expect("central");
		Application::soft_delete(&mut conn, central)
			.await
			.expect("archive");
		// As a box whose applications were all archived when ranks moved onto
		// boxes: the migration leaves it unranked.
		sql_query("UPDATE machines SET rank = NULL WHERE id = $1")
			.bind::<sql_types::Uuid, _>(machine.id)
			.execute(&mut conn)
			.await
			.expect("unrank the box");

		let restored = Application::restore(&mut conn, central)
			.await
			.expect("restore");
		assert_eq!(restored.rank, Some(ServerRank::Demo));
		assert_eq!(
			serving(&mut conn, machine.id).await,
			Some(ServerRank::Demo),
			"the box takes the rank its application came back with"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn ranking_one_application_ranks_every_application_on_the_box() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = group(&mut conn).await;
		let machine = machine(&mut conn, group).await;
		let first = arrive(&mut conn, &machine, "one").await;
		arrive(&mut conn, &machine, "two").await;
		arrive(&mut conn, &machine, "three").await;
		assert_eq!(ranks_on(&mut conn, &machine).await, vec![None, None, None]);

		Application::set_rank(&mut conn, first.id, ServerRank::Demo, Some("op"))
			.await
			.expect("rank");
		assert_eq!(
			ranks_on(&mut conn, &machine).await,
			vec![Some(ServerRank::Demo); 3],
			"all three are ranked in one save"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn changing_a_ranked_box_moves_every_application_on_it() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = group(&mut conn).await;
		let machine = machine(&mut conn, group).await;
		insert(&mut conn, &machine, "http://one.invalid/", Some("test"))
			.await
			.expect("one");
		insert(&mut conn, &machine, "http://two.invalid/", Some("test"))
			.await
			.expect("two");

		Machine::set_rank(&mut conn, machine.id, ServerRank::Demo, Some("op"))
			.await
			.expect("rank");
		assert_eq!(
			ranks_on(&mut conn, &machine).await,
			vec![Some(ServerRank::Demo); 2]
		);
	})
	.await
}

/// The triggers keep a box to one rank under any writer; the exclusion
/// constraint holds it even where they are bypassed.
#[tokio::test(flavor = "multi_thread")]
async fn the_schema_refuses_two_ranks_on_one_box() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = group(&mut conn).await;
		let machine = machine(&mut conn, group).await;
		insert(
			&mut conn,
			&machine,
			"http://one.invalid/",
			Some("production"),
		)
		.await
		.expect("one");
		conn.batch_execute("ALTER TABLE applications DISABLE TRIGGER USER")
			.await
			.expect("bypass the triggers");

		assert!(
			insert(&mut conn, &machine, "http://two.invalid/", Some("test"))
				.await
				.is_err(),
			"a second rank on the box"
		);
		assert!(
			insert(&mut conn, &machine, "http://three.invalid/", None)
				.await
				.is_err(),
			"a pending application beside a ranked one"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_archived_application_at_another_rank_does_not_trip_the_constraint() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = group(&mut conn).await;
		let machine = machine(&mut conn, group).await;
		let old = insert(&mut conn, &machine, "http://old.invalid/", Some("test"))
			.await
			.expect("old");
		Application::soft_delete(&mut conn, old)
			.await
			.expect("archive");
		Machine::set_rank(&mut conn, machine.id, ServerRank::Production, Some("op"))
			.await
			.expect("re-rank the box");

		insert(
			&mut conn,
			&machine,
			"http://new.invalid/",
			Some("production"),
		)
		.await
		.expect("a live application at another rank beside the archived one");
		assert_eq!(
			Application::get_by_id(&mut conn, old).await.unwrap().rank,
			Some(ServerRank::Test),
			"the archived one keeps the rank it left at"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn restoring_an_application_brings_it_back_at_the_boxs_rank() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = group(&mut conn).await;
		let machine = machine(&mut conn, group).await;
		let central = insert(&mut conn, &machine, "http://central.invalid/", Some("test"))
			.await
			.expect("central");
		let facility = insert(
			&mut conn,
			&machine,
			"http://facility.invalid/",
			Some("test"),
		)
		.await
		.expect("facility");
		Application::soft_delete(&mut conn, facility)
			.await
			.expect("archive");

		Machine::set_rank(&mut conn, machine.id, ServerRank::Production, Some("op"))
			.await
			.expect("rank");
		let restored = Application::restore(&mut conn, facility)
			.await
			.expect("restore");
		assert_eq!(
			restored.rank,
			Some(ServerRank::Production),
			"the box is production now, so the application returns to it"
		);

		Application::soft_delete(&mut conn, facility)
			.await
			.expect("archive again");
		Application::soft_delete(&mut conn, central)
			.await
			.expect("archive the rest of the box");
		let alone = Application::restore(&mut conn, facility)
			.await
			.expect("restore");
		assert_eq!(
			alone.rank,
			Some(ServerRank::Production),
			"with no live application beside it, it keeps the rank it left with"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn restoring_a_whole_archived_box_one_application_at_a_time_keeps_its_rank() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = group(&mut conn).await;
		let machine = machine(&mut conn, group).await;
		let central = insert(
			&mut conn,
			&machine,
			"http://central.invalid/",
			Some("production"),
		)
		.await
		.expect("central");
		let facility = insert(
			&mut conn,
			&machine,
			"http://facility.invalid/",
			Some("production"),
		)
		.await
		.expect("facility");
		Machine::archive(&mut conn, machine.id)
			.await
			.expect("archive the box");

		let first = Application::restore(&mut conn, central)
			.await
			.expect("restore");
		let second = Application::restore(&mut conn, facility)
			.await
			.expect("restore");
		assert_eq!(first.rank, Some(ServerRank::Production));
		assert_eq!(second.rank, Some(ServerRank::Production));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pending_application_restores_pending() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = group(&mut conn).await;
		let machine = machine(&mut conn, group).await;
		let facility = arrive(&mut conn, &machine, "facility").await;
		Application::soft_delete(&mut conn, facility.id)
			.await
			.expect("archive");
		let restored = Application::restore(&mut conn, facility.id)
			.await
			.expect("restore");
		assert_eq!(restored.rank, None);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_archived_application_cannot_be_ranked() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = group(&mut conn).await;
		let machine = machine(&mut conn, group).await;
		insert(
			&mut conn,
			&machine,
			"http://central.invalid/",
			Some("production"),
		)
		.await
		.expect("central");
		let facility = insert(
			&mut conn,
			&machine,
			"http://facility.invalid/",
			Some("production"),
		)
		.await
		.expect("facility");
		Application::soft_delete(&mut conn, facility)
			.await
			.expect("archive");

		let refused = Application::set_rank(&mut conn, facility, ServerRank::Dev, Some("op")).await;
		assert!(
			refused.is_err(),
			"ranking an archived application is refused"
		);
		assert_eq!(
			ranks_on(&mut conn, &machine).await,
			vec![Some(ServerRank::Production)],
			"the live application beside it is left alone"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_group_with_only_pending_boxes_is_left_out_of_the_headline_ranks() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = group(&mut conn).await;
		let machine = machine(&mut conn, group).await;
		arrive(&mut conn, &machine, "facility").await;

		assert!(
			ServerGroup::highest_member_ranks(&mut conn, &[group])
				.await
				.unwrap()
				.is_empty()
		);

		Machine::set_rank(&mut conn, machine.id, ServerRank::Clone, Some("op"))
			.await
			.expect("rank");
		let ranks = ServerGroup::environment_ranks(&mut conn, &[group])
			.await
			.unwrap();
		assert_eq!(ranks.len(), 1);
		assert_eq!(ranks[0].rank, ServerRank::Clone);
		assert!(ranks[0].headline);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_archived_machine_cannot_be_ranked_and_serves_no_environment() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = group(&mut conn).await;
		let machine = machine(&mut conn, group).await;
		let central = insert(&mut conn, &machine, "http://central.invalid/", Some("test"))
			.await
			.expect("central");
		Machine::archive(&mut conn, machine.id)
			.await
			.expect("archive the box");

		assert!(
			Machine::set_rank(&mut conn, machine.id, ServerRank::Production, Some("op"))
				.await
				.is_err(),
			"an archived box is refused a rank"
		);
		let archived = Machine::get_by_id(&mut conn, machine.id).await.unwrap();
		assert_eq!(
			archived.rank,
			Some(ServerRank::Test),
			"it keeps the rank it was archived at"
		);
		assert_eq!(archived.environment_rank(), None, "and serves nothing");

		let restored = Application::restore(&mut conn, central)
			.await
			.expect("restore");
		assert_eq!(
			restored.rank,
			Some(ServerRank::Test),
			"what comes back takes the rank the box carries"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_application_moved_onto_a_ranked_box_takes_the_boxs_rank() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = group(&mut conn).await;
		let production = machine(&mut conn, group).await;
		insert(
			&mut conn,
			&production,
			"http://central.invalid/",
			Some("production"),
		)
		.await
		.expect("central");
		let demo = machine(&mut conn, group).await;
		let moving = insert(&mut conn, &demo, "http://moving.invalid/", Some("demo"))
			.await
			.expect("moving");

		sql_query("UPDATE applications SET machine_id = $1 WHERE id = $2")
			.bind::<sql_types::Uuid, _>(production.id)
			.bind::<sql_types::Uuid, _>(moving)
			.execute(&mut conn)
			.await
			.expect("a raw move to another box");
		assert_eq!(
			ranks_on(&mut conn, &production).await,
			vec![Some(ServerRank::Production); 2],
			"the box's rank wins, as its group does"
		);
		assert_eq!(
			serving(&mut conn, production.id).await,
			Some(ServerRank::Production)
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_application_unarchived_onto_a_ranked_box_takes_the_boxs_rank() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = group(&mut conn).await;
		let machine = machine(&mut conn, group).await;
		insert(&mut conn, &machine, "http://central.invalid/", Some("test"))
			.await
			.expect("central");
		let facility = insert(
			&mut conn,
			&machine,
			"http://facility.invalid/",
			Some("test"),
		)
		.await
		.expect("facility");
		Application::soft_delete(&mut conn, facility)
			.await
			.expect("archive");
		Machine::set_rank(&mut conn, machine.id, ServerRank::Production, Some("op"))
			.await
			.expect("rank");

		sql_query("UPDATE applications SET deleted_at = NULL WHERE id = $1")
			.bind::<sql_types::Uuid, _>(facility)
			.execute(&mut conn)
			.await
			.expect("a raw un-archive");
		assert_eq!(
			ranks_on(&mut conn, &machine).await,
			vec![Some(ServerRank::Production); 2],
			"the box is not re-ranked to the rank the application left at"
		);
	})
	.await
}

#[derive(QueryableByName)]
struct MaybeRank {
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	rank: Option<String>,
}

/// The SQL that keeps a box's rank and its applications' together reads a
/// spelling the way `ServerRank` does: every spelling it reads, in any case,
/// names the same rank, and anything else names none.
#[tokio::test(flavor = "multi_thread")]
async fn the_schema_reads_rank_spellings_as_server_rank_does() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let mut spellings: Vec<String> = Vec::new();
		for (spelling, _) in ServerRank::SPELLINGS {
			spellings.push((*spelling).to_owned());
			spellings.push(spelling.to_ascii_uppercase());
		}
		spellings.extend(["nonsense".to_owned(), String::new()]);

		for spelling in spellings {
			let sql = sql_query("SELECT rank_canonical($1) AS rank")
				.bind::<sql_types::Text, _>(&spelling)
				.get_result::<MaybeRank>(&mut conn)
				.await
				.expect("rank_canonical")
				.rank;
			let rust = spelling
				.parse::<ServerRank>()
				.ok()
				.map(|rank| rank.to_string());
			assert_eq!(sql, rust, "{spelling:?}");
		}
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_box_is_refused_a_rank_no_one_recognises() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = group(&mut conn).await;
		let machine = machine(&mut conn, group).await;
		for rank in ["nonsense", "Production", "live"] {
			assert!(
				sql_query("UPDATE machines SET rank = $1 WHERE id = $2")
					.bind::<sql_types::Text, _>(rank)
					.bind::<sql_types::Uuid, _>(machine.id)
					.execute(&mut conn)
					.await
					.is_err(),
				"{rank:?} is not how a box's rank is written"
			);
		}
	})
	.await
}
