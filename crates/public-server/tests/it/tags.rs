use diesel::{sql_query, sql_types};
use diesel_async::RunQueryDsl;
use http::StatusCode;
use std::collections::HashMap;
use uuid::Uuid;

/// Group-only tags propagate through the merged endpoint when the server
/// has no tags of its own.
#[tokio::test(flavor = "multi_thread")]
async fn tags_endpoint_returns_group_tags_when_server_has_none() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let group_id = Uuid::new_v4();
			let server_id = Uuid::new_v4();
			sql_query(
				"INSERT INTO server_groups (id, name, tags) \
				 VALUES ($1, 'tagged-cluster', '{\"region\": \"au\", \"tier\": \"1\"}'::jsonb)",
			)
			.bind::<sql_types::Uuid, _>(group_id)
			.execute(&mut conn)
			.await
			.unwrap();
			sql_query(
				"WITH m AS (INSERT INTO machines (name, id, group_id, device_id) VALUES ('box', $1, $3, $2) RETURNING id) INSERT INTO applications (id, host, type, group_id, machine_id) \
				 VALUES ($1, 'https://t.example.com', 'tamanu-central', $3, $1)",
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.bind::<sql_types::Uuid, _>(device_id)
			.bind::<sql_types::Uuid, _>(group_id)
			.execute(&mut conn)
			.await
			.unwrap();

			let response = public
				.get("/tags")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			response.assert_status_ok();
			let tags: HashMap<String, String> = response.json();
			assert_eq!(tags.get("region"), Some(&"au".to_string()));
			assert_eq!(tags.get("tier"), Some(&"1".to_string()));
		},
	)
	.await
}

/// Application tags win on key collision; non-colliding group keys carry through.
#[tokio::test(flavor = "multi_thread")]
async fn tags_endpoint_overlays_server_tags_onto_group() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let group_id = Uuid::new_v4();
			let server_id = Uuid::new_v4();
			sql_query(
				"INSERT INTO server_groups (id, name, tags) \
				 VALUES ($1, 'overlay-cluster', '{\"env\": \"group\", \"tier\": \"1\"}'::jsonb)",
			)
			.bind::<sql_types::Uuid, _>(group_id)
			.execute(&mut conn)
			.await
			.unwrap();
			sql_query(
				"WITH m AS (INSERT INTO machines (name, id, group_id, device_id) VALUES ('box', $1, $3, $2) RETURNING id) INSERT INTO applications (id, host, type, group_id, tags, machine_id) \
				 VALUES ($1, 'https://o.example.com', 'tamanu-central', $3, '{\"env\": \"server\", \"region\": \"au\"}'::jsonb, $1)",
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.bind::<sql_types::Uuid, _>(device_id)
			.bind::<sql_types::Uuid, _>(group_id)
			.execute(&mut conn)
			.await
			.unwrap();

			let response = public
				.get("/tags")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			response.assert_status_ok();
			let tags: HashMap<String, String> = response.json();
			// Application overrides on the colliding key.
			assert_eq!(tags.get("env"), Some(&"server".to_string()));
			// Group's non-colliding key carries through.
			assert_eq!(tags.get("tier"), Some(&"1".to_string()));
			// Application's exclusive key is present.
			assert_eq!(tags.get("region"), Some(&"au".to_string()));
		},
	)
	.await
}

/// An ungrouped server returns just its own tags — no group overlay.
#[tokio::test(flavor = "multi_thread")]
async fn tags_endpoint_returns_server_tags_when_ungrouped() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = Uuid::new_v4();
			sql_query(
				"WITH m AS (INSERT INTO machines (name, id, device_id) VALUES ('box', $1, $2) RETURNING id) INSERT INTO applications (id, host, type, tags, machine_id) \
				 VALUES ($1, 'https://lone.example.com', 'tamanu-central', '{\"role\": \"primary\"}'::jsonb, $1)",
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.bind::<sql_types::Uuid, _>(device_id)
			.execute(&mut conn)
			.await
			.unwrap();

			let response = public
				.get("/tags")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			response.assert_status_ok();
			let tags: HashMap<String, String> = response.json();
			assert_eq!(tags.get("role"), Some(&"primary".to_string()));
			// Synthetic product and kind tags are always present; ungrouped, so
			// no group tags.
			assert_eq!(
				tags.get("canopy:type"),
				Some(&"tamanu-central".to_string())
			);
			// Both halves of the pair the type replaced, so a rule written
			// against the earlier names keeps matching.
			assert_eq!(tags.get("canopy:product"), Some(&"tamanu".to_string()));
			assert_eq!(tags.get("canopy:kind"), Some(&"central".to_string()));
			assert_eq!(tags.get("canopy:group-id"), None);
			assert_eq!(tags.get("canopy:group-name"), None);
			// No rank set on this server, so no synthetic rank tag.
			assert_eq!(tags.get("canopy:rank"), None);
			assert_eq!(tags.len(), 4);
		},
	)
	.await
}

/// The endpoint injects synthetic `canopy:`-prefixed tags describing the
/// server's kind, rank, and group on top of the stored tags.
#[tokio::test(flavor = "multi_thread")]
async fn tags_endpoint_includes_synthetic_server_attributes() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let group_id = Uuid::new_v4();
			let server_id = Uuid::new_v4();
			sql_query("INSERT INTO server_groups (id, name) VALUES ($1, 'synthetic-cluster')")
				.bind::<sql_types::Uuid, _>(group_id)
				.execute(&mut conn)
				.await
				.unwrap();
			sql_query(
				"WITH m AS (INSERT INTO machines (name, id, group_id, device_id) VALUES ('box', $1, $3, $2) RETURNING id) INSERT INTO applications (id, host, type, rank, group_id, machine_id) \
				 VALUES ($1, 'https://s.example.com', 'tamanu-facility', 'production', $3, $1)",
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.bind::<sql_types::Uuid, _>(device_id)
			.bind::<sql_types::Uuid, _>(group_id)
			.execute(&mut conn)
			.await
			.unwrap();

			let response = public
				.get("/tags")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			response.assert_status_ok();
			let tags: HashMap<String, String> = response.json();
			assert_eq!(tags.get("canopy:kind"), Some(&"facility".to_string()));
			assert_eq!(tags.get("canopy:rank"), Some(&"production".to_string()));
			assert_eq!(tags.get("canopy:group-id"), Some(&group_id.to_string()));
			assert_eq!(
				tags.get("canopy:group-name"),
				Some(&"synthetic-cluster".to_string())
			);
		},
	)
	.await
}

/// A grouped server's tags include the group's effective `billing.*` labels:
/// computed defaults where the group sets nothing, and the group's explicit
/// `billing.*` tags honoured verbatim.
#[tokio::test(flavor = "multi_thread")]
async fn tags_endpoint_includes_effective_billing_labels() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let group_id = Uuid::new_v4();
			let server_id = Uuid::new_v4();
			// Group sets an explicit billing.deployment override but leaves
			// product/stage to be computed.
			sql_query(
				"INSERT INTO server_groups (id, name, tags) \
				 VALUES ($1, 'Billing Cluster', '{\"billing.deployment\": \"acme\"}'::jsonb)",
			)
			.bind::<sql_types::Uuid, _>(group_id)
			.execute(&mut conn)
			.await
			.unwrap();
			sql_query(
				"WITH m AS (INSERT INTO machines (name, id, group_id, device_id) VALUES ('box', $1, $3, $2) RETURNING id) INSERT INTO applications (id, host, type, rank, group_id, machine_id) \
				 VALUES ($1, 'https://b.example.com', 'tamanu-central', 'production', $3, $1)",
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.bind::<sql_types::Uuid, _>(device_id)
			.bind::<sql_types::Uuid, _>(group_id)
			.execute(&mut conn)
			.await
			.unwrap();

			let response = public
				.get("/tags")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			response.assert_status_ok();
			let tags: HashMap<String, String> = response.json();
			// Computed default product.
			assert_eq!(tags.get("billing.product"), Some(&"tamanu".to_string()));
			// Explicit group override honoured verbatim.
			assert_eq!(tags.get("billing.deployment"), Some(&"acme".to_string()));
			// Stage mapped from the requesting server's own rank (production -> prod).
			assert_eq!(tags.get("billing.stage"), Some(&"prod".to_string()));
		},
	)
	.await
}

/// A server's own stored `billing.*` tags win over both the group's tags and
/// the computed defaults: an operator can pin any billing label on a specific
/// server, including overriding the rank-derived stage.
#[tokio::test(flavor = "multi_thread")]
async fn tags_endpoint_server_billing_tags_win() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let group_id = Uuid::new_v4();
			let server_id = Uuid::new_v4();
			// Group pins product + stage; the server will override all three.
			sql_query(
				"INSERT INTO server_groups (id, name, tags) \
				 VALUES ($1, 'Override Cluster', '{\"billing.product\": \"tamanu\", \"billing.stage\": \"prod\"}'::jsonb)",
			)
			.bind::<sql_types::Uuid, _>(group_id)
			.execute(&mut conn)
			.await
			.unwrap();
			sql_query(
				"WITH m AS (INSERT INTO machines (name, id, group_id, device_id) VALUES ('box', $1, $3, $2) RETURNING id) INSERT INTO applications (id, host, type, rank, group_id, tags, machine_id) \
				 VALUES ($1, 'https://ov.example.com', 'tamanu-central', 'production', $3, \
				 '{\"billing.product\": \"pgro\", \"billing.deployment\": \"custom-dep\", \"billing.stage\": \"staging\"}'::jsonb, $1)",
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.bind::<sql_types::Uuid, _>(device_id)
			.bind::<sql_types::Uuid, _>(group_id)
			.execute(&mut conn)
			.await
			.unwrap();

			let response = public
				.get("/tags")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			response.assert_status_ok();
			let tags: HashMap<String, String> = response.json();
			// Application tags win over the group tags and the computed rank default.
			assert_eq!(tags.get("billing.product"), Some(&"pgro".to_string()));
			assert_eq!(tags.get("billing.deployment"), Some(&"custom-dep".to_string()));
			assert_eq!(tags.get("billing.stage"), Some(&"staging".to_string()));
		},
	)
	.await
}

/// A group's stored `billing.*` tags win over the computed defaults when the
/// server sets none of its own.
#[tokio::test(flavor = "multi_thread")]
async fn tags_endpoint_group_billing_tags_win_over_defaults() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let group_id = Uuid::new_v4();
			let server_id = Uuid::new_v4();
			// Group pins stage explicitly; server has a rank but no billing tags.
			sql_query(
				"INSERT INTO server_groups (id, name, tags) \
				 VALUES ($1, 'Group Stage Cluster', '{\"billing.stage\": \"sandbox\"}'::jsonb)",
			)
			.bind::<sql_types::Uuid, _>(group_id)
			.execute(&mut conn)
			.await
			.unwrap();
			sql_query(
				"WITH m AS (INSERT INTO machines (name, id, group_id, device_id) VALUES ('box', $1, $3, $2) RETURNING id) INSERT INTO applications (id, host, type, rank, group_id, machine_id) \
				 VALUES ($1, 'https://gs.example.com', 'tamanu-central', 'production', $3, $1)",
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.bind::<sql_types::Uuid, _>(device_id)
			.bind::<sql_types::Uuid, _>(group_id)
			.execute(&mut conn)
			.await
			.unwrap();

			let response = public
				.get("/tags")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			response.assert_status_ok();
			let tags: HashMap<String, String> = response.json();
			// Group's explicit stage wins over the rank-derived "prod".
			assert_eq!(tags.get("billing.stage"), Some(&"sandbox".to_string()));
		},
	)
	.await
}

/// `billing.stage` is derived from the requesting server's own rank, not the
/// group's highest-ranked member: a `clone` server in a group that also holds
/// a `production` server reports `billing.stage=clone`, never `prod`.
#[tokio::test(flavor = "multi_thread")]
async fn tags_endpoint_billing_stage_is_per_server_rank() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let group_id = Uuid::new_v4();
			let clone_id = Uuid::new_v4();
			let prod_id = Uuid::new_v4();
			sql_query("INSERT INTO server_groups (id, name) VALUES ($1, 'Mixed Cluster')")
				.bind::<sql_types::Uuid, _>(group_id)
				.execute(&mut conn)
				.await
				.unwrap();
			// A higher-ranked (production) sibling in the same group.
			sql_query(
				"WITH m AS (INSERT INTO machines (name, id, group_id) VALUES ('box', $1, $2) RETURNING id) INSERT INTO applications (id, host, type, rank, group_id, machine_id) \
				 VALUES ($1, 'https://prod.example.com', 'tamanu-central', 'production', $2, $1)",
			)
			.bind::<sql_types::Uuid, _>(prod_id)
			.bind::<sql_types::Uuid, _>(group_id)
			.execute(&mut conn)
			.await
			.unwrap();
			// The requesting server is a clone.
			sql_query(
				"WITH m AS (INSERT INTO machines (name, id, group_id, device_id) VALUES ('box', $1, $3, $2) RETURNING id) INSERT INTO applications (id, host, type, rank, group_id, machine_id) \
				 VALUES ($1, 'https://clone.example.com', 'tamanu-central', 'clone', $3, $1)",
			)
			.bind::<sql_types::Uuid, _>(clone_id)
			.bind::<sql_types::Uuid, _>(device_id)
			.bind::<sql_types::Uuid, _>(group_id)
			.execute(&mut conn)
			.await
			.unwrap();

			let response = public
				.get("/tags")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			response.assert_status_ok();
			let tags: HashMap<String, String> = response.json();
			// The clone reports its own stage, not the group's production.
			assert_eq!(tags.get("billing.stage"), Some(&"clone".to_string()));
		},
	)
	.await
}

/// A grouped server with no rank set gets no `billing.stage` label — there's no
/// stage to attribute its cost to — while the group-level labels still appear.
#[tokio::test(flavor = "multi_thread")]
async fn tags_endpoint_no_billing_stage_when_server_unranked() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let group_id = Uuid::new_v4();
			let server_id = Uuid::new_v4();
			sql_query("INSERT INTO server_groups (id, name) VALUES ($1, 'Unranked Cluster')")
				.bind::<sql_types::Uuid, _>(group_id)
				.execute(&mut conn)
				.await
				.unwrap();
			sql_query(
				"WITH m AS (INSERT INTO machines (name, id, group_id, device_id) VALUES ('box', $1, $3, $2) RETURNING id) INSERT INTO applications (id, host, type, group_id, machine_id) \
				 VALUES ($1, 'https://ur.example.com', 'tamanu-central', $3, $1)",
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.bind::<sql_types::Uuid, _>(device_id)
			.bind::<sql_types::Uuid, _>(group_id)
			.execute(&mut conn)
			.await
			.unwrap();

			let response = public
				.get("/tags")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			response.assert_status_ok();
			let tags: HashMap<String, String> = response.json();
			// Group-level labels still present, but no per-server stage.
			assert_eq!(tags.get("billing.product"), Some(&"tamanu".to_string()));
			assert_eq!(tags.get("billing.stage"), None);
		},
	)
	.await
}

/// An ungrouped server gets no billing labels — they're a group concept.
#[tokio::test(flavor = "multi_thread")]
async fn tags_endpoint_no_billing_labels_when_ungrouped() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = Uuid::new_v4();
			sql_query(
				"WITH m AS (INSERT INTO machines (name, id, device_id) VALUES ('box', $1, $2) RETURNING id) INSERT INTO applications (id, host, type, machine_id) \
				 VALUES ($1, 'https://nb.example.com', 'tamanu-central', $1)",
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.bind::<sql_types::Uuid, _>(device_id)
			.execute(&mut conn)
			.await
			.unwrap();

			let response = public
				.get("/tags")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			response.assert_status_ok();
			let tags: HashMap<String, String> = response.json();
			assert_eq!(tags.get("billing.product"), None);
			assert_eq!(tags.get("billing.deployment"), None);
			assert_eq!(tags.get("billing.stage"), None);
		},
	)
	.await
}

/// A device that authenticates correctly but isn't attached to any server
/// gets a 412 (precondition failed) — same code the events endpoint uses
/// for the same situation.
#[tokio::test(flavor = "multi_thread")]
async fn tags_endpoint_412_when_device_has_no_server() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut _conn, cert, _device_id, public, _| {
			let response = public
				.get("/tags")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			response.assert_status(StatusCode::PRECONDITION_FAILED);
		},
	)
	.await
}

/// A server's billing product is its own, not its group's: a SENAITE server
/// sharing a group with Tamanu ones attributes its cloud cost to SENAITE, since
/// charging it to the group's application would put the spend in the wrong
/// place.
// spec: APP#billing-attribution
#[tokio::test(flavor = "multi_thread")]
async fn tags_endpoint_billing_product_is_the_servers_own() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let group_id = Uuid::new_v4();
			let server_id = Uuid::new_v4();
			sql_query("INSERT INTO server_groups (id, name) VALUES ($1, 'Pacific')")
				.bind::<sql_types::Uuid, _>(group_id)
				.execute(&mut conn)
				.await
				.unwrap();
			// A Tamanu member alongside it, so the group genuinely spans products.
			sql_query(
				"WITH m AS (INSERT INTO machines (name, id, group_id) VALUES ('box', $1, $2) RETURNING id) INSERT INTO applications (id, host, type, rank, group_id, machine_id) \
				 VALUES ($1, 'https://central.example.com', 'tamanu-central', 'production', $2, $1)",
			)
			.bind::<sql_types::Uuid, _>(Uuid::new_v4())
			.bind::<sql_types::Uuid, _>(group_id)
			.execute(&mut conn)
			.await
			.unwrap();
			sql_query(
				"WITH m AS (INSERT INTO machines (name, id, group_id, device_id) VALUES ('box', $1, $3, $2) RETURNING id) INSERT INTO applications (id, host, type, rank, group_id, machine_id) \
				 VALUES ($1, 'https://lims.example.com', 'senaite', 'clone', $3, $1)",
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.bind::<sql_types::Uuid, _>(device_id)
			.bind::<sql_types::Uuid, _>(group_id)
			.execute(&mut conn)
			.await
			.unwrap();

			let response = public
				.get("/tags")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			response.assert_status_ok();
			let tags: HashMap<String, String> = response.json();
			assert_eq!(tags.get("billing.product"), Some(&"senaite".to_string()));
			// The deployment label still comes from the group it belongs to...
			assert_eq!(tags.get("billing.deployment"), Some(&"pacific".to_string()));
			// ...and the stage from its own rank, not the group's highest.
			assert_eq!(tags.get("billing.stage"), Some(&"clone".to_string()));
		},
	)
	.await
}

/// Billing attribution needs a group to attribute to, so an ungrouped
/// server carries no `billing.*` labels at all — not even the product it could
/// name on its own.
// spec: APP#billing-attribution
#[tokio::test(flavor = "multi_thread")]
async fn tags_endpoint_ungrouped_server_has_no_billing_product() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			sql_query(
				"WITH m AS (INSERT INTO machines (name, id, device_id) VALUES ('box', $1, $2) RETURNING id) INSERT INTO applications (id, host, type, machine_id) \
				 VALUES ($1, 'https://lone-lims.example.com', 'senaite', $1)",
			)
			.bind::<sql_types::Uuid, _>(Uuid::new_v4())
			.bind::<sql_types::Uuid, _>(device_id)
			.execute(&mut conn)
			.await
			.unwrap();

			let response = public
				.get("/tags")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			response.assert_status_ok();
			let tags: HashMap<String, String> = response.json();
			assert_eq!(tags.get("billing.product"), None);
			assert_eq!(tags.get("billing.deployment"), None);
			assert_eq!(tags.get("billing.stage"), None);
			// The classification tags are still there — those describe the
			// server, not what its cost attributes to.
			assert_eq!(tags.get("canopy:product"), Some(&"senaite".to_string()));
		},
	)
	.await
}
