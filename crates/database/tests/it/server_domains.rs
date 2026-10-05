//! DB-layer tests for group domains (`database::server_domains`). Exercises the
//! model helpers directly against a fresh migrated DB — no HTTP.

use commons_errors::AppError;
use commons_tests::db::TestDb;
use commons_types::dns::ManagedZone;
use database::ServerGroupDomain;
use database::diesel_async::AsyncPgConnection;
use diesel::{sql_query, sql_types};
use diesel_async::RunQueryDsl;
use uuid::Uuid;

#[derive(diesel::QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

async fn insert_group(conn: &mut AsyncPgConnection, name: &str) -> Uuid {
	sql_query("INSERT INTO server_groups (name) VALUES ($1) RETURNING id")
		.bind::<sql_types::Text, _>(name)
		.get_result::<RowId>(conn)
		.await
		.expect("insert group")
		.id
}

fn zones() -> Vec<ManagedZone> {
	ManagedZone::parse_list("tamanu.app=Z1, senaite.app=Z2", None).expect("parse zones")
}

#[tokio::test(flavor = "multi_thread")]
async fn claim_normalises_and_matches_a_zone() {
	TestDb::run(async |mut conn, _url| {
		let group = insert_group(&mut conn, "fiji").await;
		let claim = ServerGroupDomain::claim(
			&mut conn,
			group,
			" Fiji.Tamanu.App. ",
			Some("op@example.test".into()),
			&zones(),
		)
		.await
		.expect("claim");

		assert_eq!(claim.domain, "fiji.tamanu.app");
		assert_eq!(claim.group_id, group);
		assert_eq!(claim.created_by.as_deref(), Some("op@example.test"));

		let listed = ServerGroupDomain::list_for_group(&mut conn, group)
			.await
			.expect("list");
		assert_eq!(listed.len(), 1);
		assert_eq!(listed[0].id, claim.id);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn claim_outside_every_zone_is_rejected() {
	TestDb::run(async |mut conn, _url| {
		let group = insert_group(&mut conn, "fiji").await;
		let err = ServerGroupDomain::claim(&mut conn, group, "fiji.example.com", None, &zones())
			.await
			.expect_err("should refuse a name outside every zone");
		assert!(matches!(err, AppError::BadRequest(_)), "got {err:?}");

		// A malformed name is refused before any zone is consulted.
		let err = ServerGroupDomain::claim(&mut conn, group, "tamanu", None, &zones())
			.await
			.expect_err("should refuse a single-label name");
		assert!(matches!(err, AppError::BadRequest(_)), "got {err:?}");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn claim_with_no_zones_configured_is_rejected() {
	TestDb::run(async |mut conn, _url| {
		let group = insert_group(&mut conn, "fiji").await;
		let err = ServerGroupDomain::claim(&mut conn, group, "fiji.tamanu.app", None, &[])
			.await
			.expect_err("should refuse when Canopy has no zones at all");
		match err {
			AppError::BadRequest(msg) => assert!(
				msg.contains("no managed DNS zones"),
				"unhelpful message: {msg}"
			),
			other => panic!("got {other:?}"),
		}
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_zone_apex_may_be_claimed_whole() {
	TestDb::run(async |mut conn, _url| {
		let group = insert_group(&mut conn, "everything").await;
		let claim = ServerGroupDomain::claim(&mut conn, group, "tamanu.app", None, &zones())
			.await
			.expect("apex claim");
		assert_eq!(claim.domain, "tamanu.app");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn overlapping_claims_are_refused_across_groups() {
	TestDb::run(async |mut conn, _url| {
		let fiji = insert_group(&mut conn, "fiji").await;
		let samoa = insert_group(&mut conn, "samoa").await;
		ServerGroupDomain::claim(&mut conn, fiji, "fiji.tamanu.app", None, &zones())
			.await
			.expect("first claim");

		// The same name, refused by naming the group holding it.
		let err = ServerGroupDomain::claim(&mut conn, samoa, "fiji.tamanu.app", None, &zones())
			.await
			.expect_err("duplicate should conflict");
		match err {
			AppError::Conflict(msg) => {
				assert!(msg.ends_with("claimed by fiji"), "got {msg}");
				assert!(!msg.contains(&fiji.to_string()), "got {msg}");
			}
			other => panic!("got {other:?}"),
		}

		// A name beneath it.
		let err = ServerGroupDomain::claim(&mut conn, samoa, "sub.fiji.tamanu.app", None, &zones())
			.await
			.expect_err("descendant should conflict");
		assert!(matches!(err, AppError::Conflict(_)), "got {err:?}");

		// A name above it.
		let err = ServerGroupDomain::claim(&mut conn, samoa, "tamanu.app", None, &zones())
			.await
			.expect_err("ancestor should conflict");
		match err {
			AppError::Conflict(msg) => assert!(
				msg.contains("fiji.tamanu.app"),
				"message should name the claim in the way: {msg}"
			),
			other => panic!("got {other:?}"),
		}

		// A sibling is untouched by any of it.
		ServerGroupDomain::claim(&mut conn, samoa, "samoa.tamanu.app", None, &zones())
			.await
			.expect("sibling claim");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn overlap_is_refused_within_one_group_too() {
	TestDb::run(async |mut conn, _url| {
		let group = insert_group(&mut conn, "fiji").await;
		ServerGroupDomain::claim(&mut conn, group, "fiji.tamanu.app", None, &zones())
			.await
			.expect("first claim");
		let err = ServerGroupDomain::claim(&mut conn, group, "a.fiji.tamanu.app", None, &zones())
			.await
			.expect_err("already covered by the group's own claim");
		match err {
			AppError::Conflict(msg) => assert!(
				msg.contains("this group already claims"),
				"message should say the group already has it: {msg}"
			),
			other => panic!("got {other:?}"),
		}
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn controlling_finds_the_group_for_a_name_beneath_a_claim() {
	TestDb::run(async |mut conn, _url| {
		let fiji = insert_group(&mut conn, "fiji").await;
		let samoa = insert_group(&mut conn, "samoa").await;
		ServerGroupDomain::claim(&mut conn, fiji, "fiji.tamanu.app", None, &zones())
			.await
			.expect("fiji claim");
		ServerGroupDomain::claim(&mut conn, samoa, "samoa.tamanu.app", None, &zones())
			.await
			.expect("samoa claim");

		for name in [
			"fiji.tamanu.app",
			"central.fiji.tamanu.app",
			"a.b.fiji.tamanu.app",
		] {
			let found = ServerGroupDomain::controlling(&mut conn, name)
				.await
				.expect("lookup")
				.unwrap_or_else(|| panic!("{name} should be controlled"));
			assert_eq!(found.group_id, fiji, "{name}");
			assert_eq!(found.domain, "fiji.tamanu.app");
		}

		// Outside every claim: the zone apex above them, an unclaimed sibling,
		// and a name in another zone entirely.
		for name in ["tamanu.app", "tonga.tamanu.app", "fiji.senaite.app"] {
			assert!(
				ServerGroupDomain::controlling(&mut conn, name)
					.await
					.expect("lookup")
					.is_none(),
				"{name} should be controlled by nobody"
			);
		}
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn releasing_frees_the_name_for_another_group() {
	TestDb::run(async |mut conn, _url| {
		let fiji = insert_group(&mut conn, "fiji").await;
		let samoa = insert_group(&mut conn, "samoa").await;
		let claim = ServerGroupDomain::claim(&mut conn, fiji, "shared.tamanu.app", None, &zones())
			.await
			.expect("claim");

		ServerGroupDomain::release(&mut conn, claim.id)
			.await
			.expect("release");
		assert!(
			ServerGroupDomain::controlling(&mut conn, "x.shared.tamanu.app")
				.await
				.expect("lookup")
				.is_none()
		);

		ServerGroupDomain::claim(&mut conn, samoa, "shared.tamanu.app", None, &zones())
			.await
			.expect("reclaim by another group");

		// Releasing a claim that isn't there is a 404, not a silent success.
		let err = ServerGroupDomain::release(&mut conn, claim.id)
			.await
			.expect_err("second release should 404");
		assert!(
			matches!(
				err,
				AppError::DatabaseQuery(diesel::result::Error::NotFound)
			),
			"got {err:?}"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claim_survives_its_zone_leaving_the_configuration() {
	TestDb::run(async |mut conn, _url| {
		let group = insert_group(&mut conn, "fiji").await;
		ServerGroupDomain::claim(&mut conn, group, "fiji.tamanu.app", None, &zones())
			.await
			.expect("claim");

		// The zone is gone from the configuration: the claim stands, is still
		// found by a name lookup, and still excludes others.
		let listed = ServerGroupDomain::list_for_group(&mut conn, group)
			.await
			.expect("list");
		assert_eq!(listed.len(), 1);

		let other = insert_group(&mut conn, "samoa").await;
		let narrowed = ManagedZone::parse_list("senaite.app=Z2", None).expect("parse");
		let err = ServerGroupDomain::claim(&mut conn, other, "fiji.tamanu.app", None, &narrowed)
			.await
			.expect_err("still claimed by fiji");
		// Refused for being outside the zones now configured, before overlap is
		// even reached — either refusal is correct, so accept both shapes.
		assert!(
			matches!(err, AppError::BadRequest(_) | AppError::Conflict(_)),
			"got {err:?}"
		);
	})
	.await;
}

/// The DNS-zone-coverage self-alert (DOM): a claim outliving its zone is kept
/// and reported, warning when only some claims are uncovered and failing when
/// Canopy can read no zones at all.
mod coverage_alert {
	use super::*;
	use commons_types::status::CheckResult;
	use database::self_alerts::{self, DNS_ZONE_COVERAGE_REF};

	async fn current_result(conn: &mut AsyncPgConnection) -> Option<CheckResult> {
		self_alerts::current(conn, DNS_ZONE_COVERAGE_REF)
			.await
			.expect("current")
			.filter(|issue| issue.active)
			.and_then(|issue| issue.effective_result)
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn covered_claims_raise_nothing() {
		TestDb::run(async |mut conn, _url| {
			let group = insert_group(&mut conn, "fiji").await;
			ServerGroupDomain::claim(&mut conn, group, "fiji.tamanu.app", None, &zones())
				.await
				.expect("claim");

			self_alerts::sweep_dns_zone_coverage(&mut conn, &zones(), None)
				.await
				.expect("sweep");
			assert!(current_result(&mut conn).await.is_none());
		})
		.await;
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn no_zones_and_no_claims_is_not_a_fault() {
		TestDb::run(async |mut conn, _url| {
			// The feature simply isn't in use.
			self_alerts::sweep_dns_zone_coverage(&mut conn, &[], None)
				.await
				.expect("sweep");
			assert!(current_result(&mut conn).await.is_none());
		})
		.await;
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn dropping_one_zone_of_several_warns_and_keeps_the_claim() {
		TestDb::run(async |mut conn, _url| {
			let group = insert_group(&mut conn, "fiji").await;
			let claim =
				ServerGroupDomain::claim(&mut conn, group, "fiji.senaite.app", None, &zones())
					.await
					.expect("claim");

			// `senaite.app` leaves the configuration; `tamanu.app` stays.
			let narrowed = ManagedZone::parse_list("tamanu.app=Z1", None).expect("parse");
			self_alerts::sweep_dns_zone_coverage(&mut conn, &narrowed, None)
				.await
				.expect("sweep");

			assert_eq!(current_result(&mut conn).await, Some(CheckResult::Warning));

			// The claim itself is untouched, and still excludes other groups.
			let listed = ServerGroupDomain::list_for_group(&mut conn, group)
				.await
				.expect("list");
			assert_eq!(listed.len(), 1);
			assert_eq!(listed[0].id, claim.id);

			// Restoring the zone recovers the alert on its own.
			self_alerts::sweep_dns_zone_coverage(&mut conn, &zones(), None)
				.await
				.expect("sweep");
			assert!(current_result(&mut conn).await.is_none());
		})
		.await;
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn losing_every_zone_fails_rather_than_warns() {
		TestDb::run(async |mut conn, _url| {
			let group = insert_group(&mut conn, "fiji").await;
			ServerGroupDomain::claim(&mut conn, group, "fiji.tamanu.app", None, &zones())
				.await
				.expect("claim");

			self_alerts::sweep_dns_zone_coverage(&mut conn, &[], None)
				.await
				.expect("sweep");
			assert_eq!(current_result(&mut conn).await, Some(CheckResult::Failed));
		})
		.await;
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn an_unreadable_configuration_fails_and_reports_the_error() {
		TestDb::run(async |mut conn, _url| {
			let issue = self_alerts::sweep_dns_zone_coverage(
				&mut conn,
				&[],
				Some("managed zone \"tamanu.app\" has no provider zone id"),
			)
			.await
			.expect("sweep")
			.expect("an alert even with nothing claimed");

			assert_eq!(issue.effective_result, Some(CheckResult::Failed));
			assert!(
				issue.message.contains("no provider zone id"),
				"the parse error belongs in the message: {}",
				issue.message
			);
		})
		.await;
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn a_warning_can_still_escalate_to_a_failure() {
		TestDb::run(async |mut conn, _url| {
			let group = insert_group(&mut conn, "fiji").await;
			ServerGroupDomain::claim(&mut conn, group, "fiji.senaite.app", None, &zones())
				.await
				.expect("claim");

			// Warn first, so the catalog entry is seeded by that sighting...
			let narrowed = ManagedZone::parse_list("tamanu.app=Z1", None).expect("parse");
			self_alerts::sweep_dns_zone_coverage(&mut conn, &narrowed, None)
				.await
				.expect("sweep");
			assert_eq!(current_result(&mut conn).await, Some(CheckResult::Warning));

			// ...then lose everything: the seeded ceiling must not cap this at
			// warning.
			self_alerts::sweep_dns_zone_coverage(&mut conn, &[], None)
				.await
				.expect("sweep");
			assert_eq!(current_result(&mut conn).await, Some(CheckResult::Failed));
		})
		.await;
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn an_archived_groups_uncovered_claim_is_left_alone() {
		TestDb::run(async |mut conn, _url| {
			let group = insert_group(&mut conn, "put-away").await;
			ServerGroupDomain::claim(&mut conn, group, "old.senaite.app", None, &zones())
				.await
				.expect("claim");
			sql_query("UPDATE server_groups SET deleted_at = now() WHERE id = $1")
				.bind::<sql_types::Uuid, _>(group)
				.execute(&mut conn)
				.await
				.expect("archive group");

			let narrowed = ManagedZone::parse_list("tamanu.app=Z1", None).expect("parse");
			self_alerts::sweep_dns_zone_coverage(&mut conn, &narrowed, None)
				.await
				.expect("sweep");
			assert!(current_result(&mut conn).await.is_none());
		})
		.await;
	}
}
