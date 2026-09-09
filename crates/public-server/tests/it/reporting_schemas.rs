//! Dispatching reporting-schema builds, and who may publish what one produces.
//!
//! spec: RPT

use axum::http::StatusCode;
use diesel_async::SimpleAsyncConnection;

const GROUP: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
const OTHER_GROUP: &str = "ffffffff-ffff-ffff-ffff-ffffffffffff";
const MACHINE: &str = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
const CENTRAL: &str = "cccccccc-cccc-cccc-cccc-cccccccccccc";
const VERSION: &str = "22222222-2222-2222-2222-222222222222";

/// A group whose central runs 2.60.0, with a ready backup repo and a snapshot,
/// and a consumer device declared against it building reporting schemas.
async fn seed(conn: &mut database::diesel_async::AsyncPgConnection, consumer: uuid::Uuid) {
	conn.batch_execute(&format!(
		"INSERT INTO versions (id, major, minor, patch, changelog, status)
		 VALUES ('{VERSION}', 2, 60, 0, '', 'published');

		 INSERT INTO server_groups (id, name) VALUES
		 ('{GROUP}', 'kamaka'), ('{OTHER_GROUP}', 'drifting');

		 INSERT INTO machines (id, name, group_id) VALUES ('{MACHINE}', 'box', '{GROUP}');

		 INSERT INTO applications (id, type, name, host, machine_id, group_id)
		 VALUES ('{CENTRAL}', 'tamanu-central', 'central', 'https://c', '{MACHINE}', '{GROUP}');

		 INSERT INTO application_reported_detail (application_id, source, reported_at, version)
		 VALUES ('{CENTRAL}', 'tamanu', NOW(), '2.60.0');

		 INSERT INTO server_group_backup_config
		   (group_id, bucket, prefix, target_role_arn, maintenance_role_arn, repo_password_ref, status)
		 VALUES ('{GROUP}', 'b', 'p/', 'arn:t', 'arn:m', 'ref', 'ready');

		 INSERT INTO backup_runs
		   (id, device_id, machine_id, group_id, type, purpose, outcome, snapshot_id, reported_at)
		 VALUES (gen_random_uuid(), '{consumer}', '{MACHINE}', '{GROUP}', 'tamanu-postgres', 'backup', 'success', 'snap-1', NOW());

		 INSERT INTO restore_consumer_capabilities
		   (consumer_device_id, intent, description, semantics, params)
		 VALUES ('{consumer}', 'schema-build', 'builds schemas',
		         '[\"check\", \"once\", \"migrate\", \"reporting-schema\"]'::jsonb, '{{}}'::jsonb);

		 INSERT INTO restore_replicas
		   (consumer_device_id, group_id, type, intent, name, enabled, publishes_schemas)
		 VALUES ('{consumer}', '{GROUP}', 'tamanu-postgres', 'schema-build', 'schemas', true, true)",
	))
	.await
	.expect("seed");
}

/// A build is dispatched per pair on the group's central machine, naming the
/// pair's version rather than the machine's own upgrade candidate.
#[tokio::test(flavor = "multi_thread")]
async fn a_build_is_dispatched_per_pair_on_the_central() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn, device_id).await;

			let response = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			response.assert_status_ok();
			let entries: Vec<serde_json::Value> = response.json();

			let ours: Vec<&serde_json::Value> = entries
				.iter()
				.filter(|e| e["intent"] == "schema-build")
				.collect();

			assert_eq!(ours.len(), 1, "one entry for the group's one pair");
			assert_eq!(ours[0]["machine_id"], MACHINE, "restores the central's box");
			assert_eq!(
				ours[0]["target_version"], "2.60.0",
				"names the pair's version"
			);
			assert_eq!(ours[0]["application_type"], "tamanu-central");
		},
	)
	.await
}

/// A pair is dispatched once however many declarations cover its group. Each
/// entry costs a restore and a migrate, so a second declaration doubling the
/// list is paid for.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_declaration_dispatches_no_second_build() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn, device_id).await;

			conn.batch_execute(&format!(
				"INSERT INTO restore_replicas
					(consumer_device_id, group_id, type, intent, name, enabled, publishes_schemas)
				 VALUES ('{device_id}', '{GROUP}', 'tamanu-postgres', 'schema-build',
					'schemas-weekly', true, true)"
			))
			.await
			.expect("a second schema declaration");

			let response = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			response.assert_status_ok();
			let entries: Vec<serde_json::Value> = response.json();

			assert_eq!(
				entries
					.iter()
					.filter(|e| e["intent"] == "schema-build")
					.count(),
				1,
				"one entry for the group's one pair"
			);
		},
	)
	.await
}

/// A build restores the group's canonical central, so a declaration pinned to a
/// machine names something this dispatch cannot honour. Retargeting it silently
/// would build against a box the operator did not declare.
#[tokio::test(flavor = "multi_thread")]
async fn a_machine_scoped_declaration_builds_no_schema() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn, device_id).await;

			conn.batch_execute(&format!(
				"UPDATE restore_replicas SET machine_id = '{MACHINE}'
				 WHERE consumer_device_id = '{device_id}'"
			))
			.await
			.expect("pin the declaration to a machine");

			let response = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			response.assert_status_ok();
			let entries: Vec<serde_json::Value> = response.json();

			assert!(
				entries.iter().all(|e| e["intent"] != "schema-build"),
				"a build is per group, not per machine"
			);
		},
	)
	.await
}

/// `once` is keyed to the pair rather than the snapshot, so a pair that has been
/// built drops off the worklist and stays off while the snapshot moves on.
#[tokio::test(flavor = "multi_thread")]
async fn a_built_pair_drops_off_the_worklist() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn, device_id).await;

			// Record a build for the pair, riding a restore report as one does.
			conn.batch_execute(&format!(
				"INSERT INTO backup_restore_checks
				   (consumer_device_id, group_id, machine_id, type, intent, snapshot_id,
				    outcome, replica_healthy, observed_at, reported_at)
				 VALUES ('{device_id}', '{GROUP}', '{MACHINE}', 'tamanu-postgres',
				         'schema-build', 'snap-1', 'success', true, NOW(), NOW());

				 INSERT INTO reporting_schema_builds (check_id, group_id, version_id, built)
				 SELECT id, '{GROUP}', '{VERSION}', true FROM backup_restore_checks
				 ORDER BY id DESC LIMIT 1",
			))
			.await
			.expect("record a build");

			let response = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			response.assert_status_ok();
			let entries: Vec<serde_json::Value> = response.json();

			assert!(
				!entries.iter().any(|e| e["intent"] == "schema-build"),
				"a built pair is settled and not dispatched again"
			);

			// A newer snapshot does not bring it back: the key is the pair.
			conn.batch_execute(&format!(
				"INSERT INTO backup_runs
				   (id, device_id, machine_id, group_id, type, purpose, outcome, snapshot_id, reported_at)
				 VALUES (gen_random_uuid(), '{device_id}', '{MACHINE}', '{GROUP}', 'tamanu-postgres', 'backup', 'success', 'snap-2', NOW())",
			))
			.await
			.expect("newer snapshot");

			let response = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			let entries: Vec<serde_json::Value> = response.json();
			assert!(
				!entries.iter().any(|e| e["intent"] == "schema-build"),
				"a newer snapshot does not rebuild a schema the pair already has"
			);
		},
	)
	.await
}

/// Masking alters the configuration a schema follows from, so a declaration set
/// to redact builds nothing rather than building from a database that is no
/// longer the group's.
///
/// spec: RPT#the-build-contract
#[tokio::test(flavor = "multi_thread")]
async fn a_redacting_declaration_builds_no_schema() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn, device_id).await;

			conn.batch_execute(&format!(
				"UPDATE restore_replicas SET redacts = true
				 WHERE consumer_device_id = '{device_id}'"
			))
			.await
			.expect("set the declaration to redact");

			let response = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			response.assert_status_ok();
			let entries: Vec<serde_json::Value> = response.json();

			assert!(
				!entries.iter().any(|e| e["intent"] == "schema-build"),
				"a redacting declaration dispatches no build: {entries:?}"
			);
		},
	)
	.await
}

/// The configuration a schema follows from is held centrally, so every pair of
/// a group restores a central's snapshot. A group with no central has no
/// snapshot to build from, and dispatching against a facility would build a
/// schema from the wrong half of the deployment.
///
/// spec: RPT#the-build-contract
#[tokio::test(flavor = "multi_thread")]
async fn a_group_with_no_central_dispatches_nothing() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn, device_id).await;

			conn.batch_execute(&format!(
				"UPDATE applications SET type = 'tamanu-facility' WHERE id = '{CENTRAL}'"
			))
			.await
			.expect("leave the group with no central");

			let response = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			response.assert_status_ok();
			let entries: Vec<serde_json::Value> = response.json();

			assert!(
				!entries.iter().any(|e| e["intent"] == "schema-build"),
				"nothing to restore a central's snapshot from: {entries:?}"
			);
		},
	)
	.await
}

/// An intent may advertise `redact` alongside building schemas, and a schema is
/// built against the group's own data rather than a masked copy. Canopy owns
/// the masking parameters, and sending them unset is what tells a consumer not
/// to redact, so an entry carrying the defaults declared with the intent would
/// have the builder mask the very configuration it is reading.
///
/// spec: RST#the-masking-manifest
#[tokio::test(flavor = "multi_thread")]
async fn a_schema_build_is_never_told_to_redact() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn, device_id).await;

			conn.batch_execute(&format!(
				"UPDATE restore_consumer_capabilities
				 SET semantics = '[\"check\", \"once\", \"migrate\", \"reporting-schema\", \"redact\"]'::jsonb,
				     params = '{{\"redaction_manifest_url\": {{\"type\": \"text\",
				                  \"default\": \"https://masks.example/{{version}}.yaml\"}}}}'::jsonb
				 WHERE consumer_device_id = '{device_id}'",
			))
			.await
			.expect("advertise redaction too");

			let response = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			response.assert_status_ok();
			let entries: Vec<serde_json::Value> = response.json();

			let ours: Vec<&serde_json::Value> = entries
				.iter()
				.filter(|e| e["intent"] == "schema-build")
				.collect();
			assert_eq!(ours.len(), 1, "the pair is still dispatched");
			assert_eq!(
				ours[0]["params"]["redaction_manifest_url"],
				serde_json::Value::Null,
				"the parameter is advertised, and sent unset"
			);
		},
	)
	.await
}

/// A builder registers artifacts for the group its declaration covers, and is
/// refused another's the same way it would be refused a group that does not
/// exist.
#[tokio::test(flavor = "multi_thread")]
async fn a_builder_publishes_only_for_its_own_group() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn, device_id).await;

			let ours = public
				.post(&format!(
					"/artifacts/2.60.0/reporting-schema/any?group={GROUP}"
				))
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.add_header("content-type", "application/sql")
				.text("CREATE VIEW ...")
				.await;
			ours.assert_status_ok();

			let theirs = public
				.post(&format!(
					"/artifacts/2.60.0/reporting-schema/any?group={OTHER_GROUP}"
				))
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.text("CREATE VIEW ...")
				.await;
			assert_eq!(theirs.status_code(), StatusCode::FORBIDDEN);

			let nowhere = public
				.post(
					"/artifacts/2.60.0/reporting-schema/any?group=99999999-9999-9999-9999-999999999999",
				)
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.text("CREATE VIEW ...")
				.await;
			assert_eq!(
				nowhere.status_code(),
				theirs.status_code(),
				"a group it is not authorised for and one that does not exist answer alike"
			);
		},
	)
	.await
}

/// A schema is registered against one exact version. Canopy resolves a range
/// artifact for every version it covers, so a range registration would hand a
/// server a schema built for a version it does not run.
#[tokio::test(flavor = "multi_thread")]
async fn a_schema_registered_against_a_range_is_refused() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn, device_id).await;

			let ranged = public
				.post(&format!(
					"/artifacts/2.60.x/reporting-schema/any?group={GROUP}"
				))
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.add_header("content-type", "application/sql")
				.text("CREATE VIEW ...")
				.await;
			assert_eq!(ranged.status_code(), StatusCode::BAD_REQUEST);
		},
	)
	.await
}

/// A build is dispatched for a pair whose version Canopy already holds, so a
/// registration naming one it does not is refused. Drafting a release row for
/// it would put a builder's near-miss of a real version into the catalog every
/// machine reads.
#[tokio::test(flavor = "multi_thread")]
async fn a_schema_for_an_unknown_version_drafts_none() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn, device_id).await;

			let refused = public
				.post(&format!(
					"/artifacts/9999.0.0/reporting-schema/any?group={GROUP}"
				))
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.add_header("content-type", "application/sql")
				.text("CREATE VIEW ...")
				.await;
			assert_eq!(refused.status_code(), StatusCode::BAD_REQUEST);

			let catalog = database::versions::Version::get_all_including_drafts(&mut conn)
				.await
				.expect("the version catalog");
			assert!(
				!catalog.iter().any(|v| v.major == 9999),
				"no release row is drafted for it"
			);
		},
	)
	.await
}

/// A declaration an operator has turned off does not authorise anything. It is
/// the enabled declaration that covers a group, so a builder whose declaration
/// is disabled is refused its own group's artifacts.
#[tokio::test(flavor = "multi_thread")]
async fn a_disabled_declaration_authorises_nothing() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn, device_id).await;

			conn.batch_execute(&format!(
				"UPDATE restore_replicas SET enabled = false WHERE consumer_device_id = '{device_id}'"
			))
			.await
			.expect("disable the declaration");

			let refused = public
				.post(&format!(
					"/artifacts/2.60.0/reporting-schema/any?group={GROUP}"
				))
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.add_header("content-type", "application/sql")
				.text("CREATE VIEW ...")
				.await;

			assert_eq!(refused.status_code(), StatusCode::FORBIDDEN);
		},
	)
	.await
}

/// Restoring for a group is not the same authority as building its schema. A
/// consumer whose declaration covers the group but whose intent advertises no
/// `reporting-schema` semantic is refused, so a verify or migrate consumer
/// cannot publish a schema for the group it already restores.
#[tokio::test(flavor = "multi_thread")]
async fn restoring_for_a_group_does_not_authorise_publishing_its_schema() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn, device_id).await;

			conn.batch_execute(&format!(
				"UPDATE restore_consumer_capabilities
				 SET semantics = '[\"check\", \"once\", \"migrate\"]'::jsonb
				 WHERE consumer_device_id = '{device_id}'"
			))
			.await
			.expect("withdraw the semantic");

			let refused = public
				.post(&format!(
					"/artifacts/2.60.0/reporting-schema/any?group={GROUP}"
				))
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.add_header("content-type", "application/sql")
				.text("CREATE VIEW ...")
				.await;

			assert_eq!(refused.status_code(), StatusCode::FORBIDDEN);
		},
	)
	.await
}

/// A consumer registers its own capability set, so the semantics an intent
/// carries are its own claim: a device declared for the group can put
/// `reporting-schema` back on its intent in one request. What the operator set
/// on the declaration is what decides, so the refusal stands.
///
/// spec: RPT#the-build-contract
#[tokio::test(flavor = "multi_thread")]
async fn a_consumer_cannot_advertise_itself_into_publishing() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn, device_id).await;

			// An operator has this consumer restoring for the group, and has
			// not made it the group's publisher.
			conn.batch_execute(&format!(
				"UPDATE restore_replicas SET publishes_schemas = false
				 WHERE consumer_device_id = '{device_id}'"
			))
			.await
			.expect("the operator has not granted publishing");

			let readvertised = public
				.post("/restore-capabilities")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.json(&serde_json::json!({
					"intents": [{
						"intent": "schema-build",
						"description": "builds schemas",
						"semantics": ["check", "once", "migrate", "reporting-schema"],
						"params": {},
					}],
				}))
				.await;
			assert_eq!(
				readvertised.status_code(),
				StatusCode::NO_CONTENT,
				"a consumer may advertise what it likes"
			);

			let refused = public
				.post(&format!(
					"/artifacts/2.60.0/reporting-schema/any?group={GROUP}"
				))
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.add_header("content-type", "application/sql")
				.text("CREATE VIEW ...")
				.await;

			assert_eq!(
				refused.status_code(),
				StatusCode::FORBIDDEN,
				"advertising the semantic grants nothing"
			);
		},
	)
	.await
}

/// The flag is the operator's, and it is what the group's builds and the
/// operator page follow: a declaration without it is dispatched no build, so
/// Canopy never asks for one it would refuse to accept.
///
/// spec: RPT#the-build-contract
#[tokio::test(flavor = "multi_thread")]
async fn a_declaration_that_does_not_publish_is_dispatched_no_build() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn, device_id).await;

			conn.batch_execute(&format!(
				"UPDATE restore_replicas SET publishes_schemas = false
				 WHERE consumer_device_id = '{device_id}'"
			))
			.await
			.expect("withdraw publishing");

			let response = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			response.assert_status_ok();
			let entries: Vec<serde_json::Value> = response.json();

			assert!(
				!entries.iter().any(|e| e["intent"] == "schema-build"),
				"no build is dispatched for it: {entries:?}"
			);
		},
	)
	.await
}

/// A build report settles the pair: it stops the pair being dispatched again
/// and clears an operator's ask. A plain verify or migrate consumer declared
/// for the group can otherwise settle a pair no schema was ever built for, and
/// inject its own error string into the group's check.
#[tokio::test(flavor = "multi_thread")]
async fn restoring_for_a_group_does_not_authorise_settling_its_pairs() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn, device_id).await;
			let replica = declaration_id(&mut conn).await;

			conn.batch_execute(&format!(
				"UPDATE restore_consumer_capabilities
				 SET semantics = '[\"check\", \"once\", \"migrate\"]'::jsonb
				 WHERE consumer_device_id = '{device_id}'"
			))
			.await
			.expect("withdraw the semantic");

			let refused = public
				.post("/restore-verification")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.json(&build_report(
					replica,
					serde_json::json!({ "target_version": "2.60.0", "built": true }),
				))
				.await;

			assert_eq!(refused.status_code(), StatusCode::FORBIDDEN);
		},
	)
	.await
}

/// The artifacts route carries a body limit sized from the held-bytes cap, so a
/// schema past axum's 2 MiB default is taken in rather than answered with a
/// plain-text 413 for a limit sixteen times below the documented one.
#[tokio::test(flavor = "multi_thread")]
async fn a_schema_over_axum_s_default_is_taken_in() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn, device_id).await;

			let sql = "-- ".to_owned() + &"x".repeat(3 * 1024 * 1024);
			let response = public
				.post(&format!(
					"/artifacts/2.60.0/reporting-schema/any?group={GROUP}"
				))
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.add_header("content-type", "application/sql")
				.text(sql)
				.await;

			response.assert_status_ok();
		},
	)
	.await
}

/// A builder is authorised for the artifact its declaration names. Any other
/// type registered under that authority outranks the releaser's own for every
/// machine in the group, and those machines fetch and run what they are
/// offered.
#[tokio::test(flavor = "multi_thread")]
async fn a_builder_cannot_displace_the_group_s_installer() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn, device_id).await;

			let installer = public
				.post(&format!(
					"/artifacts/2.60.0/installer/windows?group={GROUP}"
				))
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.add_header("content-type", "application/octet-stream")
				.text("MZ...")
				.await;
			assert_eq!(installer.status_code(), StatusCode::FORBIDDEN);

			let schema = public
				.post(&format!(
					"/artifacts/2.60.0/reporting-schema/any?group={GROUP}"
				))
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.add_header("content-type", "application/sql")
				.text("CREATE VIEW ...")
				.await;
			schema.assert_status_ok();
		},
	)
	.await
}

/// Provenance a party can forge for itself answers nothing an operator asks of
/// it, so a run already recorded for another consumer is not one this
/// registration may name. A run Canopy has not seen is ordinary: the artifact
/// lands mid-restore, before the report of that restore does.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_another_consumer_reported_cannot_be_claimed() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn, device_id).await;

			let run = "77777777-7777-7777-7777-777777777777";
			let stranger = "88888888-8888-8888-8888-888888888888";
			conn.batch_execute(&format!(
				"INSERT INTO devices (id, role) VALUES ('{stranger}', 'backup-restore');
				 INSERT INTO backup_runs
					(id, device_id, group_id, machine_id, type, purpose, outcome, reported_at)
				 VALUES ('{run}', '{stranger}', '{GROUP}', '{MACHINE}',
					'tamanu-postgres', 'restore', 'success', now())"
			))
			.await
			.expect("another consumer's run");

			let claimed = public
				.post(&format!(
					"/artifacts/2.60.0/reporting-schema/any?group={GROUP}&run={run}"
				))
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.add_header("content-type", "application/sql")
				.text("CREATE VIEW ...")
				.await;
			assert_eq!(claimed.status_code(), StatusCode::BAD_REQUEST);

			let own = public
				.post(&format!(
					"/artifacts/2.60.0/reporting-schema/any?group={GROUP}&run=99999999-9999-9999-9999-999999999999"
				))
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.add_header("content-type", "application/sql")
				.text("CREATE VIEW ...")
				.await;
			own.assert_status_ok();
		},
	)
	.await
}

/// The declaration `seed` made, which a report has to name.
async fn declaration_id(conn: &mut database::diesel_async::AsyncPgConnection) -> uuid::Uuid {
	use diesel::{QueryableByName, sql_query, sql_types};
	use diesel_async::RunQueryDsl;

	#[derive(QueryableByName)]
	struct Row {
		#[diesel(sql_type = sql_types::Uuid)]
		id: uuid::Uuid,
	}

	sql_query("SELECT id FROM restore_replicas LIMIT 1")
		.get_result::<Row>(conn)
		.await
		.expect("the seeded declaration")
		.id
}

/// A builder's report of one run, with `build` as its reporting-schema block.
fn build_report(replica: uuid::Uuid, build: serde_json::Value) -> serde_json::Value {
	serde_json::json!({
		"replica_id": replica,
		"group": GROUP,
		"machine_id": MACHINE,
		"type": "tamanu-postgres",
		"intent": "schema-build",
		"snapshot_id": "snap-1",
		"outcome": "success",
		"replica_healthy": true,
		"observed_at": "2026-09-07T00:00:00Z",
		"reporting_schema": build,
	})
}

/// The build a report carries settles the pair it names, and is held against
/// the group's central application, whose database the schema followed from.
#[tokio::test(flavor = "multi_thread")]
async fn a_build_report_settles_the_pair_it_names() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn, device_id).await;
			let replica = declaration_id(&mut conn).await;

			let resp = public
				.post("/restore-verification")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.json(&build_report(
					replica,
					serde_json::json!({ "target_version": "2.60.0", "built": true }),
				))
				.await;
			resp.assert_status(StatusCode::NO_CONTENT);

			let build = database::reporting_schemas::ReportingSchemaBuild::latest_for_pair(
				&mut conn,
				GROUP.parse().unwrap(),
				VERSION.parse().unwrap(),
			)
			.await
			.expect("read the build")
			.expect("a build landed");

			assert!(build.built);
			assert_eq!(
				build.application_id,
				Some(CENTRAL.parse().unwrap()),
				"held against the central, not the reporting device's own machine"
			);
		},
	)
	.await
}

/// A consumer may name the version by id rather than by semver, which is what
/// the worklist entry hands it.
#[tokio::test(flavor = "multi_thread")]
async fn a_build_report_may_name_its_version_by_id() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn, device_id).await;
			let replica = declaration_id(&mut conn).await;

			let resp = public
				.post("/restore-verification")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.json(&build_report(
					replica,
					serde_json::json!({ "target_version_id": VERSION, "built": true }),
				))
				.await;
			resp.assert_status(StatusCode::NO_CONTENT);

			assert!(
				database::reporting_schemas::ReportingSchemaBuild::is_settled(
					&mut conn,
					GROUP.parse().unwrap(),
					VERSION.parse().unwrap(),
				)
				.await
				.expect("settled"),
			);
		},
	)
	.await
}

/// A build is for a pair, so a report that names no version cannot be
/// attributed to one and is refused rather than recorded against a guess.
#[tokio::test(flavor = "multi_thread")]
async fn a_build_report_naming_no_version_is_refused() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn, device_id).await;
			let replica = declaration_id(&mut conn).await;

			let resp = public
				.post("/restore-verification")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.json(&build_report(replica, serde_json::json!({ "built": true })))
				.await;

			assert_eq!(resp.status_code(), StatusCode::BAD_REQUEST);
		},
	)
	.await
}

/// A build that produced nothing settles the pair too, carrying the builder's
/// own description of what went wrong.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_build_report_carries_its_description() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn, device_id).await;
			let replica = declaration_id(&mut conn).await;

			let resp = public
				.post("/restore-verification")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.json(&build_report(
					replica,
					serde_json::json!({
						"target_version": "2.60.0",
						"built": false,
						"error": "views did not compile",
					}),
				))
				.await;
			resp.assert_status(StatusCode::NO_CONTENT);

			let pairs =
				database::reporting_schemas::pairs_for_group(&mut conn, GROUP.parse().unwrap())
					.await
					.expect("pairs");
			let pair = pairs
				.iter()
				.find(|p| p.version == "2.60.0")
				.expect("the pair");

			assert_eq!(pair.state, database::reporting_schemas::PairState::Failed);
			assert_eq!(pair.error.as_deref(), Some("views did not compile"));
		},
	)
	.await
}

/// A build rides the migrate pathway, so one run's report can carry both
/// blocks. The build is the one that settles the pair, and the migration
/// payload beside it is deliberately not recorded as a migration test.
#[tokio::test(flavor = "multi_thread")]
async fn a_report_carrying_both_records_only_the_build() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			use diesel::{QueryableByName, sql_query, sql_types};
			use diesel_async::RunQueryDsl;

			#[derive(QueryableByName)]
			struct Count {
				#[diesel(sql_type = sql_types::BigInt)]
				count: i64,
			}

			seed(&mut conn, device_id).await;
			let replica = declaration_id(&mut conn).await;

			let mut body = build_report(
				replica,
				serde_json::json!({ "target_version": "2.60.0", "built": true }),
			);
			body["migration"] = serde_json::json!({
				"target_version": "2.60.0",
				"total_elapsed_seconds": 12,
				"data_bytes_before": 1_000,
				"data_bytes_after": 1_200,
				"timings": [],
			});

			let resp = public
				.post("/restore-verification")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.json(&body)
				.await;
			resp.assert_status(StatusCode::NO_CONTENT);

			assert!(
				database::reporting_schemas::ReportingSchemaBuild::is_settled(
					&mut conn,
					GROUP.parse().unwrap(),
					VERSION.parse().unwrap(),
				)
				.await
				.expect("settled"),
				"the build is what settles the pair"
			);

			let migrations = sql_query("SELECT COUNT(*) AS count FROM migration_tests")
				.get_result::<Count>(&mut conn)
				.await
				.expect("count")
				.count;
			assert_eq!(
				migrations, 0,
				"the migration payload beside a build is not a migration test"
			);
		},
	)
	.await
}

/// A schema the builder registers is what the group's machines are later
/// offered, byte for byte, under the version it was built for.
///
/// The one device stands in for both the builder and a machine of the group:
/// which credential may do which is settled by the refusals above and in
/// `artifact_scopes`, and what this asserts is that the bytes survive the trip
/// and that the listing's own `download_url` is the one that fetches them.
#[tokio::test(flavor = "multi_thread")]
async fn a_registered_schema_is_offered_back_byte_for_byte() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn, device_id).await;
			conn.batch_execute(&format!(
				"UPDATE machines SET device_id = '{device_id}' WHERE id = '{MACHINE}'"
			))
			.await
			.expect("enrol the machine");

			let sql = "CREATE VIEW reporting.encounters AS SELECT 1;";

			public
				.post(&format!(
					"/artifacts/2.60.0/reporting-schema/any?group={GROUP}"
				))
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.add_header("content-type", "application/sql")
				.text(sql)
				.await
				.assert_status_ok();

			let listing = public
				.get("/versions/2.60.0/artifacts")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			listing.assert_status_ok();

			let artifacts: Vec<serde_json::Value> = listing.json();
			let schema = artifacts
				.iter()
				.find(|a| a["artifact_type"] == "reporting-schema")
				.expect("the group's schema is offered");

			assert_eq!(schema["platform"], "any");
			assert_eq!(schema["group_id"], GROUP);
			assert_eq!(
				schema["version_id"], VERSION,
				"published against the exact version, not a range"
			);
			assert!(
				schema["version_range_pattern"].is_null(),
				"a schema follows the migrations one version applies: {schema}"
			);
			assert_eq!(
				schema["digest"].as_str().expect("a digest"),
				database::artifacts::digest_of(sql.as_bytes()),
				"the digest describes the bytes canopy took in"
			);

			// Follow the URL the listing handed out rather than rebuilding it,
			// so the offer a device actually receives is what gets fetched.
			let offered_url = schema["download_url"].as_str().expect("a download url");
			let path = offered_url
				.split_once("/versions/")
				.map(|(_, rest)| format!("/versions/{rest}"))
				.expect("the offer names a versions path");

			let download = public
				.get(&path)
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			download.assert_status_ok();
			assert_eq!(download.text(), sql);
		},
	)
	.await
}

/// A facility is offered the same schema as its group's centrals: a schema
/// follows the group and the version rather than the application it was built
/// from, and the build only ever runs against a central's snapshot.
#[tokio::test(flavor = "multi_thread")]
async fn a_facility_is_offered_the_same_schema_as_its_centrals() {
	commons_tests::server::run_with_device_auth(
		"machine",
		async |mut conn, cert, device_id, public, _| {
			// A builder of its own, since the authenticated device here is the
			// facility's machine rather than the consumer that built the schema.
			let consumer = uuid::Uuid::new_v4();
			conn.batch_execute(&format!(
				"INSERT INTO devices (id, role) VALUES ('{consumer}', 'backup-restore')"
			))
			.await
			.expect("the builder device");
			seed(&mut conn, consumer).await;

			let digest = database::artifacts::digest_of(b"the group's schema");
			conn.batch_execute(&format!(
				"INSERT INTO artifacts
				   (version_id, platform, artifact_type, group_id, content, content_type, digest)
				 VALUES ('{VERSION}', 'any', 'reporting-schema', '{GROUP}',
				         'the group''s schema'::bytea, 'application/sql', '{digest}');

				 INSERT INTO machines (id, name, group_id, device_id)
				 VALUES (gen_random_uuid(), 'facility-box', '{GROUP}', '{device_id}')"
			))
			.await
			.expect("seed the schema and a facility box");

			let listing = public
				.get("/versions/2.60.0/artifacts")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			listing.assert_status_ok();

			let artifacts: Vec<serde_json::Value> = listing.json();
			let schema = artifacts
				.iter()
				.find(|a| a["artifact_type"] == "reporting-schema")
				.expect("a facility's device is offered its group's schema");

			assert_eq!(schema["group_id"], GROUP);
		},
	)
	.await
}
