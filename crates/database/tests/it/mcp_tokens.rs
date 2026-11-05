//! Lifecycle of MCP bearer tokens: mint hands out a prefixed plaintext and
//! persists only the hash; find_active refuses revoked and expired tokens
//! identically; revoke is idempotent but 404s on unknown ids; expiring_soon
//! catches tokens inside the 15-day alert lead, including already-lapsed ones.

use database::mcp_tokens::{McpToken, TOKEN_PREFIX};
use diesel_async::SimpleAsyncConnection as _;
use uuid::Uuid;

#[tokio::test(flavor = "multi_thread")]
async fn mint_find_touch_revoke_lifecycle() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (token, plaintext) = McpToken::mint(&mut conn, "claude", "admin@example.com", &[])
			.await
			.expect("mint");
		assert!(plaintext.starts_with(TOKEN_PREFIX));
		assert_eq!(token.name, "claude");
		assert_eq!(token.created_by, "admin@example.com");
		assert!(token.revoked_at.is_none());
		assert!(token.last_used_at.is_none());
		// Roughly a year out; exact value is the model's business.
		let ttl = token.expires_at.duration_since(token.created_at);
		assert!(ttl.as_hours() > 364 * 24 && ttl.as_hours() <= 366 * 24);

		let found = McpToken::find_active(&mut conn, &plaintext)
			.await
			.expect("find")
			.expect("token is active");
		assert_eq!(found.id, token.id);

		assert!(
			McpToken::find_active(&mut conn, "canopy_mcp_not-a-real-token")
				.await
				.expect("find")
				.is_none()
		);

		McpToken::touch_last_used(&mut conn, token.id)
			.await
			.expect("touch");
		let listed = McpToken::list(&mut conn).await.expect("list");
		assert_eq!(listed.len(), 1);
		assert!(listed[0].last_used_at.is_some());

		McpToken::revoke(&mut conn, token.id).await.expect("revoke");
		assert!(
			McpToken::find_active(&mut conn, &plaintext)
				.await
				.expect("find")
				.is_none(),
			"revoked token must not authenticate"
		);
		// Idempotent on an already-revoked token…
		McpToken::revoke(&mut conn, token.id)
			.await
			.expect("re-revoke");
		// …but unknown ids are an error.
		assert!(McpToken::revoke(&mut conn, Uuid::new_v4()).await.is_err());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn expired_tokens_do_not_authenticate() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (token, plaintext) = McpToken::mint(&mut conn, "old", "admin@example.com", &[])
			.await
			.expect("mint");
		conn.batch_execute(&format!(
			"UPDATE mcp_tokens SET expires_at = NOW() - INTERVAL '1 day' WHERE id = '{}'",
			token.id
		))
		.await
		.expect("age token");

		assert!(
			McpToken::find_active(&mut conn, &plaintext)
				.await
				.expect("find")
				.is_none(),
			"expired token must not authenticate"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn expiring_soon_catches_the_alert_window() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (fresh, _) = McpToken::mint(&mut conn, "fresh", "a@example.com", &[])
			.await
			.expect("mint");
		let (closing, _) = McpToken::mint(&mut conn, "closing", "a@example.com", &[])
			.await
			.expect("mint");
		let (lapsed, _) = McpToken::mint(&mut conn, "lapsed", "a@example.com", &[])
			.await
			.expect("mint");
		let (revoked, _) = McpToken::mint(&mut conn, "revoked", "a@example.com", &[])
			.await
			.expect("mint");

		conn.batch_execute(&format!(
			"UPDATE mcp_tokens SET expires_at = NOW() + INTERVAL '10 days' WHERE id = '{}'; \
			 UPDATE mcp_tokens SET expires_at = NOW() - INTERVAL '2 days' WHERE id = '{}'; \
			 UPDATE mcp_tokens SET expires_at = NOW() + INTERVAL '10 days' WHERE id = '{}';",
			closing.id, lapsed.id, revoked.id,
		))
		.await
		.expect("adjust expiries");
		McpToken::revoke(&mut conn, revoked.id)
			.await
			.expect("revoke");

		let soon = McpToken::expiring_soon(&mut conn).await.expect("scan");
		let ids: Vec<_> = soon.iter().map(|t| t.id).collect();
		assert!(ids.contains(&closing.id), "inside the 15-day lead");
		assert!(ids.contains(&lapsed.id), "already lapsed still alerts");
		assert!(!ids.contains(&fresh.id), "a year out is not soon");
		assert!(!ids.contains(&revoked.id), "revoked tokens don't alert");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn expiry_sweep_files_one_self_alert_and_recovers() {
	use database::mcp_tokens::{EXPIRY_REF, sweep_token_expiry};
	use database::self_alerts;

	commons_tests::db::TestDb::run(async |mut conn, _url| {
		// Groups exist, but must NOT each get an alert: a rotation heads-up
		// is a single self-alert, not per group.
		conn.batch_execute(&format!(
			"INSERT INTO server_groups (id, name) VALUES ('{}', 'A'), ('{}', 'B');",
			Uuid::new_v4(),
			Uuid::new_v4(),
		))
		.await
		.expect("seed groups");

		// Nothing expiring, nothing alerted: the sweep is a no-op.
		assert_eq!(sweep_token_expiry(&mut conn).await.expect("sweep"), 0);
		assert!(
			self_alerts::list(&mut conn, 50)
				.await
				.expect("list")
				.is_empty(),
			"idle sweep must not file alerts"
		);

		// A token inside the 15-day lead raises the single self-alert.
		let (token, _) = McpToken::mint(&mut conn, "claude", "admin@example.com", &[])
			.await
			.expect("mint");
		conn.batch_execute(&format!(
			"UPDATE mcp_tokens SET expires_at = NOW() + INTERVAL '10 days' WHERE id = '{}'",
			token.id
		))
		.await
		.expect("age token");

		assert_eq!(sweep_token_expiry(&mut conn).await.expect("sweep"), 1);
		let alerts = self_alerts::list(&mut conn, 50).await.expect("list");
		let [issue] = alerts
			.iter()
			.filter(|i| i.r#ref == EXPIRY_REF)
			.collect::<Vec<_>>()[..]
		else {
			panic!("exactly one self-alert, got: {alerts:?}");
		};
		assert!(issue.active);
		assert_eq!(issue.application_id, None);
		assert_eq!(issue.server_group_id, None);
		assert!(issue.server_group_id.is_none());
		assert!(issue.message.contains("claude"), "{}", issue.message);
		assert!(issue.message.contains("in 10 days"), "{}", issue.message);

		// Revoking the token recovers the alert on the next sweep…
		McpToken::revoke(&mut conn, token.id).await.expect("revoke");
		assert_eq!(sweep_token_expiry(&mut conn).await.expect("sweep"), 1);
		assert!(
			self_alerts::list(&mut conn, 50)
				.await
				.expect("list")
				.iter()
				.filter(|i| i.r#ref == EXPIRY_REF)
				.all(|i| !i.active),
			"alert must recover after revocation"
		);

		// …and once recovered, further sweeps are no-ops again.
		assert_eq!(sweep_token_expiry(&mut conn).await.expect("sweep"), 0);
	})
	.await
}
