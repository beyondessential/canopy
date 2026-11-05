//! The scan and the model's first look, against stand-ins for OSV, Tamanu's
//! repository and the model provider.
//!
//! spec: VUL, VTR

use std::{
	collections::HashMap,
	sync::{Arc, Mutex},
};

use axum::{
	Json, Router,
	extract::{Path, State},
	routing::{get, post},
};
use commons_types::{
	server::{TagMap, app_type::ApplicationType, rank::ServerRank},
	version::VersionStatus,
};
use database::{
	applications::Application,
	audit::{self, Actor, Filter},
	machines::{Machine, NewMachine},
	pg_duration::PgDuration,
	reported_detail::ReportedDetail,
	url_field::UrlField,
	versions::{NewVersion, Version},
	vulnerabilities::{
		Decision, ItemDetail, ListFilter, NewDecision, Outcome, Source, Status, Suggestion, Via,
		today,
	},
};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use jiff::SignedDuration;
use jobs::vulnerabilities::{
	ai::{self, Config, Model},
	osv_client::OsvClient,
	scan::{Lockfiles, Scanner},
};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Default)]
struct Fake {
	/// Advisories OSV knows, by id: (modified, record).
	advisories: HashMap<String, Value>,
	/// What querybatch says for a package name: pages of (id, modified).
	answers: HashMap<String, Vec<Vec<(String, String)>>>,
	/// Every query asked, as (package, version, page token).
	queries: Vec<(String, String, Option<String>)>,
	/// Every advisory record fetched.
	fetched: Vec<String>,
	/// Every lockfile fetched.
	lockfiles: Vec<String>,
	/// What the model replies with, in order.
	replies: Vec<String>,
	/// A database a person records a decision in while the model answers.
	decided_while_answering: Option<String>,
	/// Every prompt the model was given.
	prompts: Vec<String>,
}

type Shared = Arc<Mutex<Fake>>;

const LOCKFILE: &str = r#"{
	"lockfileVersion": 3,
	"packages": {
		"": {},
		"packages/server": { "name": "@tamanu/server", "version": "1.0.0", "dependencies": { "left-pad": "1" } },
		"node_modules/left-pad": { "version": "1.0.0" },
		"node_modules/right-pad": { "version": "2.0.0" }
	}
}"#;

async fn query_batch(State(fake): State<Shared>, Json(body): Json<Value>) -> Json<Value> {
	let mut fake = fake.lock().unwrap();
	let mut results = Vec::new();
	for query in body["queries"].as_array().unwrap() {
		let name = query["package"]["name"].as_str().unwrap().to_owned();
		let version = query["version"].as_str().unwrap().to_owned();
		let token = query["page_token"].as_str().map(str::to_owned);
		fake.queries.push((name.clone(), version, token.clone()));
		let page: usize = token.as_deref().map(|t| t.parse().unwrap()).unwrap_or(0);
		let pages = fake.answers.get(&name).cloned().unwrap_or_default();
		let vulns: Vec<Value> = pages
			.get(page)
			.map(|page| {
				page.iter()
					.map(|(id, modified)| json!({"id": id, "modified": modified}))
					.collect()
			})
			.unwrap_or_default();
		let mut result = json!({ "vulns": vulns });
		if page + 1 < pages.len() {
			result["next_page_token"] = json!((page + 1).to_string());
		}
		results.push(if vulns_is_empty(&result) {
			json!({})
		} else {
			result
		});
	}
	Json(json!({ "results": results }))
}

fn vulns_is_empty(result: &Value) -> bool {
	result["vulns"].as_array().is_none_or(Vec::is_empty)
}

async fn vuln(
	State(fake): State<Shared>,
	Path(id): Path<String>,
) -> Result<Json<Value>, axum::http::StatusCode> {
	let mut fake = fake.lock().unwrap();
	fake.fetched.push(id.clone());
	fake.advisories
		.get(&id)
		.cloned()
		.map(Json)
		.ok_or(axum::http::StatusCode::NOT_FOUND)
}

async fn lockfile(State(fake): State<Shared>, Path(version): Path<String>) -> String {
	fake.lock().unwrap().lockfiles.push(version);
	LOCKFILE.to_owned()
}

async fn completion(State(fake): State<Shared>, Json(body): Json<Value>) -> Json<Value> {
	let (reply, decided_in) = {
		let mut fake = fake.lock().unwrap();
		if let Some(prompt) = body["messages"][1]["content"].as_str() {
			fake.prompts.push(prompt.to_owned());
		}
		let reply = if fake.replies.is_empty() {
			String::new()
		} else {
			fake.replies.remove(0)
		};
		(reply, fake.decided_while_answering.take())
	};
	if let Some(url) = decided_in {
		let mut conn = AsyncPgConnection::establish(&url).await.unwrap();
		Decision::record(
			&mut conn,
			NewDecision {
				vulnerability_id: "GHSA-1".into(),
				source: Source::Tamanu,
				outcome: Outcome::WillFix,
				reason: None,
				review_by: None,
				issue_url: None,
				ground: None,
				actor: Actor::operator("admin@example.com", None),
				via: Via::Ui,
			},
			today(),
		)
		.await
		.unwrap();
	}
	Json(json!({ "choices": [{ "message": { "content": reply } }] }))
}

async fn serve(fake: Shared) -> String {
	let router = Router::new()
		.route("/v1/querybatch", post(query_batch))
		.route("/v1/vulns/{id}", get(vuln))
		.route("/{version}/package-lock.json", get(lockfile))
		.route("/chat/completions", post(completion))
		.with_state(fake);
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let addr = listener.local_addr().unwrap();
	tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
	format!("http://{addr}")
}

fn advisory(id: &str, modified: &str, severity: &str, fixed: &str) -> Value {
	json!({
		"id": id,
		"summary": format!("{id} summary"),
		"details": "details",
		"modified": modified,
		"database_specific": { "severity": severity },
		"affected": [{
			"package": { "ecosystem": "npm", "name": "left-pad" },
			"ranges": [{ "type": "SEMVER", "events": [{ "introduced": "0" }, { "fixed": fixed }] }],
		}],
	})
}

fn application(machine_id: Uuid) -> Application {
	Application {
		id: Uuid::new_v4(),
		name: Some("central".into()),
		host: Some(UrlField("https://central.example/".parse().unwrap())),
		r#type: ApplicationType::TamanuCentral,
		rank: Some(ServerRank::Production),
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

/// A production Tamanu on 2.3.4.
async fn production_tamanu(conn: &mut AsyncPgConnection) {
	diesel::insert_into(database::schema::versions::table)
		.values(NewVersion {
			major: 2,
			minor: 3,
			patch: 4,
			status: VersionStatus::Published,
			changelog: String::new(),
			device_id: None,
		})
		.returning(<Version as diesel::SelectableHelper<_>>::as_select())
		.get_result(conn)
		.await
		.unwrap();
	let machine = Machine::create(
		conn,
		NewMachine {
			rank: Some(ServerRank::Production),
			..NewMachine::named("box")
		},
	)
	.await
	.unwrap();
	let app = Application::create(conn, application(machine.id))
		.await
		.unwrap();
	ReportedDetail::record(
		conn,
		Some(app.id),
		machine.id,
		"alertd",
		&json!({}),
		Some(&"2.3.4".parse().unwrap()),
	)
	.await
	.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_scan_reads_each_lockfile_once_and_each_advisory_until_it_is_revised() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		production_tamanu(&mut conn).await;
		let fake: Shared = Arc::default();
		{
			let mut f = fake.lock().unwrap();
			f.advisories.insert(
				"GHSA-1".into(),
				advisory("GHSA-1", "2026-10-01T00:00:00Z", "HIGH", "2.0.0"),
			);
			// left-pad's answer comes over two pages.
			f.answers.insert(
				"left-pad".into(),
				vec![
					vec![("GHSA-1".into(), "2026-10-01T00:00:00Z".into())],
					vec![("GHSA-2".into(), "2026-10-02T00:00:00Z".into())],
				],
			);
			f.advisories.insert(
				"GHSA-2".into(),
				advisory("GHSA-2", "2026-10-02T00:00:00Z", "LOW", "9.0.0"),
			);
		}
		let base = serve(fake.clone()).await;
		let scanner = Scanner::new(OsvClient::new(&base), Lockfiles::new(&base));

		let report = scanner.scan(&mut conn).await.unwrap();
		assert_eq!(report.lockfiles_read, 1);
		assert_eq!(report.package_versions_queried, 2, "left-pad and right-pad");
		assert_eq!(
			report.advisories_fetched, 2,
			"results past a page are followed"
		);
		{
			let f = fake.lock().unwrap();
			assert_eq!(f.lockfiles, ["v2.3.4"]);
			assert!(
				f.queries.iter().any(|(_, _, token)| token.is_some()),
				"the second page was asked for"
			);
		}

		let listing = database::vulnerabilities::ItemSummary::list(
			&mut conn,
			&ListFilter::default(),
			today(),
		)
		.await
		.unwrap();
		let ids: Vec<&str> = listing.items.iter().map(|i| i.id.as_str()).collect();
		assert_eq!(ids, ["GHSA-1", "GHSA-2"], "by severity");
		assert_eq!(listing.items[0].exposed, 1);

		// Nothing changed upstream: neither a lockfile nor a record is fetched again.
		let again = scanner.scan(&mut conn).await.unwrap();
		assert_eq!(again.lockfiles_read, 0);
		assert_eq!(again.advisories_fetched, 0);
		assert_eq!(fake.lock().unwrap().lockfiles.len(), 1);

		// OSV revises one of them.
		{
			let mut f = fake.lock().unwrap();
			f.advisories.insert(
				"GHSA-2".into(),
				advisory("GHSA-2", "2026-10-09T00:00:00Z", "CRITICAL", "9.0.0"),
			);
			f.answers.get_mut("left-pad").unwrap()[1][0].1 = "2026-10-09T00:00:00Z".into();
		}
		let revised = scanner.scan(&mut conn).await.unwrap();
		assert_eq!(revised.advisories_fetched, 1, "only the revised record");
		let after = database::vulnerabilities::ItemSummary::list(
			&mut conn,
			&ListFilter::default(),
			today(),
		)
		.await
		.unwrap();
		assert_eq!(after.items[0].severity, database::osv::Severity::Critical);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_scan_reopens_a_decided_item_when_severity_rises_and_audits_it_as_canopys() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		production_tamanu(&mut conn).await;
		let fake: Shared = Arc::default();
		{
			let mut f = fake.lock().unwrap();
			f.advisories.insert(
				"GHSA-1".into(),
				advisory("GHSA-1", "2026-10-01T00:00:00Z", "LOW", "2.0.0"),
			);
			f.answers.insert(
				"left-pad".into(),
				vec![vec![("GHSA-1".into(), "2026-10-01T00:00:00Z".into())]],
			);
		}
		let base = serve(fake.clone()).await;
		let scanner = Scanner::new(OsvClient::new(&base), Lockfiles::new(&base));
		scanner.scan(&mut conn).await.unwrap();

		Decision::record(
			&mut conn,
			NewDecision {
				vulnerability_id: "GHSA-1".into(),
				source: Source::Tamanu,
				outcome: Outcome::NotAffected,
				reason: Some("never reached".into()),
				review_by: None,
				issue_url: None,
				ground: None,
				actor: Actor::operator("admin@example.com", None),
				via: Via::Ui,
			},
			today(),
		)
		.await
		.unwrap();

		{
			let mut f = fake.lock().unwrap();
			f.advisories.insert(
				"GHSA-1".into(),
				advisory("GHSA-1", "2026-10-05T00:00:00Z", "CRITICAL", "2.0.0"),
			);
			f.answers.get_mut("left-pad").unwrap()[0][0].1 = "2026-10-05T00:00:00Z".into();
		}
		let report = scanner.scan(&mut conn).await.unwrap();
		assert_eq!(report.reopened, 1);

		let detail = ItemDetail::load(&mut conn, "GHSA-1", Source::Tamanu, today())
			.await
			.unwrap();
		assert_eq!(detail.item.status, Status::Open);
		assert!(detail.item.reopened.is_some());

		let log = audit::find(
			&mut conn,
			&Filter {
				action: Some("vulnerability_reopened".to_owned()),
				limit: 10,
				..Default::default()
			},
		)
		.await
		.unwrap();
		assert_eq!(log.len(), 1);
		assert!(matches!(&log[0].actor, Actor::Canopy { job } if job == "vulnerability-scan"));
	})
	.await;
}

fn model(base: &str) -> Model {
	Model::new(Config {
		api_key: "key".into(),
		model: "glm-flash".into(),
		base_url: base.to_owned(),
	})
}

async fn scanned(conn: &mut AsyncPgConnection) -> (Shared, String) {
	production_tamanu(conn).await;
	let fake: Shared = Arc::default();
	{
		let mut f = fake.lock().unwrap();
		f.advisories.insert(
			"GHSA-1".into(),
			advisory("GHSA-1", "2026-10-01T00:00:00Z", "HIGH", "2.0.0"),
		);
		f.answers.insert(
			"left-pad".into(),
			vec![vec![("GHSA-1".into(), "2026-10-01T00:00:00Z".into())]],
		);
	}
	let base = serve(fake.clone()).await;
	Scanner::new(OsvClient::new(&base), Lockfiles::new(&base))
		.scan(conn)
		.await
		.unwrap();
	(fake, base)
}

async fn item(conn: &mut AsyncPgConnection) -> ItemDetail {
	ItemDetail::load(conn, "GHSA-1", Source::Tamanu, today())
		.await
		.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_not_affected_answer_on_a_listed_ground_becomes_the_models_decision() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (fake, base) = scanned(&mut conn).await;
		fake.lock().unwrap().replies.push(
			json!({"outcome": "not_affected", "ground": "build_time_only", "reason": "Only the bundler loads it."})
				.to_string(),
		);

		assert_eq!(ai::pass(&mut conn, &model(&base), 10).await.unwrap(), 1);

		let detail = item(&mut conn).await;
		assert_eq!(detail.item.status, Status::NotAffected);
		let decision = detail.item.decision.unwrap();
		assert_eq!(decision.via, Via::Model);
		assert!(matches!(&decision.actor, Actor::Canopy { job } if job == "ai-worker"));

		let log = audit::find(
			&mut conn,
			&Filter {
				action: Some("vulnerability_decided".to_owned()),
				limit: 10,
				..Default::default()
			},
		)
		.await
		.unwrap();
		assert_eq!(log.len(), 1);
		assert_eq!(log[0].detail["via"], "model");

		assert_eq!(
			ai::pass(&mut conn, &model(&base), 10).await.unwrap(),
			0,
			"an item is looked at once"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn any_other_answer_is_a_suggestion_and_the_item_stays_open() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (fake, base) = scanned(&mut conn).await;
		fake.lock().unwrap().replies.push(
			json!({"outcome": "not_affected", "ground": "none", "reason": "Looks unused."})
				.to_string(),
		);

		ai::pass(&mut conn, &model(&base), 10).await.unwrap();

		let detail = item(&mut conn).await;
		assert_eq!(detail.item.status, Status::Open);
		let suggestion = detail.item.suggestion.unwrap();
		assert_eq!(suggestion.outcome, Outcome::NotAffected);
		assert_eq!(suggestion.model, "glm-flash");

		let log = audit::find(
			&mut conn,
			&Filter {
				action: Some("vulnerability_decided".to_owned()),
				limit: 10,
				..Default::default()
			},
		)
		.await
		.unwrap();
		assert!(log.is_empty(), "a suggestion is not audited");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_item_a_person_decided_before_only_gets_a_suggestion() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (fake, base) = scanned(&mut conn).await;
		Decision::record(
			&mut conn,
			NewDecision {
				vulnerability_id: "GHSA-1".into(),
				source: Source::Tamanu,
				outcome: Outcome::WillFix,
				reason: None,
				review_by: None,
				issue_url: None,
				ground: None,
				actor: Actor::operator("admin@example.com", None),
				via: Via::Ui,
			},
			today(),
		)
		.await
		.unwrap();
		// Reopen it so the model looks again.
		let counted = database::vulnerabilities::CountedFleet::load(&mut conn)
			.await
			.unwrap()
			.packages(&mut conn)
			.await
			.unwrap();
		diesel::sql_query("UPDATE vulnerability_decisions SET severity_held = 'low'")
			.execute(&mut conn)
			.await
			.unwrap();
		diesel::sql_query("UPDATE vulnerabilities SET severity = 'critical'")
			.execute(&mut conn)
			.await
			.unwrap();
		let reopened = database::vulnerabilities::Reopened::pass(&mut conn, &counted, today())
			.await
			.unwrap();
		assert_eq!(reopened.len(), 1);

		fake.lock().unwrap().replies.push(
			json!({"outcome": "not_affected", "ground": "frontend_only", "reason": "Browser only."})
				.to_string(),
		);
		ai::pass(&mut conn, &model(&base), 10).await.unwrap();

		let detail = item(&mut conn).await;
		assert_eq!(
			detail.item.status,
			Status::Open,
			"a person's decision is not replaced"
		);
		assert!(detail.item.suggestion.is_some());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_person_deciding_while_the_model_answers_keeps_their_decision() {
	commons_tests::db::TestDb::run(async |mut conn, url| {
		let (fake, base) = scanned(&mut conn).await;
		{
			let mut f = fake.lock().unwrap();
			f.replies.push(
				json!({"outcome": "not_affected", "ground": "frontend_only", "reason": "Browser only."})
					.to_string(),
			);
			f.decided_while_answering = Some(url.to_string());
		}

		ai::pass(&mut conn, &model(&base), 10).await.unwrap();

		let detail = item(&mut conn).await;
		assert_eq!(detail.item.status, Status::WillFix);
		assert_eq!(detail.item.decision.unwrap().via, Via::Ui);
		assert!(
			Suggestion::get(&mut conn, "GHSA-1", Source::Tamanu)
				.await
				.unwrap()
				.is_some()
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_prompt_sets_the_advisory_apart_and_quotes_what_was_reported() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (fake, base) = scanned(&mut conn).await;
		fake.lock().unwrap().replies.push(
			json!({"outcome": "will_fix", "ground": "none", "reason": "Reachable."}).to_string(),
		);

		ai::pass(&mut conn, &model(&base), 10).await.unwrap();

		let prompt = fake.lock().unwrap().prompts.remove(0);
		let (advisory, rest) = prompt.split_once("</advisory>").unwrap();
		assert!(advisory.contains("Fixed in left-pad: 2.0.0"));
		assert!(rest.contains(r#"- "left-pad" "1.0.0""#), "{rest}");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_invalid_reply_leaves_the_item_with_no_suggestion() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (fake, base) = scanned(&mut conn).await;
		fake.lock()
			.unwrap()
			.replies
			.push("I am not able to say.".to_owned());

		ai::pass(&mut conn, &model(&base), 10).await.unwrap();

		let detail = item(&mut conn).await;
		assert!(detail.item.suggestion.is_none());
		assert_eq!(detail.item.status, Status::Open);
		let _ = Suggestion::get(&mut conn, "GHSA-1", Source::Tamanu)
			.await
			.unwrap()
			.is_none();
	})
	.await;
}

#[test]
fn with_no_model_configured_nothing_is_looked_at() {
	// The test environment names no model.
	assert!(Config::from_env().is_none());
}
