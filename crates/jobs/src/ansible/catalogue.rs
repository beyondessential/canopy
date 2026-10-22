//! Reads the Ansible variable catalogue infra publishes, keeps the last copy
//! that read as one, and holds what Canopy stores to it.
//!
//! Spec: `.workhorse/specs/private-server/ansible/variables.md` (id `ANSVAR`).
//!
//! The catalogue is read from infra's main branch, at the commit that is its
//! head when asked, so the copy and the commit it is recorded against cannot
//! disagree. A read that fails, or publishes something that does not read as a
//! catalogue, leaves the copy Canopy holds as it was and says why; the
//! self-alert (see [`database::self_alerts`]) is what makes that lasting
//! visible.
// spec: ANSVAR#catalogue

use std::time::Duration;

use commons_servers::backup_secrets::{BackupSecrets, SecretString};
use database::{
	ansible_catalogue::{Catalogue, CatalogueState, Problem},
	ansible_variables::AnsibleVariable,
	audit::{self, Action, Actor, Outcome},
	diesel_async::{AsyncConnection, AsyncPgConnection},
	self_alerts,
};
use jiff::Timestamp;
use serde_json::Value;
use tokio::{
	task::{self, JoinHandle},
	time::sleep,
};
use tracing::{debug, error, info, warn};

/// How often the catalogue is read.
const TICK: Duration = Duration::from_secs(3600);

/// What the audit log names as the actor of what this does.
const JOB: &str = "ansible-catalogue";

/// Where the catalogue is published. Only a test points this anywhere but
/// infra's public repository, which is read with no credentials.
#[derive(Clone, Debug)]
pub struct Source {
	/// GitHub's API, which names the commit the file is read at.
	pub api_base: String,
	/// Where raw file contents are served.
	pub raw_base: String,
	pub repo: String,
	pub branch: String,
	pub path: String,
}

impl Default for Source {
	fn default() -> Self {
		Self {
			api_base: "https://api.github.com".into(),
			raw_base: "https://raw.githubusercontent.com".into(),
			repo: "beyondessential/infra".into(),
			branch: "main".into(),
			path: "ansible/variables.schema.json".into(),
		}
	}
}

/// Why a read did not produce a document.
#[derive(Debug)]
pub enum Failure {
	Unreachable(String),
	Invalid(String),
}

/// Read the published document, and the commit it was read at.
pub async fn fetch(client: &reqwest::Client, source: &Source) -> Result<(Value, String), Failure> {
	let commits_url = reqwest::Url::parse_with_params(
		&format!("{}/repos/{}/commits", source.api_base, source.repo),
		&[
			("sha", source.branch.as_str()),
			("path", source.path.as_str()),
			("per_page", "1"),
		],
	)
	.map_err(|e| Failure::Unreachable(format!("the repository's address is unusable: {e}")))?;
	let response = client
		.get(commits_url)
		.header("accept", "application/vnd.github+json")
		.send()
		.await
		.map_err(|e| Failure::Unreachable(format!("asking for the head commit: {e}")))?;
	if !response.status().is_success() {
		return Err(Failure::Unreachable(format!(
			"asking for the head commit: {}",
			response.status()
		)));
	}
	let commits: Value = response
		.json()
		.await
		.map_err(|e| Failure::Unreachable(format!("reading the head commit: {e}")))?;
	let commit = commits
		.get(0)
		.and_then(|commit| commit.get("sha"))
		.and_then(Value::as_str)
		.ok_or_else(|| {
			Failure::Invalid(format!(
				"{} has no {} on {}",
				source.repo, source.path, source.branch
			))
		})?
		.to_owned();

	let file_url = format!(
		"{}/{}/{commit}/{}",
		source.raw_base, source.repo, source.path
	);
	let response = client
		.get(&file_url)
		.send()
		.await
		.map_err(|e| Failure::Unreachable(format!("reading the catalogue: {e}")))?;
	if response.status() == reqwest::StatusCode::NOT_FOUND {
		return Err(Failure::Invalid(format!(
			"{} is not in {} at {commit}",
			source.path, source.repo
		)));
	}
	if !response.status().is_success() {
		return Err(Failure::Unreachable(format!(
			"reading the catalogue: {}",
			response.status()
		)));
	}
	let text = response
		.text()
		.await
		.map_err(|e| Failure::Unreachable(format!("reading the catalogue: {e}")))?;
	let document = serde_json::from_str(&text)
		.map_err(|e| Failure::Invalid(format!("the file is not JSON: {e}")))?;
	Ok((document, commit))
}

/// One pass: read, keep or note, hold what is stored to what was read, and
/// raise or clear the self-alert.
pub async fn refresh(
	db: &mut AsyncPgConnection,
	secrets: &BackupSecrets,
	client: &reqwest::Client,
	source: &Source,
	now: Timestamp,
) {
	match fetch(client, source).await {
		Ok((document, commit)) => match Catalogue::parse(&document) {
			Ok(catalogue) => {
				if let Err(e) = keep(db, &document, &commit, now).await {
					error!("ansible-catalogue: keeping what was read failed: {e}");
				}
				let digest = database::ansible_catalogue::digest(&document);
				if let Err(e) = make_secret(db, secrets, &catalogue, &digest).await {
					error!("ansible-catalogue: moving values into the secret store failed: {e}");
				}
			}
			Err(not) => note(db, Problem::Invalid, &not.to_string(), now).await,
		},
		Err(Failure::Unreachable(why)) => note(db, Problem::Unreachable, &why, now).await,
		Err(Failure::Invalid(why)) => note(db, Problem::Invalid, &why, now).await,
	}

	if let Err(e) = self_alerts::sweep_ansible_catalogue(db, now).await {
		error!("ansible-catalogue: the self-alert could not be updated: {e}");
	}
}

/// Keep a copy that read, auditing a copy that is not the one held.
async fn keep(
	db: &mut AsyncPgConnection,
	document: &Value,
	commit: &str,
	now: Timestamp,
) -> commons_errors::Result<()> {
	db.transaction::<_, commons_errors::AppError, _>(async |conn| {
		let changed = CatalogueState::record_read(conn, document, Some(commit), now).await?;
		if changed {
			audit::record(
				conn,
				&Actor::canopy(JOB),
				&Action::AnsibleCatalogueChanged {
					commit: Some(commit.to_owned()),
					digest: database::ansible_catalogue::digest(document),
				},
			)
			.await?;
			info!(commit, "ansible-catalogue: read a new catalogue");
		} else {
			debug!(commit, "ansible-catalogue: the catalogue is as it was");
		}
		Ok(())
	})
	.await
}

async fn note(db: &mut AsyncPgConnection, problem: Problem, detail: &str, now: Timestamp) {
	warn!(
		?problem,
		detail, "ansible-catalogue: did not read a catalogue"
	);
	if let Err(e) = CatalogueState::record_problem(db, problem, detail, now).await {
		error!("ansible-catalogue: noting the problem failed: {e}");
	}
}

/// Move into the secret store every plain value the catalogue calls secret, so
/// a secret is never held beside its name.
///
/// Each value is its own entry, committed ahead of the write to the store and
/// settled by whether the row was cleared. A value the store would not take is
/// left plain, and tried again the next time the catalogue is read.
pub async fn make_secret(
	db: &mut AsyncPgConnection,
	secrets: &BackupSecrets,
	catalogue: &Catalogue,
	digest: &str,
) -> commons_errors::Result<usize> {
	let actor = Actor::canopy(JOB);

	let mut moved = 0;
	let names: Vec<String> = catalogue
		.variables()
		.filter(|variable| variable.secret)
		.map(|variable| variable.name.clone())
		.collect();
	for variable in AnsibleVariable::list_plain_named(db, &names).await? {
		let Some(value) = variable.value.as_ref() else {
			continue;
		};

		let scope = variable.scope();
		let (record, scope_record, rank) =
			AnsibleVariable::records(db, scope, &variable.name).await?;
		let action = Action::AnsibleVariableMadeSecret {
			variable: record,
			scope: scope_record,
			rank,
			digest: digest.to_owned(),
		};
		let pending = audit::commit_ahead(db, &actor, &action).await?;

		let stored = SecretString::from(value.to_string());
		if let Err(failed) = secrets
			.put_secret_key(&scope.secret_name(), &variable.name, &stored)
			.await
		{
			error!(
				name = %variable.name,
				"ansible-catalogue: the secret store did not take a value: {failed:?}"
			);
			audit::settle(db, &pending, Outcome::Failed, None).await?;
			continue;
		}

		let name = variable.name.clone();
		let set_by = variable.set_by.clone();
		// The row still holding the value is the move not having happened, so
		// the entry is settled failed, and the value the store took is written
		// over when the move is tried again.
		if let Err(failed) = db
			.transaction::<_, commons_errors::AppError, _>(async |conn| {
				AnsibleVariable::set(conn, scope, &name, None, set_by.as_deref()).await?;
				audit::settle(conn, &pending, Outcome::Done, None).await
			})
			.await
		{
			error!(
				name = %variable.name,
				"ansible-catalogue: the row was not cleared after the store took a value: {failed:?}"
			);
			audit::settle(db, &pending, Outcome::Failed, None).await?;
			continue;
		}
		moved += 1;
	}
	Ok(moved)
}

pub fn spawn(secrets: BackupSecrets) -> JoinHandle<()> {
	let pool = database::init();
	task::spawn(async move {
		let client = match reqwest::Client::builder()
			.user_agent("canopy-ansible-catalogue")
			.timeout(Duration::from_secs(30))
			.build()
		{
			Ok(client) => client,
			Err(e) => {
				error!("ansible-catalogue: could not build an HTTP client: {e}");
				return;
			}
		};
		let source = Source::default();
		loop {
			match pool.get().await {
				Ok(mut db) => refresh(&mut db, &secrets, &client, &source, Timestamp::now()).await,
				Err(e) => error!("ansible-catalogue: no database connection: {e}"),
			}
			sleep(TICK).await;
		}
	})
}

#[cfg(test)]
mod tests {
	use std::{
		collections::BTreeMap,
		sync::{Arc, Mutex},
	};

	use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
	use commons_tests::diesel_async::SimpleAsyncConnection;
	use database::{
		ansible_variables::VariableScope,
		audit::{Filter, Status},
		self_alerts::{ANSIBLE_CATALOGUE_REF, current},
	};
	use serde_json::json;
	use tokio::net::TcpListener;

	use super::*;

	const COMMIT: &str = "4a5b6c7d8e9f";

	/// What infra's repository answers, which a test changes between passes.
	#[derive(Clone)]
	struct Stub {
		commits: Arc<Mutex<(StatusCode, Value)>>,
		file: Arc<Mutex<(StatusCode, String)>>,
	}

	impl Stub {
		fn serving(document: &Value) -> Self {
			Self {
				commits: Arc::new(Mutex::new((StatusCode::OK, json!([{ "sha": COMMIT }])))),
				file: Arc::new(Mutex::new((StatusCode::OK, document.to_string()))),
			}
		}

		fn publish(&self, body: &str) {
			*self.file.lock().unwrap() = (StatusCode::OK, body.to_owned());
		}

		fn break_down(&self) {
			*self.commits.lock().unwrap() = (StatusCode::BAD_GATEWAY, json!({}));
		}

		/// Start answering as GitHub would, and say where.
		async fn start(&self) -> Source {
			async fn commits(State(stub): State<Stub>) -> (StatusCode, Json<Value>) {
				let (status, body) = stub.commits.lock().unwrap().clone();
				(status, Json(body))
			}
			async fn file(State(stub): State<Stub>) -> (StatusCode, String) {
				stub.file.lock().unwrap().clone()
			}

			let app = Router::new()
				.route("/repos/beyondessential/infra/commits", get(commits))
				.route(
					&format!("/beyondessential/infra/{COMMIT}/ansible/variables.schema.json"),
					get(file),
				)
				.with_state(self.clone());
			let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
			let base = format!("http://{}", listener.local_addr().unwrap());
			tokio::spawn(async move { axum::serve(listener, app).await });
			Source {
				api_base: base.clone(),
				raw_base: base,
				..Source::default()
			}
		}
	}

	fn catalogue_document() -> Value {
		json!({
			"$schema": "https://json-schema.org/draft/2020-12/schema",
			"type": "object",
			"properties": {
				"salt": { "type": "string", "x-canopy": { "secret": true } },
				"timezone": { "type": "string" }
			},
			"x-canopy": { "playbooks": {
				"base-install.yml": { "intent": "configure", "tags": [] }
			} }
		})
	}

	async fn audit_actions(db: &mut AsyncPgConnection) -> Vec<(String, Actor, Status)> {
		let mut found = audit::find(
			db,
			&Filter {
				include_routine: true,
				limit: 100,
				..Filter::default()
			},
		)
		.await
		.unwrap();
		found.reverse();
		found
			.into_iter()
			.map(|entry| (entry.action, entry.actor, entry.status))
			.collect()
	}

	async fn alert_active(db: &mut AsyncPgConnection) -> bool {
		current(db, ANSIBLE_CATALOGUE_REF)
			.await
			.unwrap()
			.is_some_and(|issue| issue.active)
	}

	/// Reading a catalogue keeps it and says so in the log, once.
	// spec: ANSVAR#catalogue
	#[tokio::test(flavor = "multi_thread")]
	async fn a_catalogue_that_reads_is_kept_and_audited_once() {
		commons_tests::db::TestDb::run(async |mut conn, _| {
			let stub = Stub::serving(&catalogue_document());
			let source = stub.start().await;
			let (client, secrets) = (reqwest::Client::new(), BackupSecrets::memory());

			refresh(&mut conn, &secrets, &client, &source, Timestamp::now()).await;
			refresh(&mut conn, &secrets, &client, &source, Timestamp::now()).await;

			let state = CatalogueState::get(&mut conn).await.unwrap().unwrap();
			assert_eq!(state.document, Some(catalogue_document()));
			assert_eq!(state.source_commit.as_deref(), Some(COMMIT));
			assert_eq!(state.problem, None);

			let log = audit_actions(&mut conn).await;
			assert_eq!(log.len(), 1, "{log:?}");
			assert_eq!(log[0].0, "ansible_catalogue_changed");
			assert_eq!(log[0].1, Actor::canopy(JOB));
			assert!(!alert_active(&mut conn).await);
		})
		.await
	}

	/// Publishing something that is not a catalogue leaves the last copy in
	/// use and warns at once.
	// spec: ANSVAR#catalogue
	#[tokio::test(flavor = "multi_thread")]
	async fn a_publication_that_does_not_read_leaves_the_last_copy_and_warns() {
		commons_tests::db::TestDb::run(async |mut conn, _| {
			let stub = Stub::serving(&catalogue_document());
			let source = stub.start().await;
			let (client, secrets) = (reqwest::Client::new(), BackupSecrets::memory());
			refresh(&mut conn, &secrets, &client, &source, Timestamp::now()).await;

			stub.publish("{ not json");
			refresh(&mut conn, &secrets, &client, &source, Timestamp::now()).await;
			let state = CatalogueState::get(&mut conn).await.unwrap().unwrap();
			assert_eq!(state.problem, Some(Problem::Invalid));
			assert_eq!(state.document, Some(catalogue_document()));
			assert!(alert_active(&mut conn).await);

			stub.publish(
				&json!({ "properties": { "a": { "x-canopy": { "scope": ["planet"] } } } })
					.to_string(),
			);
			refresh(&mut conn, &secrets, &client, &source, Timestamp::now()).await;
			let state = CatalogueState::get(&mut conn).await.unwrap().unwrap();
			assert_eq!(state.problem, Some(Problem::Invalid));
			assert!(state.problem_detail.unwrap().contains("planet"));

			stub.publish(&catalogue_document().to_string());
			refresh(&mut conn, &secrets, &client, &source, Timestamp::now()).await;
			assert!(!alert_active(&mut conn).await, "a good read clears it");
		})
		.await
	}

	/// An unreachable repository leaves the copy, and is not yet a fault.
	// spec: ANSVAR#catalogue
	#[tokio::test(flavor = "multi_thread")]
	async fn an_unreachable_repository_leaves_the_last_copy_without_an_alert_yet() {
		commons_tests::db::TestDb::run(async |mut conn, _| {
			let stub = Stub::serving(&catalogue_document());
			let source = stub.start().await;
			let (client, secrets) = (reqwest::Client::new(), BackupSecrets::memory());
			refresh(&mut conn, &secrets, &client, &source, Timestamp::now()).await;

			stub.break_down();
			refresh(&mut conn, &secrets, &client, &source, Timestamp::now()).await;

			let state = CatalogueState::get(&mut conn).await.unwrap().unwrap();
			assert_eq!(state.problem, Some(Problem::Unreachable));
			assert_eq!(state.document, Some(catalogue_document()));
			assert!(!alert_active(&mut conn).await);
		})
		.await
	}

	/// A plain value the catalogue comes to call secret moves into the secret
	/// store, one entry for it, and the row no longer holds it.
	// spec: ANSVAR#catalogue
	#[tokio::test(flavor = "multi_thread")]
	async fn a_value_the_catalogue_calls_secret_moves_into_the_secret_store() {
		commons_tests::db::TestDb::run(async |mut conn, _| {
			let group = uuid::Uuid::new_v4();
			conn.batch_execute(&format!(
				"INSERT INTO server_groups (id, name) VALUES ('{group}', 'kamaka');
				 INSERT INTO ansible_variables (server_group_id, rank, name, value, is_secret, set_by)
				 VALUES ('{group}', 'production', 'salt', '\"pepper\"', FALSE, 'someone'),
				        ('{group}', 'production', 'timezone', '\"Pacific/Fiji\"', FALSE, 'someone')"
			))
			.await
			.unwrap();

			let stub = Stub::serving(&catalogue_document());
			let source = stub.start().await;
			let (client, secrets) = (reqwest::Client::new(), BackupSecrets::memory());
			refresh(&mut conn, &secrets, &client, &source, Timestamp::now()).await;

			let scope = VariableScope::Environment {
				group_id: group,
				rank: commons_types::server::rank::ServerRank::Production,
			};
			let salt = AnsibleVariable::at(&mut conn, scope, "salt")
				.await
				.unwrap()
				.unwrap();
			assert!(salt.is_secret);
			assert!(salt.value.is_none());
			assert_eq!(salt.set_by.as_deref(), Some("someone"));
			let timezone = AnsibleVariable::at(&mut conn, scope, "timezone")
				.await
				.unwrap()
				.unwrap();
			assert!(
				!timezone.is_secret,
				"what the catalogue does not call secret stays"
			);

			let held: BTreeMap<String, _> = secrets
				.try_read_secret_keys(&scope.secret_name())
				.await
				.unwrap()
				.unwrap();
			use commons_servers::backup_secrets::ExposeSecret;
			assert_eq!(held["salt"].expose_secret(), "\"pepper\"");

			let log = audit_actions(&mut conn).await;
			let moved: Vec<_> = log
				.iter()
				.filter(|(action, ..)| action == "ansible_variable_made_secret")
				.collect();
			assert_eq!(moved.len(), 1, "{log:?}");
			assert_eq!(moved[0].1, Actor::canopy(JOB));
			assert_eq!(moved[0].2, Status::Done);

			refresh(&mut conn, &secrets, &client, &source, Timestamp::now()).await;
			let again = audit_actions(&mut conn).await;
			assert_eq!(
				again
					.iter()
					.filter(|(action, ..)| action == "ansible_variable_made_secret")
					.count(),
				1,
				"nothing is left to move the next time"
			);
		})
		.await
	}
}
