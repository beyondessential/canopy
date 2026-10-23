//! Keeps Ansible leases honest: releases one no run read in time, so an
//! abandoned start does not hold an environment shut, and notices one that ran
//! out, which would otherwise leave nothing in the log.
//!
//! Spec: `.workhorse/specs/private-server/ansible/runs.md` (id `ANSRUN`).
// spec: ANSRUN#starting-a-run

use std::time::Duration;

use database::{
	ansible_leases::{AnsibleLease, ReleaseReason},
	audit::{self, Action, Actor, Record, RecordKind},
	diesel_async::{AsyncConnection, AsyncPgConnection},
	server_groups::ServerGroup,
};
use jiff::Timestamp;
use tokio::{
	task::{self, JoinHandle},
	time::sleep,
};
use tracing::{debug, error, info};

/// How often leases are looked at. A lease waits five minutes unread, so this is
/// well inside it.
const TICK: Duration = Duration::from_secs(30);

/// What the audit log names as the actor of what this does.
const JOB: &str = "ansible-leases";

/// One pass, for a test to drive without waiting out [`TICK`]. Answers how many
/// leases it released unread and how many it noticed had run out.
pub async fn tick(db: &mut AsyncPgConnection, now: Timestamp) -> (usize, usize) {
	let swept = match db
		.transaction::<_, commons_errors::AppError, _>(async |conn| {
			let swept = AnsibleLease::sweep(conn, now).await?;
			let actor = Actor::canopy(JOB);
			for lease in &swept.unstarted {
				let (group, lease_record) = records(conn, lease).await?;
				audit::record(
					conn,
					&actor,
					&Action::AnsibleLeaseReleased {
						group,
						lease: lease_record,
						rank: lease.rank,
						held_by: lease.held_by.clone(),
						reason: ReleaseReason::Unstarted.to_string(),
					},
				)
				.await?;
			}
			for lease in &swept.expired {
				let (group, lease_record) = records(conn, lease).await?;
				audit::record(
					conn,
					&actor,
					&Action::AnsibleLeaseExpired {
						group,
						lease: lease_record,
						rank: lease.rank,
						held_by: lease.held_by.clone(),
					},
				)
				.await?;
			}
			Ok(swept)
		})
		.await
	{
		Ok(swept) => swept,
		Err(e) => {
			error!("ansible-leases: sweeping failed: {e}");
			return (0, 0);
		}
	};

	if swept.unstarted.is_empty() && swept.expired.is_empty() {
		debug!("ansible-leases: nothing to release");
	} else {
		info!(
			unstarted = swept.unstarted.len(),
			expired = swept.expired.len(),
			"ansible-leases: released leases"
		);
	}
	(swept.unstarted.len(), swept.expired.len())
}

/// The group and the lease an entry about a lease names.
async fn records(
	conn: &mut AsyncPgConnection,
	lease: &AnsibleLease,
) -> commons_errors::Result<(Record, Record)> {
	let group = ServerGroup::get_by_id(conn, lease.server_group_id).await?;
	Ok((
		Record::new(RecordKind::Group, group.id, group.name.clone()),
		Record::new(
			RecordKind::Lease,
			lease.id,
			format!("{} {}", group.name, lease.rank),
		),
	))
}

pub fn spawn() -> JoinHandle<()> {
	let pool = database::init();
	task::spawn(async move {
		loop {
			sleep(TICK).await;
			let Ok(mut db) = pool.get().await else {
				error!("ansible-leases: failed to get a database connection");
				continue;
			};
			tick(&mut db, Timestamp::now()).await;
		}
	})
}

#[cfg(test)]
mod tests {
	use std::collections::BTreeMap;

	use commons_tests::diesel_async::SimpleAsyncConnection;
	use commons_types::server::rank::ServerRank;
	use database::{
		ansible_leases::{PENDING_FOR, RunIntent, RunSpec},
		audit::{Filter, Status},
	};
	use jiff::SignedDuration;
	use uuid::Uuid;

	use super::*;

	async fn lease(conn: &mut AsyncPgConnection, group: Uuid, machine: Uuid) -> AnsibleLease {
		let none = BTreeMap::new();
		AnsibleLease::take(
			conn,
			group,
			ServerRank::Production,
			&[machine],
			&RunSpec {
				intent: RunIntent::Configure,
				note: None,
				playbook: Some("tamanu-install.yml"),
				limit_pattern: None,
				limited_to: None,
				tags: &[],
				skip_tags: &[],
				assignments: &none,
			},
			Some("sam@example.org"),
			false,
		)
		.await
		.unwrap()
		.unwrap()
	}

	async fn seed(conn: &mut AsyncPgConnection) -> (Uuid, Uuid) {
		let (group, machine) = (Uuid::new_v4(), Uuid::new_v4());
		conn.batch_execute(&format!(
			"INSERT INTO server_groups (id, name) VALUES ('{group}', 'kamaka');
			 INSERT INTO machines (id, name, group_id, rank)
			 VALUES ('{machine}', 'kamaka-central', '{group}', 'production')"
		))
		.await
		.unwrap();
		(group, machine)
	}

	async fn entries(conn: &mut AsyncPgConnection, action: &str) -> Vec<database::audit::Entry> {
		audit::find(
			conn,
			&Filter {
				action: Some(action.into()),
				include_routine: true,
				limit: 10,
				..Filter::default()
			},
		)
		.await
		.unwrap()
	}

	/// A lease no run read is released by Canopy, and one that ran out leaves
	/// an entry, which it did not before: both are on the record as Canopy's own.
	// spec: ANSRUN#starting-a-run, ANSRUN#audit
	#[tokio::test(flavor = "multi_thread")]
	async fn records_a_lease_no_run_read_and_one_that_ran_out_as_canopys_own() {
		commons_tests::db::TestDb::run(async |mut conn, _| {
			let (group, machine) = seed(&mut conn).await;
			let unread = lease(&mut conn, group, machine).await;

			let late = Timestamp::now() + PENDING_FOR + SignedDuration::from_secs(10);
			assert_eq!(tick(&mut conn, late).await, (1, 0));
			assert_eq!(tick(&mut conn, late).await, (0, 0), "released once");

			let released = entries(&mut conn, "inventory_lease_released").await;
			assert_eq!(released.len(), 1);
			assert_eq!(released[0].actor, Actor::canopy(JOB));
			assert_eq!(released[0].detail["reason"], "unstarted");
			assert_eq!(released[0].status, Status::Done);
			assert!(
				released[0]
					.records
					.iter()
					.any(|record| record.id == unread.id.to_string())
			);

			let held = lease(&mut conn, group, machine).await;
			AnsibleLease::start(&mut conn, held.id).await.unwrap();
			let after = held.expires_at + SignedDuration::from_secs(1);
			assert_eq!(tick(&mut conn, after).await, (0, 1));

			let expired = entries(&mut conn, "ansible_lease_expired").await;
			assert_eq!(expired.len(), 1);
			assert_eq!(expired[0].actor, Actor::canopy(JOB));
			assert_eq!(expired[0].detail["held_by"], "sam@example.org");
		})
		.await
	}
}
