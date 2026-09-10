//! A group-scoped artifact reaches its own group's machines and nobody else.
//!
//! spec: ART

use axum::http::StatusCode;
use database::artifacts::{digest_of, sri};
use diesel_async::SimpleAsyncConnection;

const VERSION: &str = "11111111-1111-1111-1111-111111111111";
const UNSCOPED: &str = "22222222-2222-2222-2222-222222222222";
const THEIRS: &str = "33333333-3333-3333-3333-333333333333";
const GROUP_A: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
const GROUP_B: &str = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";

/// One published version, two groups, and a `reporting-schema` artifact for
/// each of the unscoped and group-A cases.
async fn seed(conn: &mut database::diesel_async::AsyncPgConnection) {
	let digest = hex::encode(digest_of(b"group a schema"));
	conn.batch_execute(&format!(
		"INSERT INTO versions (id, major, minor, patch, changelog, status)
		 VALUES ('{VERSION}', 2, 60, 0, '', 'published');

		 INSERT INTO server_groups (id, name) VALUES
		 ('{GROUP_A}', 'kamaka'), ('{GROUP_B}', 'drifting');

		 INSERT INTO artifacts (id, version_id, platform, artifact_type, download_url)
		 VALUES ('{UNSCOPED}', '{VERSION}', 'any', 'reporting-schema', 'https://example.com/all.sql');

		 INSERT INTO artifacts (id, version_id, platform, artifact_type, group_id, content, content_type, digest)
		 VALUES ('{THEIRS}', '{VERSION}', 'any', 'reporting-schema', '{GROUP_A}',
		         'group a schema'::bytea, 'application/sql', '\\x{digest}'::bytea)",
	))
	.await
	.expect("seed");
}

/// Put the authenticated device on a machine in the given group.
async fn enrol(
	conn: &mut database::diesel_async::AsyncPgConnection,
	device_id: uuid::Uuid,
	group: &str,
) {
	conn.batch_execute(&format!(
		"INSERT INTO machines (name, group_id, device_id)
		 VALUES ('box', '{group}', '{device_id}')"
	))
	.await
	.expect("enrol machine");
}

/// A read carrying no identity is answered with the unscoped artifacts alone,
/// so giving an artifact a group narrows who is offered it rather than
/// widening what an open path serves.
#[tokio::test(flavor = "multi_thread")]
async fn an_anonymous_read_sees_only_unscoped_artifacts() {
	commons_tests::server::run(async |mut conn, public, _| {
		seed(&mut conn).await;

		let response = public.get("/versions/2.60.0/artifacts").await;
		response.assert_status_ok();
		let artifacts: Vec<serde_json::Value> = response.json();

		assert_eq!(artifacts.len(), 1);
		assert_eq!(artifacts[0]["id"], UNSCOPED);
	})
	.await
}

/// A caller whose credential is bound to a machine has that machine's group,
/// and the artifact scoped to it displaces the unscoped one of the same type
/// and platform.
#[tokio::test(flavor = "multi_thread")]
async fn a_machine_is_offered_its_own_group_s_artifact() {
	commons_tests::server::run_with_device_auth(
		"machine",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn).await;
			enrol(&mut conn, device_id, GROUP_A).await;

			let response = public
				.get("/versions/2.60.0/artifacts")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			response.assert_status_ok();
			let artifacts: Vec<serde_json::Value> = response.json();

			assert_eq!(artifacts.len(), 1, "never offered both");
			assert_eq!(artifacts[0]["id"], THEIRS);

			// A caller is offered one artifact per type and platform. Which
			// group it belongs to is how Canopy chose it, not part of what
			// the caller is told.
			// spec: ART#what-a-version-offers
			assert!(
				artifacts[0].get("group_id").is_none(),
				"the scope is not disclosed, but got {}",
				artifacts[0]
			);
		},
	)
	.await
}

/// A machine in another group reaches the unscoped artifact, and the one it is
/// not offered is answered as though it did not exist. The refusal is the same
/// one an artifact id that was never registered gets, so which groups hold an
/// artifact is not enumerable here.
#[tokio::test(flavor = "multi_thread")]
async fn another_group_cannot_tell_the_artifact_apart_from_a_missing_one() {
	commons_tests::server::run_with_device_auth(
		"machine",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn).await;
			enrol(&mut conn, device_id, GROUP_B).await;

			let response = public
				.get("/versions/2.60.0/artifacts")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			response.assert_status_ok();
			let artifacts: Vec<serde_json::Value> = response.json();
			assert_eq!(artifacts.len(), 1);
			assert_eq!(artifacts[0]["id"], UNSCOPED, "not group A's");

			// The one it is not offered, by its real id.
			let refused = public
				.get(&format!("/versions/2.60.0/artifacts/{THEIRS}/download"))
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;

			// An id that was never registered at all.
			let absent = public
				.get("/versions/2.60.0/artifacts/99999999-9999-9999-9999-999999999999/download")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;

			assert_eq!(
				refused.status_code(),
				absent.status_code(),
				"an artifact held for another group must answer exactly as a missing one does"
			);
			assert_eq!(refused.status_code(), StatusCode::NOT_FOUND);
			assert_eq!(refused.text(), absent.text(), "and say the same thing");
		},
	)
	.await
}

/// Canopy serves the bytes it holds to a caller the artifact is offered to.
#[tokio::test(flavor = "multi_thread")]
async fn the_owning_group_is_served_the_held_bytes() {
	commons_tests::server::run_with_device_auth(
		"machine",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn).await;
			enrol(&mut conn, device_id, GROUP_A).await;

			let response = public
				.get(&format!("/versions/2.60.0/artifacts/{THEIRS}/download"))
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;

			response.assert_status_ok();
			assert_eq!(response.text(), "group a schema");
		},
	)
	.await
}

/// Canopy verifies the bytes it holds against the recorded digest as it serves
/// them, so a corrupted artifact fails the read rather than reaching a server
/// as the artifact it is not.
#[tokio::test(flavor = "multi_thread")]
async fn corrupted_bytes_fail_the_read() {
	commons_tests::server::run_with_device_auth(
		"machine",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn).await;
			enrol(&mut conn, device_id, GROUP_A).await;

			conn.batch_execute(&format!(
				"UPDATE artifacts SET content = 'tampered'::bytea WHERE id = '{THEIRS}'"
			))
			.await
			.expect("corrupt the stored bytes");

			let response = public
				.get(&format!("/versions/2.60.0/artifacts/{THEIRS}/download"))
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;

			assert_eq!(response.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
			assert!(!response.text().contains("tampered"));
		},
	)
	.await
}

/// A releaser credential carries no authorisation for any group, so a
/// registration naming one is refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_releaser_cannot_register_for_a_group() {
	commons_tests::server::run_with_device_auth(
		"releaser",
		async |mut conn, cert, _device_id, public, _| {
			seed(&mut conn).await;

			let response = public
				.post(&format!(
					"/artifacts/groups/{GROUP_A}/2.60.0/reporting-schema/any"
				))
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.text("https://example.com/x.sql")
				.await;

			assert_eq!(response.status_code(), StatusCode::FORBIDDEN);

			// The same registration without a group is accepted.
			let response = public
				.post("/artifacts/2.60.0/installer/windows")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.text("https://example.com/x.exe")
				.await;
			response.assert_status_ok();
		},
	)
	.await
}

/// The same read over the client-certificate header the live ingress sets.
/// Every other test here runs the Envoy path the harness selects by default,
/// which is not what is deployed.
#[tokio::test(flavor = "multi_thread")]
async fn the_owning_group_is_served_over_the_nginx_header() {
	commons_tests::server::run_with_device_auth_on(
		commons_servers::device_auth::mtls::ClientCertHeader::Mtls,
		"machine",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn).await;
			enrol(&mut conn, device_id, GROUP_A).await;

			let response = public
				.get("/versions/2.60.0/artifacts")
				.add_header("mtls-certificate", &cert)
				.await;
			response.assert_status_ok();
			let artifacts: Vec<serde_json::Value> = response.json();

			assert_eq!(artifacts.len(), 1);
			assert_eq!(artifacts[0]["id"], THEIRS);
		},
	)
	.await
}

/// The listing hands out a URL that fetches the bytes, including where the
/// caller named a range rather than the resolved version. Building the path by
/// hand instead leaves the synthesis untested.
#[tokio::test(flavor = "multi_thread")]
async fn the_offered_download_url_fetches_the_bytes() {
	commons_tests::server::run_with_device_auth(
		"machine",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn).await;
			enrol(&mut conn, device_id, GROUP_A).await;

			let response = public
				.get("/versions/2.60.x/artifacts")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			response.assert_status_ok();
			let artifacts: Vec<serde_json::Value> = response.json();
			let url = artifacts[0]["download_url"]
				.as_str()
				.expect("a download url")
				.to_owned();

			assert!(
				url.ends_with(&format!("/versions/2.60.0/artifacts/{THEIRS}/download")),
				"names the resolved version, not the range asked for: {url}"
			);

			let path = url.split_once("://").map_or(url.as_str(), |(_, rest)| {
				rest.split_once('/').map_or("", |(_, path)| path)
			});
			let fetched = public
				.get(&format!("/{path}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			fetched.assert_status_ok();
			assert_eq!(fetched.text(), "group a schema");
		},
	)
	.await
}

/// An artifact of another version is missing in the same way one held for
/// another group is, so the three refusals a caller can provoke are not
/// distinguishable from each other.
#[tokio::test(flavor = "multi_thread")]
async fn an_artifact_of_another_version_is_refused_identically() {
	commons_tests::server::run_with_device_auth(
		"machine",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn).await;
			enrol(&mut conn, device_id, GROUP_A).await;

			conn.batch_execute(
				"INSERT INTO versions (id, major, minor, patch, changelog, status)
				 VALUES ('44444444-4444-4444-4444-444444444444', 2, 59, 0, '', 'published');

				 INSERT INTO artifacts (id, version_id, platform, artifact_type, download_url)
				 VALUES ('55555555-5555-5555-5555-555555555555',
				         '44444444-4444-4444-4444-444444444444', 'any', 'installer',
				         'https://example.com/old.exe')",
			)
			.await
			.expect("seed another version");

			let elsewhere = public
				.get("/versions/2.60.0/artifacts/55555555-5555-5555-5555-555555555555/download")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			let absent = public
				.get("/versions/2.60.0/artifacts/99999999-9999-9999-9999-999999999999/download")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;

			assert_eq!(elsewhere.status_code(), StatusCode::NOT_FOUND);
			assert_eq!(elsewhere.status_code(), absent.status_code());
			assert_eq!(elsewhere.text(), absent.text());
		},
	)
	.await
}

/// A device Canopy can place but which sits on no machine, and one on a machine
/// with no group, are both answered as an anonymous caller is rather than
/// refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_device_with_no_group_is_answered_anonymously() {
	commons_tests::server::run_with_device_auth(
		"machine",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn).await;

			// No machine at all.
			let response = public
				.get("/versions/2.60.0/artifacts")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			response.assert_status_ok();
			let artifacts: Vec<serde_json::Value> = response.json();
			assert_eq!(artifacts.len(), 1);
			assert_eq!(artifacts[0]["id"], UNSCOPED);

			// On a machine, but the machine belongs to no group.
			conn.batch_execute(&format!(
				"INSERT INTO machines (name, device_id) VALUES ('ungrouped', '{device_id}')"
			))
			.await
			.expect("enrol without a group");

			let response = public
				.get("/versions/2.60.0/artifacts")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			response.assert_status_ok();
			let artifacts: Vec<serde_json::Value> = response.json();
			assert_eq!(artifacts.len(), 1);
			assert_eq!(artifacts[0]["id"], UNSCOPED);
		},
	)
	.await
}

/// The public pages and the release feed are read by anyone, so they resolve
/// unscoped whoever asks. A caller presenting the owning group's credential
/// still sees no trace of the artifact Canopy holds for it.
// spec: ART#who-is-offered-a-group-scoped-artifact
#[tokio::test(flavor = "multi_thread")]
async fn the_public_pages_never_carry_a_group_s_artifact() {
	commons_tests::server::run_with_device_auth(
		"machine",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn).await;
			enrol(&mut conn, device_id, GROUP_A).await;

			for path in [
				"/versions/2.60.0",
				"/versions/2.60.0/mobile",
				"/versions/rss",
			] {
				let response = public
					.get(path)
					.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
					.await;
				// A page that did not render carries no artifact either, which
				// would pass the assertions below for the wrong reason.
				response.assert_status_ok();
				let body = response.text();
				assert!(
					!body.contains(THEIRS),
					"{path} names the artifact held for group A"
				);
				assert!(
					!body.contains("group a schema"),
					"{path} carries the bytes held for group A"
				);
			}
		},
	)
	.await
}

/// A corrupted artifact fails the read as itself, so an operator reading the
/// problem type is told the bytes no longer match rather than being left with
/// an unclassified fault.
// spec: ART#digests
#[tokio::test(flavor = "multi_thread")]
async fn a_digest_mismatch_says_what_it_is() {
	commons_tests::server::run_with_device_auth(
		"machine",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn).await;
			enrol(&mut conn, device_id, GROUP_A).await;

			conn.batch_execute(&format!(
				"UPDATE artifacts SET content = 'tampered'::bytea WHERE id = '{THEIRS}'"
			))
			.await
			.expect("corrupt the stored bytes");

			let response = public
				.get(&format!("/versions/2.60.0/artifacts/{THEIRS}/download"))
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;

			assert_eq!(response.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
			let problem: serde_json::Value = response.json();
			assert_eq!(problem["type"], "/errors/artifact-digest-mismatch");
		},
	)
	.await
}

/// A registration through the endpoint replaces what stood for the same version,
/// type and platform, so a caller is never offered two of a kind.
// spec: ART#registration
#[tokio::test(flavor = "multi_thread")]
async fn registering_again_over_the_wire_replaces() {
	commons_tests::server::run_with_device_auth(
		"releaser",
		async |mut conn, cert, _device_id, public, _| {
			seed(&mut conn).await;

			let first = public
				.post("/artifacts/2.60.0/installer/windows")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.text("https://example.com/first.exe")
				.await;
			first.assert_status_ok();
			let first: serde_json::Value = first.json();

			let second = public
				.post("/artifacts/2.60.0/installer/windows")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.text("https://example.com/second.exe")
				.await;
			second.assert_status_ok();
			let second: serde_json::Value = second.json();

			assert_eq!(first["id"], second["id"], "replaced in place");
			assert_eq!(second["download_url"], "https://example.com/second.exe");

			let listed = public.get("/versions/2.60.0/artifacts").await;
			let artifacts: Vec<serde_json::Value> = listed.json();
			assert_eq!(
				artifacts
					.iter()
					.filter(|a| a["artifact_type"] == "installer")
					.count(),
				1,
			);
		},
	)
	.await
}

/// A credential Canopy cannot place is anonymous rather than refused, so a
/// deactivated key still reads the unscoped artifacts instead of failing a path
/// that serves everyone. The same credential registering is still a refusal:
/// the downgrade widens nothing.
// spec: ART#who-is-offered-a-group-scoped-artifact
#[tokio::test(flavor = "multi_thread")]
async fn a_deactivated_key_reads_as_anonymous_and_still_cannot_register() {
	commons_tests::server::run_with_device_auth(
		"releaser",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn).await;
			enrol(&mut conn, device_id, GROUP_A).await;

			conn.batch_execute(&format!(
				"UPDATE device_keys SET is_active = false WHERE device_id = '{device_id}'"
			))
			.await
			.expect("deactivate the key");

			// The read still answers, with the unscoped set rather than group A's.
			let response = public
				.get("/versions/2.60.0/artifacts")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			response.assert_status_ok();
			let artifacts: Vec<serde_json::Value> = response.json();
			assert_eq!(artifacts.len(), 1);
			assert_eq!(artifacts[0]["id"], UNSCOPED);

			// Registering with the same credential is refused: a path that
			// needs an identity does not accept one Canopy cannot place.
			let refused = public
				.post("/artifacts/2.60.0/installer/windows")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.text("https://example.com/x.exe")
				.await;
			assert_eq!(refused.status_code(), StatusCode::UNAUTHORIZED);
		},
	)
	.await
}

/// The media type a registration recorded is what the bytes are served as, and
/// an artifact registered without one is served as opaque bytes rather than
/// guessed at.
// spec: ART#where-an-artifact-rests
#[tokio::test(flavor = "multi_thread")]
async fn held_bytes_are_served_as_the_type_they_were_registered_with() {
	commons_tests::server::run_with_device_auth(
		"machine",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn).await;
			enrol(&mut conn, device_id, GROUP_A).await;

			let typed = public
				.get(&format!("/versions/2.60.0/artifacts/{THEIRS}/download"))
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			assert_eq!(
				typed.header("content-type").to_str().unwrap(),
				"application/sql"
			);

			conn.batch_execute(&format!(
				"UPDATE artifacts SET content_type = NULL WHERE id = '{THEIRS}'"
			))
			.await
			.expect("drop the media type");

			let untyped = public
				.get(&format!("/versions/2.60.0/artifacts/{THEIRS}/download"))
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			assert_eq!(
				untyped.header("content-type").to_str().unwrap(),
				"application/octet-stream"
			);
		},
	)
	.await
}

/// Held bytes come off the same origin as this server's own HTML pages, and
/// the media type is whatever the registration named, so a schema registered
/// as `text/html` would otherwise render as same-origin content.
// spec: ART#where-an-artifact-rests
#[tokio::test(flavor = "multi_thread")]
async fn held_bytes_are_never_rendered_by_a_browser() {
	commons_tests::server::run_with_device_auth(
		"machine",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn).await;
			enrol(&mut conn, device_id, GROUP_A).await;

			conn.batch_execute(&format!(
				"UPDATE artifacts SET content_type = 'text/html' WHERE id = '{THEIRS}'"
			))
			.await
			.expect("set the media type");

			let served = public
				.get(&format!("/versions/2.60.0/artifacts/{THEIRS}/download"))
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;

			assert_eq!(
				served.header("x-content-type-options").to_str().unwrap(),
				"nosniff"
			);
			assert!(
				served
					.header("content-disposition")
					.to_str()
					.unwrap()
					.starts_with("attachment"),
				"held bytes are downloaded, not displayed"
			);
		},
	)
	.await
}

/// An operator device registers either kind, and needs no declaration to name a
/// group: the authorisation a builder holds is what a component has instead of
/// being an operator, not a narrower form of it.
// spec: ART#registration
#[tokio::test(flavor = "multi_thread")]
async fn an_admin_device_registers_for_any_group() {
	commons_tests::server::run_with_device_auth(
		"admin",
		async |mut conn, cert, _device_id, public, _| {
			seed(&mut conn).await;

			let scoped = public
				.post(&format!(
					"/artifacts/groups/{GROUP_A}/2.60.0/reporting-schema/any"
				))
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.add_header("content-type", "application/sql")
				.text("CREATE VIEW ...")
				.await;
			scoped.assert_status_ok();
			let scoped: serde_json::Value = scoped.json();
			assert!(
				scoped["digest"].is_string(),
				"the bytes are held, so Canopy digests them"
			);

			let accepted = public
				.post("/artifacts/2.60.0/installer/windows")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.text("https://example.com/x.exe")
				.await;
			accepted.assert_status_ok();
		},
	)
	.await
}

/// A registration carrying no location, and one naming a group that is not a
/// group at all, are both the registrant's own mistake and are refused as one.
/// A blank body would otherwise pass the constraint, which only tests for NULL,
/// and leave an artifact nothing can be fetched from.
// spec: ART#registration, ART#where-an-artifact-rests
#[tokio::test(flavor = "multi_thread")]
async fn a_registration_with_nothing_in_it_is_refused() {
	commons_tests::server::run_with_device_auth(
		"releaser",
		async |mut conn, cert, _device_id, public, _| {
			seed(&mut conn).await;

			for body in ["", "   "] {
				let response = public
					.post("/artifacts/2.60.0/installer/windows")
					.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
					.text(body)
					.await;
				assert_eq!(
					response.status_code(),
					StatusCode::BAD_REQUEST,
					"a body of {body:?} is no location"
				);
			}

			let malformed = public
				.post("/artifacts/groups/not-a-uuid/2.60.0/reporting-schema/any")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.text("CREATE VIEW ...")
				.await;
			assert_eq!(
				malformed.status_code(),
				StatusCode::BAD_REQUEST,
				"a group that is not a uuid is a client mistake, not a 500"
			);

			// The version a registration names is drafted so the artifact has
			// something to attach to, so a refusal after that point leaves one
			// standing for an artifact that was never registered.
			let unknown = public
				.post("/artifacts/9.9.9/installer/windows")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.text("  ")
				.await;
			assert_eq!(unknown.status_code(), StatusCode::BAD_REQUEST);
			assert!(
				database::versions::Version::get_by_version(
					&mut conn,
					"9.9.9".parse().expect("a version"),
				)
				.await
				.is_err(),
				"a refused registration drafts no version"
			);

			// Nothing was written by any of them.
			let listed = public.get("/versions/2.60.0/artifacts").await;
			let artifacts: Vec<serde_json::Value> = listed.json();
			assert!(
				!artifacts.iter().any(|a| a["artifact_type"] == "installer"),
				"nothing blank or malformed was registered"
			);
		},
	)
	.await
}

/// Archiving a machine takes its group with it. The device is unbound and its
/// keys deactivated in one transaction, so the credential that was offered the
/// group's artifact reads as no identity at all afterwards rather than keeping
/// the group the box used to be in.
// spec: ART#who-is-offered-a-group-scoped-artifact, FLT#archival
#[tokio::test(flavor = "multi_thread")]
async fn an_archived_machine_s_credential_keeps_no_group() {
	commons_tests::server::run_with_device_auth(
		"machine",
		async |mut conn, cert, device_id, public, _| {
			const MACHINE: &str = "cccccccc-cccc-cccc-cccc-cccccccccccc";
			seed(&mut conn).await;
			conn.batch_execute(&format!(
				"INSERT INTO machines (id, name, group_id, device_id)
				 VALUES ('{MACHINE}', 'box', '{GROUP_A}', '{device_id}')"
			))
			.await
			.expect("enrol machine");

			let before = public
				.get("/versions/2.60.0/artifacts")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			let artifacts: Vec<serde_json::Value> = before.json();
			assert_eq!(artifacts[0]["id"], THEIRS, "its own group's, to start with");

			database::machines::Machine::archive(&mut conn, MACHINE.parse().unwrap())
				.await
				.expect("archive");

			let after = public
				.get("/versions/2.60.0/artifacts")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			after.assert_status_ok();
			let artifacts: Vec<serde_json::Value> = after.json();
			assert_eq!(artifacts.len(), 1);
			assert_eq!(
				artifacts[0]["id"], UNSCOPED,
				"answered as a read carrying no identity is"
			);

			let refused = public
				.get(&format!("/versions/2.60.0/artifacts/{THEIRS}/download"))
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			assert_eq!(
				refused.status_code(),
				StatusCode::NOT_FOUND,
				"and the bytes it used to be served are out of reach"
			);
		},
	)
	.await
}

/// A read that serves everyone downgrades a credential it cannot place to
/// anonymous. A fault reaching that decision is not a credential it cannot
/// place: answered anonymously it hands a machine that has a group the
/// unscoped set and presents it as that machine's answer.
// spec: ART#who-is-offered-a-group-scoped-artifact
#[tokio::test(flavor = "multi_thread")]
async fn a_fault_placing_a_credential_is_not_anonymity() {
	commons_tests::server::run_with_device_auth(
		"machine",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn).await;
			enrol(&mut conn, device_id, GROUP_A).await;

			// Resolving an identity reads device_keys. Taking the table away is
			// the only way from here to make that read fail rather than miss.
			conn.batch_execute("DROP TABLE device_keys CASCADE")
				.await
				.expect("drop device_keys");

			let response = public
				.get("/versions/2.60.0/artifacts")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;

			assert_eq!(
				response.status_code(),
				StatusCode::INTERNAL_SERVER_ERROR,
				"a caller that cannot be placed is told so, not answered as nobody"
			);
		},
	)
	.await
}

/// An unscoped artifact is read from its location by the caller rather than by
/// Canopy, so its digest is what that caller checks what it fetched against.
/// A releaser records one where it has one, and an artifact registered without
/// one is fetched unchecked rather than fetched against a blank.
// spec: ART#digests
#[tokio::test(flavor = "multi_thread")]
async fn a_releaser_records_the_digest_it_publishes() {
	commons_tests::server::run_with_device_auth(
		"releaser",
		async |mut conn, cert, _device_id, public, _| {
			seed(&mut conn).await;

			let claimed = sri(&digest_of(b"the installer"));
			let recorded = public
				.post("/artifacts/2.60.0/installer/windows")
				.add_query_param("digest", &claimed)
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.text("https://example.com/x.exe")
				.await;
			recorded.assert_status_ok();
			let recorded: serde_json::Value = recorded.json();
			assert_eq!(recorded["digest"], claimed);

			for query in ["", "?digest=", "?digest=%20%20"] {
				let response = public
					.post(&format!("/artifacts/2.60.0/installer/linux{query}"))
					.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
					.text("https://example.com/x.deb")
					.await;
				response.assert_status_ok();
				let artifact: serde_json::Value = response.json();
				assert!(
					artifact["digest"].is_null(),
					"a blank digest is no digest, but got {artifact}"
				);
			}

			// A digest nothing can check the bytes against is refused rather
			// than published to every device that fetches the artifact.
			for claimed in ["sha256:abcd", "notadigest", "sha256-abcd"] {
				let response = public
					.post("/artifacts/2.60.0/installer/macos")
					.add_query_param("digest", claimed)
					.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
					.text("https://example.com/x.dmg")
					.await;
				assert_eq!(
					response.status_code(),
					StatusCode::BAD_REQUEST,
					"{claimed:?} is not a digest"
				);
			}
		},
	)
	.await
}

/// A device is offered the artifacts of the version it reports running, and a
/// known issue on that version does not withhold them. A known issue says a
/// version is not one to move to, and the versions carrying one are exactly the
/// ones a fleet is still sitting on and still needs a schema for. A range is a
/// question about where to go, so it still resolves past the version.
// spec: ART#what-a-version-offers
#[tokio::test(flavor = "multi_thread")]
async fn a_known_issue_does_not_withhold_a_version_s_own_artifacts() {
	commons_tests::server::run_with_device_auth(
		"machine",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn).await;
			enrol(&mut conn, device_id, GROUP_A).await;

			// An open known issue has no upper bound, so it covers 2.60.0 and
			// every later patch of that line.
			conn.batch_execute(
				"INSERT INTO version_known_issues
				 (author, description, min_major, min_minor, min_patch)
				 VALUES ('admin', 'broken', 2, 60, 0)",
			)
			.await
			.expect("seed a known issue over 2.60.0");

			let offered = public
				.get("/versions/2.60.0/artifacts")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			offered.assert_status_ok();
			let artifacts: Vec<serde_json::Value> = offered.json();
			assert_eq!(artifacts.len(), 1);
			assert_eq!(
				artifacts[0]["id"], THEIRS,
				"the group's own schema for the version it is on"
			);

			let bytes = public
				.get(&format!("/versions/2.60.0/artifacts/{THEIRS}/download"))
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			bytes.assert_status_ok();
			assert_eq!(bytes.text(), "group a schema");

			// The same known issue still keeps a range off that version.
			let ranged = public
				.get("/versions/2.60.x/artifacts")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			assert_eq!(
				ranged.status_code(),
				StatusCode::NOT_FOUND,
				"a range has nothing ready to land on"
			);
		},
	)
	.await
}

/// A URL Canopy handed out keeps working after a more specific artifact of the
/// same type and platform is registered. Fetchability is what a caller may see,
/// not which artifact currently wins the offer.
// spec: ART#who-is-offered-a-group-scoped-artifact
#[tokio::test(flavor = "multi_thread")]
async fn a_displaced_artifact_is_still_fetchable() {
	commons_tests::server::run_with_device_auth(
		"machine",
		async |mut conn, cert, device_id, public, _| {
			seed(&mut conn).await;
			enrol(&mut conn, device_id, GROUP_A).await;

			// A range artifact of group A's, which group A's exact one displaces.
			let ranged = "44444444-4444-4444-4444-444444444444";
			let digest = hex::encode(digest_of(b"the range schema"));
			conn.batch_execute(&format!(
				"INSERT INTO artifacts
				   (id, version_id, platform, artifact_type, version_range_pattern, group_id, content, content_type, digest)
				 VALUES ('{ranged}', NULL, 'any', 'reporting-schema', '2.60.x', '{GROUP_A}',
				         'the range schema'::bytea, 'application/sql', '\\x{digest}'::bytea)"
			))
			.await
			.expect("seed the range artifact");

			let listed = public
				.get("/versions/2.60.0/artifacts")
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			listed.assert_status_ok();
			let artifacts: Vec<serde_json::Value> = listed.json();
			assert_eq!(artifacts.len(), 1, "one artifact per type and platform");
			assert_eq!(artifacts[0]["id"], THEIRS, "the exact one is offered");

			// The one specificity passed over is still fetchable by the URL it
			// was offered under before.
			let download = public
				.get(&format!("/versions/2.60.0/artifacts/{ranged}/download"))
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.await;
			download.assert_status_ok();
			assert_eq!(download.text(), "the range schema");
		},
	)
	.await
}
