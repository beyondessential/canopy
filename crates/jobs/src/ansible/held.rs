//! Forgets a secret variable's earlier value once it has been held for its
//! week, so Canopy keeps a replaced or removed secret only as long as undoing
//! the change needs it. The escrow holds it beyond that.
//!
//! Spec: `.workhorse/specs/private-server/ansible/variables.md` (id `ANSVAR`).
// spec: ANSVAR#history

use std::time::Duration;

use commons_servers::backup_secrets::BackupSecrets;
use database::{
	ansible_variables::{AnsibleVariable, HeldSecretValue, VariableScope},
	audit::{self, Action, Actor, Outcome, Record, RecordKind},
	diesel_async::{AsyncConnection, AsyncPgConnection},
};
use tokio::{
	task::{self, JoinHandle},
	time::sleep,
};
use tracing::{debug, error, info};

/// How often held values are looked at. A value is held for a week, so being
/// forgotten a little after it is due is of no consequence.
const TICK: Duration = Duration::from_secs(15 * 60);

/// What the audit log names as the actor of what this does.
const JOB: &str = "ansible-held-secrets";

/// One pass, for a test to drive without waiting out [`TICK`]. Answers how many
/// values it forgot.
pub async fn tick(db: &mut AsyncPgConnection, secrets: &BackupSecrets) -> usize {
	match forget_due(db, secrets).await {
		Ok(0) => {
			debug!("ansible-held-secrets: nothing due");
			0
		}
		Ok(forgotten) => {
			info!(forgotten, "ansible-held-secrets: forgot held values");
			forgotten
		}
		Err(e) => {
			error!("ansible-held-secrets: forgetting failed: {e}");
			0
		}
	}
}

/// Each value is its own entry, committed ahead of forgetting it in the store
/// and settled by whether its row went with it. A value the store would not
/// forget stays noted, and is tried again on the next pass.
async fn forget_due(
	db: &mut AsyncPgConnection,
	secrets: &BackupSecrets,
) -> commons_errors::Result<usize> {
	let actor = Actor::canopy(JOB);
	let mut forgotten = 0;
	for held in HeldSecretValue::due(db).await? {
		let scope = held.scope();
		let (variable, scope_record, rank) = records(db, scope, &held.name).await;
		let action = Action::AnsibleSecretValueForgotten {
			variable,
			scope: scope_record,
			rank,
			replaced_by: held.entry_id,
		};
		let pending = audit::commit_ahead(db, &actor, &action).await?;

		if let Err(failed) = secrets
			.forget_secret_key(&scope.held_secret_name(), &held.entry_id.to_string())
			.await
		{
			error!(
				entry = %held.entry_id,
				"ansible-held-secrets: the secret store did not forget a value: {failed:?}"
			);
			audit::settle(db, &pending, Outcome::Failed, None).await?;
			continue;
		}

		if let Err(failed) = db
			.transaction::<_, commons_errors::AppError, _>(async |conn| {
				HeldSecretValue::release(conn, held.entry_id).await?;
				audit::settle(conn, &pending, Outcome::Done, None).await
			})
			.await
		{
			// The store has forgotten it, so the row noting it is released on
			// the next pass, which finds nothing more to forget.
			error!(
				entry = %held.entry_id,
				"ansible-held-secrets: the row was not released after the store forgot a value: {failed:?}"
			);
			audit::settle(db, &pending, Outcome::Failed, None).await?;
			continue;
		}
		forgotten += 1;
	}
	Ok(forgotten)
}

/// The records an entry names the variable by. Its scope may have gone since
/// the value began to be held, which is no reason to keep the value, so it is
/// then named by its identifier alone.
async fn records(
	db: &mut AsyncPgConnection,
	scope: VariableScope,
	name: &str,
) -> (
	Record,
	Record,
	Option<commons_types::server::rank::ServerRank>,
) {
	if let Ok(records) = AnsibleVariable::records(db, scope, name).await {
		return records;
	}
	let (kind, id, rank) = match scope {
		VariableScope::Group { group_id } => (RecordKind::Group, group_id, None),
		VariableScope::Environment { group_id, rank } => (RecordKind::Group, group_id, Some(rank)),
		VariableScope::Machine { machine_id } => (RecordKind::Machine, machine_id, None),
	};
	(
		Record::new(RecordKind::Variable, scope.variable_record_id(name), name),
		Record::new(kind, id, id.to_string()),
		rank,
	)
}

pub fn spawn(secrets: BackupSecrets) -> JoinHandle<()> {
	let pool = database::init();
	task::spawn(async move {
		loop {
			sleep(TICK).await;
			match pool.get().await {
				Ok(mut db) => {
					tick(&mut db, &secrets).await;
				}
				Err(e) => error!("ansible-held-secrets: no database connection: {e}"),
			}
		}
	})
}

#[cfg(test)]
mod tests {
	use commons_servers::backup_secrets::{ExposeSecret, SecretString};
	use commons_tests::diesel_async::SimpleAsyncConnection;
	use commons_types::server::rank::ServerRank;
	use database::audit::{Filter, Status};
	use uuid::Uuid;

	use super::*;

	/// A value is forgotten once its week has passed, and one not yet due is kept.
	/// A value whose scope has gone since it began to be held is still forgotten,
	/// and its entry names the scope by identifier alone, with the rank for an
	/// environment.
	// spec: ANSVAR#history
	#[tokio::test(flavor = "multi_thread")]
	async fn forgets_a_value_once_its_week_has_passed() {
		fn named_as(scope: VariableScope) -> (RecordKind, Uuid, Option<ServerRank>) {
			match scope {
				VariableScope::Group { group_id } => (RecordKind::Group, group_id, None),
				VariableScope::Environment { group_id, rank } => {
					(RecordKind::Group, group_id, Some(rank))
				}
				VariableScope::Machine { machine_id } => (RecordKind::Machine, machine_id, None),
			}
		}

		commons_tests::db::TestDb::run(async |mut conn, _| {
			let group = Uuid::new_v4();
			conn.batch_execute(&format!(
				"INSERT INTO server_groups (id, name) VALUES ('{group}', 'kamaka');"
			))
			.await
			.unwrap();
			let scope = VariableScope::Environment {
				group_id: group,
				rank: ServerRank::Production,
			};
			let gone = [
				VariableScope::Group {
					group_id: Uuid::new_v4(),
				},
				VariableScope::Environment {
					group_id: Uuid::new_v4(),
					rank: ServerRank::Demo,
				},
				VariableScope::Machine {
					machine_id: Uuid::new_v4(),
				},
			];
			let secrets = BackupSecrets::memory();
			let (due, kept) = (Uuid::new_v4(), Uuid::new_v4());
			let mut orphans = Vec::new();
			let held = [(due, scope), (kept, scope)]
				.into_iter()
				.chain(gone.into_iter().map(|scope| (Uuid::new_v4(), scope)));
			for (entry, scope) in held {
				secrets
					.put_secret_key(
						&scope.held_secret_name(),
						&entry.to_string(),
						&SecretString::from("\"pepper\"".to_owned()),
					)
					.await
					.unwrap();
				HeldSecretValue::hold(&mut conn, entry, scope, "salt")
					.await
					.unwrap();
				if entry != due && entry != kept {
					orphans.push((scope, entry));
				}
			}
			conn.batch_execute(&format!(
				"UPDATE ansible_secret_values_held SET held_until = now() - interval '1 minute'
				 WHERE entry_id <> '{kept}'"
			))
			.await
			.unwrap();

			assert_eq!(
				tick(&mut conn, &secrets).await,
				1 + orphans.len(),
				"the due value and the three whose scopes have gone are forgotten"
			);

			let left = secrets
				.try_read_secret_keys(&scope.held_secret_name())
				.await
				.unwrap()
				.unwrap_or_default();
			assert!(
				!left.contains_key(&due.to_string()),
				"the due value is forgotten from the store"
			);
			assert_eq!(
				left[&kept.to_string()].expose_secret(),
				"\"pepper\"",
				"the value not yet due is kept in the store"
			);
			assert!(
				HeldSecretValue::get(&mut conn, kept)
					.await
					.unwrap()
					.is_some(),
				"the value not yet due keeps its row"
			);
			assert_eq!(
				HeldSecretValue::list_all(&mut conn).await.unwrap().len(),
				1,
				"only the value not yet due is still held"
			);

			let logged = audit::find(
				&mut conn,
				&Filter {
					action: Some("ansible_secret_value_forgotten".into()),
					limit: 10,
					..Default::default()
				},
			)
			.await
			.unwrap();
			assert_eq!(
				logged.len(),
				1 + orphans.len(),
				"one entry for each value forgotten, none for the one kept"
			);
			assert!(
				!logged
					.iter()
					.any(|logged| logged.detail["replaced_by"] == kept.to_string()),
				"no entry for the value not yet due"
			);
			let entry_for = logged
				.iter()
				.find(|logged| logged.detail["replaced_by"] == due.to_string())
				.expect("an entry for the due value");
			assert_eq!(entry_for.status, Status::Done, "the due value's entry");

			for (scope, entry) in orphans {
				let (kind, id, rank) = named_as(scope);
				let logged = logged
					.iter()
					.find(|logged| logged.detail["replaced_by"] == entry.to_string())
					.unwrap_or_else(|| panic!("no entry for {scope:?}"));
				assert_eq!(logged.status, Status::Done, "{scope:?}");
				assert_eq!(
					logged.records,
					vec![
						Record::new(
							RecordKind::Variable,
							scope.variable_record_id("salt"),
							"salt"
						),
						Record::new(kind, id, id.to_string()),
					],
					"{scope:?}"
				);
				assert_eq!(
					logged.detail["rank"],
					serde_json::to_value(rank).unwrap(),
					"{scope:?}"
				);

				let left = secrets
					.try_read_secret_keys(&scope.held_secret_name())
					.await
					.unwrap()
					.unwrap_or_default();
				assert!(
					!left.contains_key(&entry.to_string()),
					"{scope:?}: forgotten from the store"
				);
			}

			assert_eq!(tick(&mut conn, &secrets).await, 0, "nothing more is due");
		})
		.await
	}
}
