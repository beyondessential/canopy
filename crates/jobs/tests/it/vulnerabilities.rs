//! The repository reads, the scan and the model's first look, against
//! stand-ins for GitHub, OSV and the model provider.
//!
//! spec: VUL, VTR

use std::{
	collections::{BTreeMap, HashMap},
	sync::{Arc, Mutex},
};

use axum::{
	Json, Router,
	extract::{Path, State},
	http::{StatusCode, header},
	response::IntoResponse,
	routing::{get, post},
};
use commons_types::server::rank::ServerRank;
use database::{
	audit::{self, Actor, Filter},
	machines::{Machine, NewMachine},
	packages::{MachinePackageReport, PackageReport, ReportedPackage, RepositoryRead},
	self_alerts::{self, VULNERABILITY_REPOSITORIES_REF},
	vulnerabilities::{
		Basis, Decision, Entry, ItemDetail, ItemSummary, ListQuery, NewDecision, Outcome,
		Repository, Source, Status, Suggestion, Via,
	},
};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use jobs::vulnerabilities::{
	ai::{self, Config, Model},
	osv_client::OsvClient,
	repository::{Endpoints, GitHub},
	scan::Scanner,
};
use serde_json::{Value, json};

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
	/// The commit each repository's default branch is at, by GitHub slug.
	heads: HashMap<String, String>,
	/// The files of each repository at each commit, by (slug, commit).
	files: HashMap<(String, String), BTreeMap<String, String>>,
	/// Every head asked for, by slug.
	head_requests: Vec<String>,
	/// Every tree listed, as (slug, commit).
	tree_requests: Vec<(String, String)>,
	/// Every file fetched, as (commit, path).
	raw_requests: Vec<(String, String)>,
	/// What the model replies with, in order.
	replies: Vec<String>,
	/// A database a person records a decision in while the model answers.
	decided_while_answering: Option<String>,
	/// Every prompt the model was given.
	prompts: Vec<String>,
	/// What the provider answers a request for structured output with, in
	/// order, until it runs out and answers normally.
	structured_refusals: Vec<u16>,
	/// Whether each request asked for structured output.
	structured: Vec<bool>,
}

type Shared = Arc<Mutex<Fake>>;

const SLUG: &str = "beyondessential/tamanu";

const TAMANU: Repository = Repository {
	name: "tamanu",
	slug: SLUG,
};

const LOCKFILE: &str = r#"{
	"lockfileVersion": 3,
	"packages": {
		"": {},
		"packages/server": { "name": "@tamanu/server", "version": "1.0.0", "dependencies": { "left-pad": "1" } },
		"node_modules/left-pad": { "version": "1.0.0" },
		"node_modules/right-pad": { "version": "2.0.0" }
	}
}"#;

/// The same, with a second workspace package that reaches left-pad.
const TWO_WORKSPACES: &str = r#"{
	"lockfileVersion": 3,
	"packages": {
		"": {},
		"packages/server": { "name": "@tamanu/server", "version": "1.0.0", "dependencies": { "left-pad": "1" } },
		"packages/web": { "name": "@tamanu/web", "version": "1.0.0", "dependencies": { "left-pad": "1" } },
		"node_modules/left-pad": { "version": "1.0.0" }
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
) -> Result<Json<Value>, StatusCode> {
	let mut fake = fake.lock().unwrap();
	fake.fetched.push(id.clone());
	fake.advisories
		.get(&id)
		.cloned()
		.map(Json)
		.ok_or(StatusCode::NOT_FOUND)
}

fn pkt(line: &str) -> String {
	format!("{:04x}{line}", line.len() + 4)
}

/// Git's smart HTTP advertisement of a repository whose head is at `sha`.
async fn info_refs(
	State(fake): State<Shared>,
	Path((owner, repo)): Path<(String, String)>,
) -> impl IntoResponse {
	let slug = format!("{owner}/{}", repo.trim_end_matches(".git"));
	let mut fake = fake.lock().unwrap();
	fake.head_requests.push(slug.clone());
	let Some(sha) = fake.heads.get(&slug) else {
		return (StatusCode::NOT_FOUND, String::new()).into_response();
	};
	let body = format!(
		"{}0000{}{}0000",
		pkt("# service=git-upload-pack\n"),
		pkt(&format!(
			"{sha} HEAD\0multi_ack symref=HEAD:refs/heads/main agent=git/test\n"
		)),
		pkt(&format!("{sha} refs/heads/main\n")),
	);
	(
		[(
			header::CONTENT_TYPE,
			"application/x-git-upload-pack-advertisement",
		)],
		body,
	)
		.into_response()
}

async fn tree(
	State(fake): State<Shared>,
	Path((owner, repo, sha)): Path<(String, String, String)>,
) -> Result<Json<Value>, StatusCode> {
	let slug = format!("{owner}/{repo}");
	let mut fake = fake.lock().unwrap();
	fake.tree_requests.push((slug.clone(), sha.clone()));
	let files = fake.files.get(&(slug, sha)).ok_or(StatusCode::NOT_FOUND)?;
	let tree: Vec<Value> = files
		.keys()
		.map(|path| json!({ "path": path, "type": "blob", "sha": "0" }))
		.collect();
	Ok(Json(json!({ "tree": tree, "truncated": false })))
}

async fn raw(
	State(fake): State<Shared>,
	Path((owner, repo, sha, path)): Path<(String, String, String, String)>,
) -> Result<String, StatusCode> {
	let slug = format!("{owner}/{repo}");
	let mut fake = fake.lock().unwrap();
	fake.raw_requests.push((sha.clone(), path.clone()));
	fake.files
		.get(&(slug, sha))
		.and_then(|files| files.get(&path))
		.cloned()
		.ok_or(StatusCode::NOT_FOUND)
}

async fn completion(
	State(fake): State<Shared>,
	Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
	let (reply, decided_in) = {
		let mut fake = fake.lock().unwrap();
		if let Some(prompt) = body["messages"][1]["content"].as_str() {
			fake.prompts.push(prompt.to_owned());
		}
		let structured = body.get("response_format").is_some();
		fake.structured.push(structured);
		if structured && !fake.structured_refusals.is_empty() {
			let status = fake.structured_refusals.remove(0);
			return Err(StatusCode::from_u16(status).unwrap());
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
				source: tamanu(),
				entries: vec![whole(Outcome::WillFix, None)],
				actor: Actor::operator("admin@example.com", None),
				via: Via::Ui,
			},
		)
		.await
		.unwrap();
	}
	Ok(Json(
		json!({ "choices": [{ "message": { "content": reply } }] }),
	))
}

async fn serve(fake: Shared) -> String {
	let router = Router::new()
		.route("/v1/querybatch", post(query_batch))
		.route("/v1/vulns/{id}", get(vuln))
		.route("/git/{owner}/{repo}/info/refs", get(info_refs))
		.route("/api/repos/{owner}/{repo}/git/trees/{sha}", get(tree))
		.route("/raw/{owner}/{repo}/{sha}/{*path}", get(raw))
		.route("/chat/completions", post(completion))
		.with_state(fake);
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let addr = listener.local_addr().unwrap();
	tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
	format!("http://{addr}")
}

fn github(base: &str) -> GitHub {
	GitHub::new(Endpoints {
		git: format!("{base}/git"),
		api: format!("{base}/api"),
		raw: format!("{base}/raw"),
	})
}

fn scanner(base: &str) -> Scanner {
	Scanner::new(OsvClient::new(base), github(base)).with_repositories(vec![TAMANU])
}

/// Make `commit` the head of Tamanu's default branch, with these files.
fn publish(fake: &Shared, commit: &str, files: &[(&str, &str)]) {
	let mut fake = fake.lock().unwrap();
	fake.heads.insert(SLUG.to_owned(), commit.to_owned());
	fake.files.insert(
		(SLUG.to_owned(), commit.to_owned()),
		files
			.iter()
			.map(|(path, text)| ((*path).to_owned(), (*text).to_owned()))
			.collect(),
	);
}

fn tamanu() -> Source {
	Source::Repository("tamanu".into())
}

fn whole(outcome: Outcome, reason: Option<&str>) -> Entry {
	Entry {
		workspace_packages: None,
		outcome,
		reason: reason.map(str::to_owned),
		issue_url: None,
	}
}

fn operator() -> Actor {
	Actor::operator("admin@example.com", None)
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

async fn audited(conn: &mut AsyncPgConnection, action: &str) -> Vec<audit::Entry> {
	audit::find(
		conn,
		&Filter {
			action: Some(action.to_owned()),
			limit: 10,
			..Default::default()
		},
	)
	.await
	.unwrap()
}

/// A scan of Tamanu with left-pad's one advisory, GHSA-1.
async fn scanned(conn: &mut AsyncPgConnection) -> (Shared, String) {
	scanned_with(conn, LOCKFILE).await
}

async fn scanned_with(conn: &mut AsyncPgConnection, lockfile: &str) -> (Shared, String) {
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
	publish(&fake, "aaaa", &[("package-lock.json", lockfile)]);
	let base = serve(fake.clone()).await;
	let scanner = scanner(&base);
	scanner.read_repositories(conn).await.unwrap();
	scanner.scan(conn).await.unwrap();
	(fake, base)
}

async fn item(conn: &mut AsyncPgConnection) -> ItemDetail {
	ItemDetail::load(conn, "GHSA-1", &tamanu()).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_repository_is_read_when_its_head_moves_and_not_otherwise() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let fake: Shared = Arc::default();
		publish(&fake, "aaaa", &[("package-lock.json", LOCKFILE)]);
		let base = serve(fake.clone()).await;
		let scanner = scanner(&base);

		let first = scanner.read_repositories(&mut conn).await.unwrap();
		assert_eq!((first.read, first.unchanged, first.failed), (1, 0, 0));
		let read = RepositoryRead::get(&mut conn, "tamanu")
			.await
			.unwrap()
			.unwrap();
		assert_eq!(read.commit_sha.as_deref(), Some("aaaa"));
		assert_eq!(read.failure, None);
		{
			let f = fake.lock().unwrap();
			assert_eq!(f.tree_requests.len(), 1);
			assert_eq!(
				f.raw_requests,
				[("aaaa".to_owned(), "package-lock.json".to_owned())]
			);
		}

		let again = scanner.read_repositories(&mut conn).await.unwrap();
		assert_eq!((again.read, again.unchanged), (0, 1));
		{
			let f = fake.lock().unwrap();
			assert_eq!(f.head_requests.len(), 2, "the head is checked each time");
			assert_eq!(f.tree_requests.len(), 1, "but the tree only when it moved");
			assert_eq!(f.raw_requests.len(), 1);
		}

		publish(&fake, "bbbb", &[("package-lock.json", TWO_WORKSPACES)]);
		let moved = scanner.read_repositories(&mut conn).await.unwrap();
		assert_eq!(moved.read, 1);
		assert_eq!(fake.lock().unwrap().tree_requests.len(), 2);
		let read = RepositoryRead::get(&mut conn, "tamanu")
			.await
			.unwrap()
			.unwrap();
		assert_eq!(read.commit_sha.as_deref(), Some("bbbb"));
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_scan_asks_about_each_package_version_once_and_each_advisory_until_it_is_revised() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
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
		publish(&fake, "aaaa", &[("package-lock.json", LOCKFILE)]);
		let base = serve(fake.clone()).await;
		let scanner = scanner(&base);
		scanner.read_repositories(&mut conn).await.unwrap();

		let report = scanner.scan(&mut conn).await.unwrap();
		assert_eq!(report.package_versions_queried, 2, "left-pad and right-pad");
		assert_eq!(
			report.advisories_fetched, 2,
			"results past a page are followed"
		);
		assert!(
			fake.lock()
				.unwrap()
				.queries
				.iter()
				.any(|(_, _, token)| token.is_some()),
			"the second page was asked for"
		);

		let listing = ItemSummary::list(&mut conn, &ListQuery::default())
			.await
			.unwrap();
		let ids: Vec<&str> = listing.items.iter().map(|i| i.id.as_str()).collect();
		assert_eq!(ids, ["GHSA-1", "GHSA-2"], "by severity");
		assert_eq!(listing.items[0].source, tamanu());
		assert_eq!(listing.items[0].workspace_packages, ["@tamanu/server"]);

		// Nothing changed upstream: no record is fetched again.
		let again = scanner.scan(&mut conn).await.unwrap();
		assert_eq!(again.advisories_fetched, 0);

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
		let after = ItemSummary::list(&mut conn, &ListQuery::default())
			.await
			.unwrap();
		assert_eq!(after.items[0].severity, database::osv::Severity::Critical);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_scan_lapses_a_decided_item_when_severity_rises_and_audits_it_as_canopys() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (fake, base) = scanned(&mut conn).await;
		Decision::record(
			&mut conn,
			NewDecision {
				vulnerability_id: "GHSA-1".into(),
				source: tamanu(),
				entries: vec![whole(Outcome::NotAffected, Some("never reached"))],
				actor: operator(),
				via: Via::Ui,
			},
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
		let report = scanner(&base).scan(&mut conn).await.unwrap();
		assert_eq!(report.lapsed, 1);

		let detail = item(&mut conn).await;
		assert_eq!(detail.item.status, Status::Open);
		assert!(detail.item.lapse.is_some());

		let log = audited(&mut conn, "vulnerability_reopened").await;
		assert_eq!(log.len(), 1);
		assert!(matches!(&log[0].actor, Actor::Canopy { job } if job == "vulnerability-scan"));
	})
	.await;
}

const YARN_LOCKFILE: &str = r#"__metadata:
  version: 8
  cacheKey: 10c0

"app@workspace:.":
  version: 0.0.0-use.local
  resolution: "app@workspace:."
  dependencies:
    "@app/server": "workspace:*"
    prettier: "npm:^3"
  languageName: unknown
  linkType: soft

"@app/server@workspace:*, @app/server@workspace:packages/server":
  version: 0.0.0-use.local
  resolution: "@app/server@workspace:packages/server"
  dependencies:
    left-pad: "npm:^1"
  languageName: unknown
  linkType: soft

"left-pad@npm:^1":
  version: 1.0.0
  resolution: "left-pad@npm:1.0.0"
  languageName: node
  linkType: hard

"prettier@npm:^3":
  version: 3.0.0
  resolution: "prettier@npm:3.0.0"
  languageName: node
  linkType: hard
"#;

#[tokio::test(flavor = "multi_thread")]
async fn a_yarn_lockfile_is_read_with_the_manifests_of_its_workspaces() {
	let fake: Shared = Arc::default();
	let base = serve(fake.clone()).await;
	publish(
		&fake,
		"aaaa",
		&[
			("web/yarn.lock", YARN_LOCKFILE),
			(
				"web/package.json",
				r#"{"devDependencies":{"prettier":"^3"}}"#,
			),
			(
				"web/packages/server/package.json",
				r#"{"name":"@app/server","dependencies":{"left-pad":"^1"}}"#,
			),
		],
	);

	let packages = github(&base).read(&TAMANU, "aaaa").await.unwrap();
	let paths_of = |name: &str| -> Vec<(String, database::packages::Scope)> {
		let package = packages.iter().find(|p| p.name == name).unwrap();
		package
			.paths
			.iter()
			.map(|path| (path.workspace_package.clone(), path.scope))
			.collect()
	};
	use database::packages::Scope;
	assert_eq!(
		paths_of("left-pad"),
		[
			("@app/server".into(), Scope::Runtime),
			("app".into(), Scope::Runtime)
		],
		"the root reaches it through the workspace it depends on"
	);
	assert_eq!(paths_of("prettier"), [("app".into(), Scope::Development)]);
	assert!(
		fake.lock()
			.unwrap()
			.raw_requests
			.contains(&("aaaa".into(), "web/packages/server/package.json".into())),
		"the workspace's manifest was fetched from where the lockfile says it is"
	);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lockfile_too_large_to_read_is_refused_rather_than_parsed() {
	let fake: Shared = Arc::default();
	let base = serve(fake.clone()).await;
	publish(&fake, "aaaa", &[("package-lock.json", LOCKFILE)]);

	let failure = github(&base)
		.with_max_file_bytes(16)
		.read(&TAMANU, "aaaa")
		.await
		.unwrap_err();
	assert!(failure.to_string().contains("larger than"), "{failure}");

	github(&base)
		.with_max_file_bytes(LOCKFILE.len())
		.read(&TAMANU, "aaaa")
		.await
		.expect("a file at the limit is read");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_yarn_lockfile_that_cannot_be_read_is_a_failure_not_a_skip() {
	let fake: Shared = Arc::default();
	let base = serve(fake.clone()).await;

	publish(
		&fake,
		"aaaa",
		&[
			("yarn.lock", YARN_LOCKFILE),
			("package.json", r#"{"name":"app"}"#),
		],
	);
	let failure = github(&base).read(&TAMANU, "aaaa").await.unwrap_err();
	assert!(
		failure.to_string().contains("yarn.lock") && failure.to_string().contains("package.json"),
		"a workspace with no manifest names both: {failure}"
	);

	publish(
		&fake,
		"bbbb",
		&[(
			"yarn.lock",
			"# THIS IS AN AUTOGENERATED FILE. DO NOT EDIT THIS FILE DIRECTLY.\n\
			 # yarn lockfile v1\n\n\nleft-pad@^1:\n  version \"1.0.0\"\n",
		)],
	);
	let failure = github(&base).read(&TAMANU, "bbbb").await.unwrap_err();
	assert!(failure.to_string().contains("classic"), "{failure}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_repository_with_no_lockfile_canopy_can_read_is_a_failure_not_an_empty_read() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (fake, base) = scanned(&mut conn).await;
		let scanner = scanner(&base);
		assert_eq!(held(&mut conn).await, 2);

		// Nothing Canopy reads: as a repository in an ecosystem it does not
		// read yet looks, and as Tupaia looked to a build that did not read
		// Yarn lockfiles.
		publish(&fake, "bbbb", &[("Gemfile.lock", "GEM\n  specs:\n")]);
		let report = scanner.read_repositories(&mut conn).await.unwrap();
		assert_eq!((report.read, report.failed), (0, 1));

		let read = RepositoryRead::get(&mut conn, "tamanu")
			.await
			.unwrap()
			.unwrap();
		assert_eq!(
			read.commit_sha.as_deref(),
			Some("aaaa"),
			"the commit that could not be read is not recorded as read"
		);
		assert!(
			read.failure.as_deref().unwrap().contains("no lockfile"),
			"{:?}",
			read.failure
		);
		assert_eq!(
			held(&mut conn).await,
			2,
			"what was held is kept, not emptied"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rescan_reads_the_repository_again_where_it_stands() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (_fake, base) = scanned(&mut conn).await;
		let scanner = scanner(&base);

		let unmoved = scanner.read_repositories(&mut conn).await.unwrap();
		assert_eq!((unmoved.read, unmoved.unchanged), (0, 1));

		database::vulnerabilities::rescan(&mut conn, &tamanu())
			.await
			.unwrap();
		let read = RepositoryRead::get(&mut conn, "tamanu")
			.await
			.unwrap()
			.unwrap();
		assert_eq!(read.commit_sha, None, "it reads as due");
		assert!(read.read_at.is_some(), "without going stale meanwhile");

		let asked = scanner.read_repositories(&mut conn).await.unwrap();
		assert_eq!(
			(asked.read, asked.unchanged),
			(1, 0),
			"the head has not moved, but the read was asked for"
		);
		let read = RepositoryRead::get(&mut conn, "tamanu")
			.await
			.unwrap()
			.unwrap();
		assert_eq!(read.commit_sha.as_deref(), Some("aaaa"));

		let settled = scanner.read_repositories(&mut conn).await.unwrap();
		assert_eq!(
			(settled.read, settled.unchanged),
			(0, 1),
			"and once read it is left alone again"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn linux_cannot_be_rescanned_since_machines_report_it() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let refused = database::vulnerabilities::rescan(&mut conn, &Source::Linux)
			.await
			.unwrap_err();
		assert!(refused.to_string().contains("machines"), "{refused}");
	})
	.await;
}

async fn held(conn: &mut AsyncPgConnection) -> usize {
	database::vulnerabilities::counted_packages(conn)
		.await
		.unwrap()
		.len()
}

#[tokio::test(flavor = "multi_thread")]
async fn one_bad_lockfile_leaves_what_is_held_and_raises_the_self_alert_until_a_good_read() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (fake, base) = scanned(&mut conn).await;
		let scanner = scanner(&base);
		assert_eq!(held(&mut conn).await, 2);

		// The next commit has a good lockfile and a bad one.
		publish(
			&fake,
			"bbbb",
			&[
				("package-lock.json", TWO_WORKSPACES),
				("tools/package-lock.json", "{ not json"),
			],
		);
		let failed = scanner.read_repositories(&mut conn).await.unwrap();
		assert_eq!((failed.read, failed.failed), (0, 1));

		let read = RepositoryRead::get(&mut conn, "tamanu")
			.await
			.unwrap()
			.unwrap();
		assert_eq!(
			read.commit_sha.as_deref(),
			Some("aaaa"),
			"still the last good read"
		);
		assert!(
			read.failure
				.as_deref()
				.unwrap()
				.contains("tools/package-lock.json"),
			"{:?}",
			read.failure
		);
		assert_eq!(held(&mut conn).await, 2, "nothing was replaced");
		let items = ItemSummary::list(&mut conn, &ListQuery::default())
			.await
			.unwrap();
		assert_eq!(items.items.len(), 1, "and nothing closed");

		// Not stale yet: it was read a moment ago.
		assert!(
			self_alerts::current(&mut conn, VULNERABILITY_REPOSITORIES_REF)
				.await
				.unwrap()
				.is_none_or(|issue| !issue.active)
		);

		// A day on.
		diesel::sql_query(
			"UPDATE repository_reads SET read_at = now() - interval '2 days', \
			 last_attempt_at = now() - interval '1 hour'",
		)
		.execute(&mut conn)
		.await
		.unwrap();
		scanner.read_repositories(&mut conn).await.unwrap();
		let alert = self_alerts::current(&mut conn, VULNERABILITY_REPOSITORIES_REF)
			.await
			.unwrap()
			.unwrap();
		assert!(alert.active);
		assert!(alert.message.contains("tamanu"), "{}", alert.message);
		assert!(
			alert.message.contains("tools/package-lock.json"),
			"{}",
			alert.message
		);

		// Fixed.
		publish(&fake, "cccc", &[("package-lock.json", TWO_WORKSPACES)]);
		diesel::sql_query(
			"UPDATE repository_reads SET last_attempt_at = now() - interval '1 hour'",
		)
		.execute(&mut conn)
		.await
		.unwrap();
		let fixed = scanner.read_repositories(&mut conn).await.unwrap();
		assert_eq!(fixed.read, 1);
		let alert = self_alerts::current(&mut conn, VULNERABILITY_REPOSITORIES_REF)
			.await
			.unwrap()
			.unwrap();
		assert!(!alert.active);
		assert_eq!(held(&mut conn).await, 1, "the new read replaced the old");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_repository_whose_first_read_fails_is_given_a_day_before_it_is_reported() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let fake: Shared = Arc::default();
		let base = serve(fake.clone()).await;
		let scanner = scanner(&base);
		let report = scanner.read_repositories(&mut conn).await.unwrap();
		assert_eq!(report.failed, 1, "GitHub does not know the repository");
		let alert = self_alerts::current(&mut conn, VULNERABILITY_REPOSITORIES_REF)
			.await
			.unwrap();
		assert!(
			alert.is_none_or(|alert| !alert.active),
			"a first failure is not yet a day without a read"
		);

		diesel::sql_query(
			"UPDATE repository_reads SET first_attempt_at = now() - interval '2 days'",
		)
		.execute(&mut conn)
		.await
		.unwrap();
		scanner.read_repositories(&mut conn).await.unwrap();
		let alert = self_alerts::current(&mut conn, VULNERABILITY_REPOSITORIES_REF)
			.await
			.unwrap()
			.unwrap();
		assert!(alert.active);
		assert!(alert.message.contains("never read"), "{}", alert.message);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn decisions_the_model_made_are_withdrawn_on_start_and_become_suggestions() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (fake, base) = scanned(&mut conn).await;
		{
			let mut f = fake.lock().unwrap();
			f.advisories.insert(
				"GHSA-2".into(),
				advisory("GHSA-2", "2026-10-02T00:00:00Z", "LOW", "9.0.0"),
			);
			f.answers.insert(
				"left-pad".into(),
				vec![vec![
					("GHSA-1".into(), "2026-10-01T00:00:00Z".into()),
					("GHSA-2".into(), "2026-10-02T00:00:00Z".into()),
				]],
			);
		}
		let scanner = scanner(&base);
		scanner.scan(&mut conn).await.unwrap();

		Decision::record(
			&mut conn,
			NewDecision {
				vulnerability_id: "GHSA-1".into(),
				source: tamanu(),
				entries: vec![whole(
					Outcome::NotAffected,
					Some("Only the bundler loads it."),
				)],
				actor: Actor::canopy("ai-worker"),
				via: Via::Model,
			},
		)
		.await
		.unwrap();
		Decision::record(
			&mut conn,
			NewDecision {
				vulnerability_id: "GHSA-2".into(),
				source: tamanu(),
				entries: vec![whole(Outcome::AcceptedRisk, Some("A person decided."))],
				actor: operator(),
				via: Via::Ui,
			},
		)
		.await
		.unwrap();

		assert_eq!(
			scanner.withdraw_model_decisions(&mut conn).await.unwrap(),
			1
		);

		let detail = item(&mut conn).await;
		assert_eq!(detail.item.status, Status::Open);
		assert!(detail.item.withdrawn);
		let suggestion = detail.item.suggestion.unwrap();
		assert_eq!(suggestion.basis, Basis::InsufficientInformation);
		assert_eq!(suggestion.status, Status::NotAffected);
		assert_eq!(
			suggestion.entries[0].reason.as_deref(),
			Some("Only the bundler loads it.")
		);

		let other = ItemDetail::load(&mut conn, "GHSA-2", &tamanu())
			.await
			.unwrap();
		assert_eq!(
			other.item.status,
			Status::AcceptedRisk,
			"a person's decision stands"
		);

		let log = audited(&mut conn, "vulnerability_decision_withdrawn").await;
		assert_eq!(log.len(), 1);
		assert!(matches!(&log[0].actor, Actor::Canopy { job } if job == "vulnerability-migration"));
		assert_eq!(log[0].detail["via"], "canopy");

		assert_eq!(
			scanner.withdraw_model_decisions(&mut conn).await.unwrap(),
			0,
			"once done, it is done"
		);
		assert_eq!(
			audited(&mut conn, "vulnerability_decision_withdrawn")
				.await
				.len(),
			1
		);
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

fn reply(basis: &str, entries: Value) -> String {
	json!({ "basis": basis, "entries": entries }).to_string()
}

fn whole_reply(outcome: &str, reason: &str) -> String {
	reply(
		"dependency_path_and_advisory_text",
		json!([{ "workspace_packages": null, "outcome": outcome, "reason": reason }]),
	)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_model_only_suggests_and_the_item_stays_open() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (fake, base) = scanned(&mut conn).await;
		fake.lock().unwrap().replies.push(reply(
			"dependency_path_only",
			json!([{ "workspace_packages": null, "outcome": "not_affected", "reason": "Only development reaches it." }]),
		));

		assert_eq!(ai::pass(&mut conn, &model(&base), 10).await.unwrap(), 1);

		let detail = item(&mut conn).await;
		assert_eq!(detail.item.status, Status::Open);
		assert!(detail.item.decision.is_none());
		let suggestion = detail.item.suggestion.unwrap();
		assert_eq!(suggestion.status, Status::NotAffected);
		assert_eq!(suggestion.basis, Basis::DependencyPathOnly);
		assert_eq!(suggestion.model, "glm-flash");

		assert!(audited(&mut conn, "vulnerability_decided").await.is_empty());
		assert_eq!(
			ai::pass(&mut conn, &model(&base), 10).await.unwrap(),
			0,
			"an item with a suggestion is not looked at again"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_answer_with_no_basis_gives_no_suggestion() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (fake, base) = scanned(&mut conn).await;
		fake.lock().unwrap().replies.push(
			json!({"entries": [{ "workspace_packages": null, "outcome": "will_fix", "reason": "x" }]})
				.to_string(),
		);

		ai::pass(&mut conn, &model(&base), 10).await.unwrap();

		assert!(item(&mut conn).await.item.suggestion.is_none());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_answer_whose_split_breaks_the_rules_gives_no_suggestion() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (fake, base) = scanned_with(&mut conn, TWO_WORKSPACES).await;
		let model = model(&base);
		let mut ask = async |entries: Value| {
			fake.lock()
				.unwrap()
				.replies
				.push(reply("dependency_path_only", entries));
			let looked = ai::look_at(&mut conn, &model, "GHSA-1", &tamanu())
				.await
				.unwrap();
			(
				looked,
				Suggestion::get(&mut conn, "GHSA-1", &tamanu()).await.unwrap(),
			)
		};

		// Leaves @tamanu/web uncovered.
		let (looked, held) = ask(json!([
			{ "workspace_packages": ["@tamanu/server"], "outcome": "will_fix", "reason": "Server side." },
		]))
		.await;
		assert_eq!(looked, ai::Looked::NoAnswer);
		assert!(held.is_none());

		// Names a workspace package that does not reach the advisory.
		let (looked, held) = ask(json!([
			{ "workspace_packages": ["@tamanu/server", "@tamanu/web"], "outcome": "will_fix", "reason": "x" },
			{ "workspace_packages": ["@tamanu/mobile"], "outcome": "not_affected", "reason": "y" },
		]))
		.await;
		assert_eq!(looked, ai::Looked::NoAnswer);
		assert!(held.is_none());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_valid_split_suggestion_is_stored_with_its_basis() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (fake, base) = scanned_with(&mut conn, TWO_WORKSPACES).await;
		fake.lock().unwrap().replies.push(reply(
			"dependency_path_and_advisory_text",
			json!([
				{ "workspace_packages": ["@tamanu/web"], "outcome": "not_affected", "reason": "Browser only; the flaw needs a server." },
				{ "workspace_packages": null, "outcome": "will_fix", "reason": "Server side." },
			]),
		));

		ai::pass(&mut conn, &model(&base), 10).await.unwrap();

		let detail = item(&mut conn).await;
		assert_eq!(detail.item.status, Status::Open);
		let suggestion = detail.item.suggestion.unwrap();
		assert_eq!(suggestion.status, Status::Mixed);
		assert_eq!(suggestion.basis, Basis::DependencyPathAndAdvisoryText);
		assert_eq!(suggestion.entries.len(), 2);
		assert_eq!(
			suggestion.entries[0].workspace_packages.as_deref(),
			Some(["@tamanu/web".to_owned()].as_slice())
		);
		assert!(suggestion.entries[1].is_catch_all());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_person_deciding_while_the_model_answers_keeps_their_decision() {
	commons_tests::db::TestDb::run(async |mut conn, url| {
		let (fake, base) = scanned(&mut conn).await;
		{
			let mut f = fake.lock().unwrap();
			f.replies.push(whole_reply("not_affected", "Browser only."));
			f.decided_while_answering = Some(url.to_string());
		}

		ai::pass(&mut conn, &model(&base), 10).await.unwrap();

		let detail = item(&mut conn).await;
		assert_eq!(detail.item.status, Status::WillFix);
		assert_eq!(detail.item.decision.unwrap().via, Via::Ui);
		assert!(
			Suggestion::get(&mut conn, "GHSA-1", &tamanu())
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
		fake.lock()
			.unwrap()
			.replies
			.push(whole_reply("will_fix", "Reachable."));

		ai::pass(&mut conn, &model(&base), 10).await.unwrap();

		let prompt = fake.lock().unwrap().prompts.remove(0);
		let (advisory, rest) = prompt.split_once("</advisory>").unwrap();
		assert!(advisory.contains("Fixed in left-pad: 2.0.0"));
		assert!(rest.contains(r#"- "left-pad" "1.0.0""#), "{rest}");
		assert!(
			rest.contains(r#"the lowest version that fixes it is "2.0.0""#),
			"{rest}"
		);
		assert!(
			rest.contains(
				r#"workspace package "@tamanu/server": scope runtime, a direct dependency, chain ["left-pad"]"#
			),
			"{rest}"
		);
		assert!(
			rest.contains(r#"Workspace packages that reach it: ["@tamanu/server"]"#),
			"{rest}"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_linux_prompt_gives_the_package_and_advisory_and_no_machine() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let machine = Machine::create(
			&mut conn,
			NewMachine {
				rank: Some(ServerRank::Production),
				..NewMachine::named("secret-box-name")
			},
		)
		.await
		.unwrap();
		MachinePackageReport::store(
			&mut conn,
			machine.id,
			&PackageReport {
				distribution: "ubuntu".into(),
				release: "24.04".into(),
				packages: vec![ReportedPackage {
					name: "openssl".into(),
					version: "9.9.9-0ubuntu1".into(),
				}],
			},
		)
		.await
		.unwrap();

		let fake: Shared = Arc::default();
		{
			let mut f = fake.lock().unwrap();
			f.advisories.insert(
				"UBUNTU-1".into(),
				json!({
					"id": "UBUNTU-1",
					"summary": "openssl flaw",
					"details": "details",
					"modified": "2026-10-01T00:00:00Z",
					"database_specific": { "severity": "HIGH" },
					"affected": [{
						"package": { "ecosystem": "Ubuntu:24.04:LTS", "name": "openssl" },
						"ranges": [{ "type": "ECOSYSTEM", "events": [{ "introduced": "0" }, { "fixed": "9.9.10-0ubuntu1" }] }],
					}],
				}),
			);
			f.answers.insert(
				"openssl".into(),
				vec![vec![("UBUNTU-1".into(), "2026-10-01T00:00:00Z".into())]],
			);
			f.replies.push(whole_reply("will_fix", "Upgrade it."));
		}
		let base = serve(fake.clone()).await;
		scanner(&base).scan(&mut conn).await.unwrap();

		assert_eq!(ai::pass(&mut conn, &model(&base), 10).await.unwrap(), 1);

		let prompt = fake.lock().unwrap().prompts.remove(0);
		assert!(prompt.contains("UBUNTU-1"));
		assert!(prompt.contains(r#"- "openssl""#), "{prompt}");
		assert!(!prompt.contains("secret-box-name"), "{prompt}");
		assert!(!prompt.contains("9.9.9-0ubuntu1"), "{prompt}");
		assert!(!prompt.contains(&machine.id.to_string()), "{prompt}");
		let suggestion = Suggestion::get(&mut conn, "UBUNTU-1", &Source::Linux)
			.await
			.unwrap();
		assert!(suggestion.is_some());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_model_refusing_structured_output_is_asked_for_json_in_the_reply() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (fake, base) = scanned(&mut conn).await;
		{
			let mut f = fake.lock().unwrap();
			f.structured_refusals.push(400);
			f.replies.push(whole_reply("will_fix", "Reachable."));
		}

		ai::pass(&mut conn, &model(&base), 10).await.unwrap();

		assert_eq!(fake.lock().unwrap().structured, [true, false]);
		assert!(item(&mut conn).await.item.suggestion.is_some());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_model_that_refused_structured_output_is_not_asked_for_it_again() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (fake, base) = scanned(&mut conn).await;
		{
			let mut f = fake.lock().unwrap();
			f.structured_refusals.push(400);
			for _ in 0..2 {
				f.replies.push(whole_reply("will_fix", "Reachable."));
			}
		}
		let model = model(&base);

		ai::look_at(&mut conn, &model, "GHSA-1", &tamanu())
			.await
			.unwrap();
		diesel::sql_query("DELETE FROM vulnerability_suggestions")
			.execute(&mut conn)
			.await
			.unwrap();
		ai::look_at(&mut conn, &model, "GHSA-1", &tamanu())
			.await
			.unwrap();

		assert_eq!(fake.lock().unwrap().structured, [true, false, false]);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_provider_failure_is_not_retried_without_structured_output() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (fake, base) = scanned(&mut conn).await;
		fake.lock().unwrap().structured_refusals.push(503);

		ai::pass(&mut conn, &model(&base), 10).await.unwrap();

		assert_eq!(fake.lock().unwrap().structured, [true]);
		assert!(item(&mut conn).await.item.suggestion.is_none());
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
	})
	.await;
}

#[test]
fn with_no_model_configured_nothing_is_looked_at() {
	// The test environment names no model.
	assert!(Config::from_env().is_none());
}
