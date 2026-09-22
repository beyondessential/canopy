//! Local-development database seeder.
//!
//! Populates a freshly-migrated database with a broad, obviously-fake spread of
//! data so the admin UI (and the new server-enrollment flow in particular) has
//! something to render against. LOCAL DEV ONLY — it truncates the app tables it
//! manages before reinserting, so it's safe to re-run, but it must never point
//! at a real Canopy instance.
//!
//! Run via `just seed`. It reuses the model functions in this crate wherever
//! they exist (so the seed tracks the schema and the real code paths), and
//! falls back to direct diesel inserts for the few rows that have no public
//! constructor (statuses with backdated timestamps, device keys/connections,
//! published versions, etc).

use commons_errors::{AppError, Result};
use commons_types::{
	device::DeviceRole,
	geo::GeoPoint,
	issue::ResolvedReason,
	namespace::Namespace,
	server::{TagMap, app_type::ApplicationType, rank::ServerRank},
	status::CheckResult,
	version::{VersionStatus, VersionStr},
};
use database::{
	Device, DeviceKey,
	admins::Admin,
	applications::Application,
	check_policies::CheckPolicy,
	devices::NewDeviceConnection,
	issues::{Incident, Issue, NewEvent},
	machine_enrollment_tokens::MachineEnrollmentToken,
	machines::{Machine, NewMachine},
	notes::{IncidentNote, IssueNote},
	pg_duration::PgDuration,
	server_groups::{NewServerGroup, ServerGroup},
	silenced_refs::{ServerGroupSilencedRef, ServerSilencedRef},
	statuses::Status,
	url_field::UrlField,
	version_known_issues::VersionKnownIssue,
	versions::NewVersion,
};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use jiff::{SignedDuration, Timestamp};
use std::collections::BTreeMap;
use uuid::Uuid;

const TEN_MINUTES: PgDuration = PgDuration(SignedDuration::from_secs(600));

/// Tables this seeder owns. Truncated (CASCADE) at the start of every run so a
/// re-run produces a clean, deterministic dataset. The migrations table and the
/// partition machinery are deliberately absent — we never touch schema.
///
/// `applications` is excluded from the blanket TRUNCATE because the nil "meta"
/// server row (id all-zeroes) is inserted by a migration and must survive; we
/// delete only the non-nil rows below.
const TRUNCATE_TABLES: &[&str] = &[
	"machine_enrollment_challenges",
	"machine_enrollment_tokens",
	"slack_outbox",
	"incident_notes",
	"issue_notes",
	"incident_issues",
	"incidents",
	"issues",
	"scoped_check_policies",
	"device_connections",
	"device_keys",
	"devices",
	"server_groups",
	"version_known_issues",
	"versions",
	"check_policies",
	"admins",
];

fn tags(pairs: &[(&str, &str)]) -> TagMap {
	let mut m = BTreeMap::new();
	for (k, v) in pairs {
		m.insert((*k).to_string(), (*v).to_string());
	}
	TagMap(m)
}

fn url(s: &str) -> UrlField {
	UrlField(s.parse().expect("seed URL parses"))
}

/// Refuse to run against anything that looks like a real Canopy instance.
fn guard_database_url(database_url: &str) -> Result<()> {
	let lower = database_url.to_ascii_lowercase();
	let looks_prod = [
		"prod",
		"production",
		".rds.",
		"amazonaws",
		"azure",
		"googleapis",
		"supabase",
		"neon.tech",
		"render.com",
		"canopy-dev",
		"bes.au",
	]
	.iter()
	.find(|needle| lower.contains(**needle));

	if let Some(needle) = looks_prod {
		return Err(AppError::custom(format!(
			"refusing to seed: DATABASE_URL looks like a real Canopy instance (matched {needle:?}). \
			 The seeder is LOCAL DEV ONLY and truncates application tables."
		)));
	}
	Ok(())
}

#[tokio::main]
async fn main() -> miette::Result<()> {
	let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");

	eprintln!("┌─────────────────────────────────────────────────────────────┐");
	eprintln!("│  canopy seed — LOCAL DEVELOPMENT ONLY                        │");
	eprintln!("│  Truncates and repopulates application tables.              │");
	eprintln!("│  Never run this against a real Canopy instance.             │");
	eprintln!("└─────────────────────────────────────────────────────────────┘");

	// Release builds are for real instances; the seeder is a dev-only tool that
	// truncates tables, so refuse to run unless built with debug assertions.
	if !cfg!(debug_assertions) {
		eprintln!(
			"error: refusing to seed: this binary was compiled in release mode. \
			 The seeder is LOCAL DEV ONLY; run it via `just seed` (a debug build)."
		);
		std::process::exit(1);
	}

	if let Err(e) = guard_database_url(&database_url) {
		eprintln!("error: {e}");
		std::process::exit(1);
	}

	let mut conn = AsyncPgConnection::establish(&database_url)
		.await
		.map_err(|e| miette::miette!("failed to connect to {database_url}: {e}"))?;

	seed(&mut conn).await.map_err(|e| miette::miette!("{e}"))?;

	eprintln!("seed: done.");
	Ok(())
}

async fn seed(conn: &mut AsyncPgConnection) -> Result<()> {
	clear(conn).await?;

	let admins = seed_admins(conn).await?;
	let versions = seed_versions(conn).await?;
	seed_known_issues(conn).await?;
	seed_healthchecks(conn, &admins).await?;

	let devices = seed_devices(conn).await?;
	let groups = seed_groups(conn).await?;
	let applications = seed_servers(conn, &groups, &devices).await?;
	seed_enrollment_tokens(conn, &applications).await?;
	seed_statuses(conn, &applications, &devices, &versions).await?;
	seed_issues_and_incidents(conn, &applications, &admins).await?;
	seed_silences(conn, &applications, &groups, &admins).await?;

	report(conn).await
}

/// TRUNCATE every app table we manage, then delete the non-nil applications. We
/// can't TRUNCATE `applications` because of the nil meta row, and CASCADE from the
/// truncated children already cleared everything that referenced these rows.
async fn clear(conn: &mut AsyncPgConnection) -> Result<()> {
	conn.transaction::<_, AppError, _>(async |conn| {
		// Drop every application except the nil meta row (inserted by a
		// migration) BEFORE truncating `devices` — a TRUNCATE ... CASCADE
		// would otherwise demand truncating `applications` wholesale (taking
		// the nil row with it) to satisfy the FK.
		diesel::sql_query(
			"DELETE FROM applications WHERE id <> '00000000-0000-0000-0000-000000000000'",
		)
		.execute(conn)
		.await?;

		let stmt = format!("TRUNCATE TABLE {} CASCADE", TRUNCATE_TABLES.join(", "));
		diesel::sql_query(stmt).execute(conn).await?;
		Ok(())
	})
	.await
}

async fn seed_admins(conn: &mut AsyncPgConnection) -> Result<Vec<String>> {
	let emails = [
		"alice.operator@example.com",
		"bob.oncall@example.com",
		"carol.releases@example.com",
	];
	for email in emails {
		Admin::add(conn, email).await?;
	}
	Ok(emails.iter().map(|s| s.to_string()).collect())
}

/// A spread of published versions plus a draft and a yanked one, so the
/// versions UI shows every status and the version-distance maths has something
/// to chew on.
async fn seed_versions(conn: &mut AsyncPgConnection) -> Result<Vec<VersionStr>> {
	use database::schema::versions;

	let rows = [
		(
			2,
			8,
			0,
			VersionStatus::Published,
			"2.8.0 — baseline release.",
		),
		(
			2,
			9,
			0,
			VersionStatus::Published,
			"2.9.0 — sync throughput improvements.",
		),
		(
			2,
			9,
			3,
			VersionStatus::Published,
			"2.9.3 — patch: fix migration ordering bug.",
		),
		(
			2,
			10,
			0,
			VersionStatus::Published,
			"2.10.0 — new facility dashboard.",
		),
		(
			2,
			10,
			5,
			VersionStatus::Published,
			"2.10.5 — patch: harden auth token refresh.",
		),
		(
			2,
			11,
			0,
			VersionStatus::Yanked,
			"2.11.0 — YANKED: regressed report exports.",
		),
		(
			2,
			12,
			0,
			VersionStatus::Draft,
			"2.12.0 — draft: experimental offline mode.",
		),
	];

	let mut out = Vec::new();
	for (major, minor, patch, status, changelog) in rows {
		diesel::insert_into(versions::table)
			.values(NewVersion {
				major,
				minor,
				patch,
				status,
				changelog: changelog.to_string(),
				device_id: None,
			})
			.execute(conn)
			.await?;
		out.push(VersionStr(node_semver::Version::new(
			major as u64,
			minor as u64,
			patch as u64,
		)));
	}
	Ok(out)
}

/// One open known issue against 2.11.x and one already-resolved one against
/// 2.9.x, so the version-detail UI shows both states.
async fn seed_known_issues(conn: &mut AsyncPgConnection) -> Result<()> {
	VersionKnownIssue::add(
		conn,
		(2, 11, 0),
		"carol.releases@example.com",
		"Report exports time out on large datasets; avoid 2.11.x in production.",
		None,
	)
	.await?;

	let resolved = VersionKnownIssue::add(
		conn,
		(2, 9, 0),
		"carol.releases@example.com",
		"Sync stalls when a facility reconnects after a long outage.",
		None,
	)
	.await?;
	VersionKnownIssue::resolve(
		conn,
		resolved.id,
		(2, 9, 3),
		"carol.releases@example.com",
		"Fixed in 2.9.3 by reordering the reconnect handshake.",
	)
	.await?;
	Ok(())
}

/// Seed the healthcheck-severity catalog: a few reviewed entries, an
/// unreviewed default, and one carrying a conditional rules ladder.
async fn seed_healthchecks(conn: &mut AsyncPgConnection, admins: &[String]) -> Result<()> {
	// None of these is a machine-subject name, so each is one catalog entry per
	// application type reporting it — which is what the seeded fleet looks like.
	let namespaces = [
		Namespace::Application(ApplicationType::TamanuCentral),
		Namespace::Application(ApplicationType::TamanuFacility),
	];

	// upsert_default creates rows at the default severity (warning, unreviewed).
	for ns in &namespaces {
		for check in [
			"database_connectivity",
			"disk_space",
			"sync_lag",
			"certificate_expiry",
			"backup_freshness",
		] {
			CheckPolicy::upsert_default(conn, "alertd", ns, check).await?;
		}
	}

	let reviewer = &admins[0];
	for ns in &namespaces {
		CheckPolicy::update(
			conn,
			"alertd",
			ns,
			"database_connectivity",
			CheckResult::Failed,
			true,
			Some("DB down means the server is effectively offline."),
			reviewer,
		)
		.await?;
		CheckPolicy::update(
			conn,
			"alertd",
			ns,
			"disk_space",
			CheckResult::Failed,
			false,
			Some("Page when disk is critically low."),
			reviewer,
		)
		.await?;
		CheckPolicy::update(
			conn,
			"alertd",
			ns,
			"backup_freshness",
			CheckResult::Passed,
			false,
			None,
			reviewer,
		)
		.await?;
	}

	// A conditional rules ladder: grade sync_lag by how far behind it is.
	use database::check_policies::{Condition, IfLadder, Var};
	let ladder = IfLadder {
		branches: vec![
			(
				Condition::Gt(
					"check.lag_seconds".parse::<Var>().expect("var parses"),
					serde_json::json!(600),
				),
				CheckResult::Failed,
			),
			(
				Condition::Gt(
					"check.lag_seconds".parse::<Var>().expect("var parses"),
					serde_json::json!(60),
				),
				CheckResult::Warning,
			),
		],
	};
	for ns in &namespaces {
		CheckPolicy::update_rules(conn, "alertd", ns, "sync_lag", Some(&ladder), reviewer).await?;
	}

	Ok(())
}

/// Identities seeded into `devices`, returned for wiring applications/statuses.
struct SeededDevices {
	/// Trusted server devices, mTLS-keyed, bound to registered applications.
	mtls_server: Vec<Uuid>,
	/// A trusted server device that also carries a Tailscale identity.
	tailscale_server: Uuid,
	/// An admin device authenticating over Tailscale only (no mTLS key).
	tailscale_admin: Uuid,
	/// A trusted releaser device (mTLS).
	releaser: Uuid,
}

async fn seed_devices(conn: &mut AsyncPgConnection) -> Result<SeededDevices> {
	// Deterministic fake key material: not real cryptographic keys, just stable
	// bytes so the device_keys rows differ and search-by-key has something to
	// match. The seeder never authenticates, so validity doesn't matter.
	fn fake_key(tag: u8) -> Vec<u8> {
		let mut k = vec![0x30, 0x59, 0x30, 0x13]; // SPKI-ish prefix so it reads as DER-y
		k.extend(std::iter::repeat(tag).take(60));
		k
	}

	// Three mTLS-keyed server devices, promoted to the Application role.
	let mut mtls_server = Vec::new();
	for i in 0..3u8 {
		let dev = Device::create(conn, fake_key(0x10 + i)).await?;
		Device::trust(conn, dev.id, DeviceRole::Machine).await?;
		mtls_server.push(dev.id);
	}

	// A server device with both an mTLS key AND a Tailscale identity.
	let tailscale_server = {
		let dev = Device::create(conn, fake_key(0x20)).await?;
		Device::trust(conn, dev.id, DeviceRole::Machine).await?;
		Device::attach_tailscale(
			conn,
			dev.id,
			database::devices::TailscaleIdentity {
				node_id: "nodekey:seed-central-01".to_string(),
				node_name: Some("central-01.example-tailnet.ts.net".to_string()),
				tailnet: Some("example-tailnet.ts.net".to_string()),
			},
		)
		.await?;
		dev.id
	};

	// An admin device authenticating over Tailscale only — no mTLS key.
	let tailscale_admin = {
		let dev = Device::create_with_tailscale(
			conn,
			database::devices::TailscaleIdentity {
				node_id: "nodekey:seed-operator-laptop".to_string(),
				node_name: Some("operator-laptop.example-tailnet.ts.net".to_string()),
				tailnet: Some("example-tailnet.ts.net".to_string()),
			},
			DeviceRole::Admin,
		)
		.await?;
		dev.id
	};

	// A releaser device (mTLS).
	let releaser = {
		let dev = Device::create(conn, fake_key(0x30)).await?;
		Device::trust(conn, dev.id, DeviceRole::Releaser).await?;
		dev.id
	};

	// Add a second, named key to one of the mTLS server devices so the device
	// detail page shows multiple keys. (add_key refuses a second active key, so
	// insert directly.)
	DeviceKey::create(
		conn,
		mtls_server[0],
		fake_key(0xA0),
		Some("rotated-2026 backup key".to_string()),
	)
	.await?;

	// A few connection rows so the device detail page shows connection history.
	seed_device_connections(conn, &mtls_server, tailscale_server).await?;

	Ok(SeededDevices {
		mtls_server,
		tailscale_server,
		tailscale_admin,
		releaser,
	})
}

async fn seed_device_connections(
	conn: &mut AsyncPgConnection,
	mtls_server: &[Uuid],
	tailscale_server: Uuid,
) -> Result<()> {
	let entries: &[(Uuid, &str, &str)] = &[
		(
			mtls_server[0],
			"203.0.113.10/32",
			"bestool/2.10.5 (Node.js/20.11.0; linux)",
		),
		(
			mtls_server[1],
			"198.51.100.22/32",
			"bestool/2.9.3 (Node.js/18.19.0; windows)",
		),
		(
			tailscale_server,
			"100.64.0.12/32",
			"bestool/2.10.0 (Node.js/20.11.0; linux)",
		),
	];
	for (device_id, ip, ua) in entries {
		NewDeviceConnection {
			device_id: *device_id,
			ip: ip.parse().expect("seed IP parses"),
			user_agent: Some((*ua).to_string()),
		}
		.create(conn)
		.await?;
	}
	Ok(())
}

/// Returns (named group ids). The last one is intentionally empty so the
/// "add a server to this group" flow is testable.
struct SeededGroups {
	pacific: Uuid,
	highlands: Uuid,
	demo: Uuid,
	empty: Uuid,
}

async fn seed_groups(conn: &mut AsyncPgConnection) -> Result<SeededGroups> {
	let pacific = ServerGroup::create(
		conn,
		NewServerGroup {
			name: "Pacific Region".to_string(),
			notes: "Production servers across the Pacific facilities.".to_string(),
			tags: tags(&[("region", "pacific"), ("tier", "production")]),
			slack_open_delay: Some(PgDuration(SignedDuration::from_secs(300))),
			slack_close_delay: None,
		},
	)
	.await?;

	let highlands = ServerGroup::create(
		conn,
		NewServerGroup {
			name: "Highlands Cluster".to_string(),
			notes: "Mixed production + clone for the highlands rollout.".to_string(),
			tags: tags(&[("region", "highlands")]),
			slack_open_delay: None,
			slack_close_delay: None,
		},
	)
	.await?;

	let demo = ServerGroup::create(
		conn,
		NewServerGroup {
			name: "Demo & Training".to_string(),
			notes: "Demo and training environments — noisy, low priority.".to_string(),
			tags: tags(&[("env", "demo")]),
			slack_open_delay: Some(PgDuration(SignedDuration::from_secs(1800))),
			slack_close_delay: None,
		},
	)
	.await?;

	let empty = ServerGroup::create(
		conn,
		NewServerGroup {
			name: "Unassigned (empty)".to_string(),
			notes: "Deliberately empty so adding the first server is testable.".to_string(),
			tags: TagMap::default(),
			slack_open_delay: None,
			slack_close_delay: None,
		},
	)
	.await?;

	Ok(SeededGroups {
		pacific: pacific.id,
		highlands: highlands.id,
		demo: demo.id,
		empty: empty.id,
	})
}

/// Handles onto the seeded applications, grouped by the role they play in later
/// seeding (statuses, issues).
struct SeededServers {
	/// Registered, monitored, grouped, healthy production central.
	healthy_central: Uuid,
	/// Registered, monitored, grouped — will get an open incident.
	unhealthy_facility: Uuid,
	/// Registered, monitored, grouped — "down" (no recent status).
	down_facility: Uuid,
	/// Registered, monitored, grouped — warning-level health.
	warning_facility: Uuid,
	/// Registered but unmonitored demo server.
	demo_server: Uuid,
	/// Ungrouped registered server.
	ungrouped: Uuid,
	/// Pending enrollment (registered_at NULL, device_id NULL) — gets a token.
	pending_with_token: Uuid,
	/// Pending enrollment, no token yet.
	pending_no_token: Uuid,
	/// A non-Tamanu server sharing a group with Tamanu ones: no version at
	/// all, and it must not become the group's headline version.
	senaite_lims: Uuid,
}

#[allow(clippy::too_many_arguments)]
async fn seed_servers(
	conn: &mut AsyncPgConnection,
	groups: &SeededGroups,
	devices: &SeededDevices,
) -> Result<SeededServers> {
	// Helper to build a fully-specified Application row and insert it via the
	// model. Each seeded application gets a machine of its own, 1:1, matching
	// what the operator create flow does and what the split's backfill
	// produced; `base_of` leaves `machine_id` unset for this to fill in.
	/// The identity, where the box has one, is bound to the machine: it speaks
	/// for the box, not for the workload.
	async fn insert(
		conn: &mut AsyncPgConnection,
		device_id: Option<Uuid>,
		server: Application,
	) -> Result<Uuid> {
		let machine = Machine::create(
			conn,
			NewMachine {
				name: server.name.clone(),
				group_id: server.group_id,
				cloud: server.cloud,
				geolocation: server.geolocation,
			},
		)
		.await?;
		if let Some(device_id) = device_id {
			Machine::bind_device(conn, machine.id, device_id).await?;
		}
		let created = Application::create(
			conn,
			Application {
				machine_id: Some(machine.id),
				..server
			},
		)
		.await?;
		Ok(created.id)
	}

	fn base(host: &str, r#type: ApplicationType) -> Application {
		base_of(r#type, host)
	}

	fn base_of(r#type: ApplicationType, host: &str) -> Application {
		Application {
			id: Uuid::new_v4(),
			name: None,
			host: Some(url(host)),
			r#type,
			rank: None,
			// Replaced by `insert`, which is the only path to the database.
			machine_id: Some(Uuid::nil()),
			kubernetes_cluster_id: None,
			reported_key: None,
			group_id: None,
			public_name: None,
			cloud: None,
			geolocation: None,
			is_monitored: true,
			alert_when_down_for: TEN_MINUTES,
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

	let now = Timestamp::now();

	// Registered, monitored, grouped, healthy production central.
	let healthy_central = insert(
		conn,
		Some(devices.tailscale_server),
		Application {
			name: Some("Pacific Central".to_string()),
			rank: Some(ServerRank::Production),
			group_id: Some(groups.pacific),
			public_name: Some("Pacific Central".to_string()),
			cloud: Some(true),
			geolocation: Some(GeoPoint {
				lat: -18.1416,
				lon: 178.4419,
			}),
			notes: "Primary central server for the Pacific region.".to_string(),
			tags: tags(&[("owner", "platform-team"), ("hosting", "ec2")]),
			registered_at: Some(now),
			..base(
				"https://pacific-central.example.com/",
				ApplicationType::TamanuCentral,
			)
		},
	)
	.await?;

	// Registered, monitored, grouped facility — will carry an open incident.
	let unhealthy_facility = insert(
		conn,
		Some(devices.mtls_server[0]),
		Application {
			name: Some("Suva Facility".to_string()),
			rank: Some(ServerRank::Production),
			group_id: Some(groups.pacific),
			cloud: Some(false),
			notes: "On-prem facility server at Suva hospital.".to_string(),
			tags: tags(&[("site", "suva")]),
			registered_at: Some(now),
			..base(
				"https://suva-facility.example.com/",
				ApplicationType::TamanuFacility,
			)
		},
	)
	.await?;

	// Registered, monitored, grouped facility that is "down" (no recent status).
	let down_facility = insert(
		conn,
		Some(devices.mtls_server[1]),
		Application {
			name: Some("Nadi Facility".to_string()),
			rank: Some(ServerRank::Production),
			group_id: Some(groups.pacific),
			notes: "Has not reported in a while — exercises the down state.".to_string(),
			tags: tags(&[("site", "nadi")]),
			registered_at: Some(now),
			..base(
				"https://nadi-facility.example.com/",
				ApplicationType::TamanuFacility,
			)
		},
	)
	.await?;

	// Registered, monitored, grouped facility with warning-level health.
	let warning_facility = insert(
		conn,
		Some(devices.mtls_server[2]),
		Application {
			name: Some("Goroka Facility".to_string()),
			rank: Some(ServerRank::Clone),
			group_id: Some(groups.highlands),
			notes: "Clone environment in the highlands.".to_string(),
			tags: tags(&[("site", "goroka")]),
			registered_at: Some(now),
			..base(
				"https://goroka-facility.example.com/",
				ApplicationType::TamanuFacility,
			)
		},
	)
	.await?;

	// Registered but unmonitored demo server.
	let demo_server = insert(
		conn,
		None,
		Application {
			name: Some("Demo Sandbox".to_string()),
			rank: Some(ServerRank::Demo),
			group_id: Some(groups.demo),
			is_monitored: false,
			notes: "Demo box — monitoring intentionally off.".to_string(),
			tags: tags(&[("env", "demo")]),
			registered_at: Some(now),
			..base(
				"https://demo-sandbox.example.com/",
				ApplicationType::TamanuCentral,
			)
		},
	)
	.await?;

	// Ungrouped registered server (test rank).
	let ungrouped = insert(
		conn,
		None,
		Application {
			name: Some("Lab Test Application".to_string()),
			rank: Some(ServerRank::Test),
			is_monitored: true,
			notes: "Ungrouped — reachable by id, not from the fleet listing.".to_string(),
			..base(
				"https://lab-test.example.com/",
				ApplicationType::TamanuCentral,
			)
		},
	)
	.await?;

	// Pending enrollment WITH an active token (registered_at + device_id NULL).
	let pending_with_token = insert(
		conn,
		None,
		Application {
			name: Some("New Lautoka Facility".to_string()),
			rank: Some(ServerRank::Production),
			group_id: Some(groups.pacific),
			notes: "Awaiting first check-in; an enrollment token is outstanding.".to_string(),
			tags: tags(&[("site", "lautoka")]),
			..base(
				"https://lautoka-facility.example.com/",
				ApplicationType::TamanuFacility,
			)
		},
	)
	.await?;

	// Pending enrollment with NO token yet (operator hasn't minted one).
	let pending_no_token = insert(
		conn,
		None,
		Application {
			name: Some("New Mendi Facility".to_string()),
			rank: Some(ServerRank::Production),
			group_id: Some(groups.highlands),
			notes: "Created but no enrollment token minted yet.".to_string(),
			..base(
				"https://mendi-facility.example.com/",
				ApplicationType::TamanuFacility,
			)
		},
	)
	.await?;

	// An ungrouped, unranked pending server (drives the bare "set up" path).
	insert(
		conn,
		None,
		Application {
			notes: "Bare pending server — no name, rank, or group yet.".to_string(),
			..base(
				"https://unconfigured.example.com/",
				ApplicationType::TamanuCentral,
			)
		},
	)
	.await?;

	// Archived (soft-deleted) server. Insert it live, then soft_delete so the
	// A SENAITE server in the same group as the Tamanu ones: the mixed-product
	// group the version rollup and billing attribution have to cope with.
	let senaite_lims = insert(
		conn,
		None,
		Application {
			name: Some("Pacific LIMS".to_string()),
			rank: Some(ServerRank::Production),
			group_id: Some(groups.pacific),
			cloud: Some(true),
			notes: "SENAITE laboratory system for the Pacific region.".to_string(),
			registered_at: Some(now),
			..base_of(
				ApplicationType::Senaite,
				"https://pacific-lims.example.com/",
			)
		},
	)
	.await?;

	// Archived box, inserted live so the real archival path runs: archiving the
	// machine releases its identity, deactivates its keys, and takes the
	// application on it down too.
	let archived = insert(
		conn,
		Some(devices.releaser),
		Application {
			name: Some("Decommissioned Old Central".to_string()),
			rank: Some(ServerRank::Production),
			group_id: Some(groups.demo),
			notes: "Archived: kept for history, hidden from live listings.".to_string(),
			registered_at: Some(now),
			..base(
				"https://old-central.example.com/",
				ApplicationType::TamanuCentral,
			)
		},
	)
	.await?;
	let archived_machine = Application::get_by_id(conn, archived)
		.await?
		.machine_id
		.expect("seeded application is machine-hosted");
	Machine::archive(conn, archived_machine).await?;

	// Re-assert the releaser role for the device listing (archival revoked the
	// device's credentials but kept its role).
	Device::trust(conn, devices.releaser, DeviceRole::Releaser).await?;

	let _ = devices.tailscale_admin;

	Ok(SeededServers {
		healthy_central,
		unhealthy_facility,
		down_facility,
		warning_facility,
		demo_server,
		ungrouped,
		pending_with_token,
		pending_no_token,
		senaite_lims,
	})
}

/// Mint an active enrollment token for the pending server that should show one
/// outstanding. The plaintext is discarded (only the hash is stored) — the UI
/// mints a fresh, usable blob on demand.
async fn seed_enrollment_tokens(
	conn: &mut AsyncPgConnection,
	applications: &SeededServers,
) -> Result<()> {
	// A ticket admits a box, so it is minted against the machine. The seed
	// names its boxes by the application on them, so the machine is read back
	// off that application.
	let pending = Application::get_by_id(conn, applications.pending_with_token).await?;
	let (_token, _plaintext) = MachineEnrollmentToken::mint(
		conn,
		pending
			.machine_id
			.expect("seeded application is machine-hosted"),
		SignedDuration::from_hours(48),
	)
	.await?;
	let _ = applications.pending_no_token;
	Ok(())
}

/// Insert status rows. Recent (NOW-relative) so they land in the live weekly
/// partition and drive status dots / version distance / health states. The
/// "down" facility gets NO recent status (absence = down). Uses a direct
/// insert of the full `Status` row so we can backdate `created_at` past a
/// target's own down threshold (still within the current week's partition).
async fn seed_statuses(
	conn: &mut AsyncPgConnection,
	applications: &SeededServers,
	devices: &SeededDevices,
	versions: &[VersionStr],
) -> Result<()> {
	use database::schema::statuses;

	let now = Timestamp::now();
	// Latest published version for "up to date"; an older one for "behind".
	let latest = versions
		.iter()
		.find(|v| v.0 == node_semver::Version::new(2, 10, 5))
		.cloned()
		.unwrap_or_default();
	let behind = versions
		.iter()
		.find(|v| v.0 == node_semver::Version::new(2, 8, 0))
		.cloned()
		.unwrap_or_default();

	fn extra(pg: &str, tamanu: &str, uptime: u64) -> serde_json::Value {
		serde_json::json!({
			"pgVersion": pg,
			"tamanuVersion": tamanu,
			"bestoolVersion": "2.10.5",
			"nodeVersion": "20.11.0",
			"uptimeSecs": uptime,
		})
	}

	async fn insert_status(
		conn: &mut AsyncPgConnection,
		server_id: Uuid,
		device_id: Option<Uuid>,
		created_at: Timestamp,
		version: Option<VersionStr>,
		healthy: bool,
		health: serde_json::Value,
		extra: serde_json::Value,
	) -> Result<()> {
		// A push is the box's, so the row names the machine. The seed names its
		// boxes by the application on them, and each seeded application has a
		// machine of its own, so the machine is read back off the application.
		let machine_id = Application::get_by_id(conn, server_id)
			.await?
			.machine_id
			.expect("seeded application is machine-hosted");
		diesel::insert_into(statuses::table)
			.values(Status {
				id: Uuid::new_v4(),
				created_at,
				machine_id: Some(machine_id),
				server_id: Some(server_id),
				device_id,
				version,
				extra,
				healthy,
				health,
				source: "alertd".into(),
			})
			.execute(conn)
			.await?;
		Ok(())
	}

	// Healthy central — up to date, fresh, all checks passing.
	insert_status(
		conn,
		applications.healthy_central,
		Some(devices.tailscale_server),
		now,
		Some(latest.clone()),
		true,
		serde_json::json!([
			{"check": "database_connectivity", "healthy": true},
			{"check": "disk_space", "healthy": true, "free_pct": 62},
			{"check": "sync_lag", "healthy": true, "lag_seconds": 4},
		]),
		extra("PostgreSQL 15.4, compiled by gcc", "2.10.5", 864_000),
	)
	.await?;

	// Unhealthy facility — top-level unhealthy, behind on version.
	insert_status(
		conn,
		applications.unhealthy_facility,
		Some(devices.mtls_server[0]),
		now,
		Some(behind.clone()),
		false,
		serde_json::json!([
			{"check": "database_connectivity", "healthy": false, "error": "connection refused"},
			{"check": "disk_space", "healthy": true, "free_pct": 40},
		]),
		extra("PostgreSQL 14.9, compiled by gcc", "2.8.0", 3_600),
	)
	.await?;

	// Warning facility — top-level healthy but one check failing.
	insert_status(
		conn,
		applications.warning_facility,
		Some(devices.mtls_server[2]),
		now,
		Some(latest.clone()),
		true,
		serde_json::json!([
			{"check": "database_connectivity", "healthy": true},
			{"check": "sync_lag", "healthy": false, "lag_seconds": 1800},
		]),
		extra("PostgreSQL 15.4, compiled by Visual C++", "2.10.5", 120_000),
	)
	.await?;

	// Demo server — healthy but unmonitored; recent.
	insert_status(
		conn,
		applications.demo_server,
		None,
		now,
		Some(latest.clone()),
		true,
		serde_json::json!([{"check": "database_connectivity", "healthy": true}]),
		extra("PostgreSQL 15.4", "2.10.5", 7_200),
	)
	.await?;

	// Ungrouped server — last reported ~20 minutes ago, so past the default
	// ten-minute threshold and unreachable.
	insert_status(
		conn,
		applications.ungrouped,
		None,
		now - SignedDuration::from_mins(20),
		Some(behind.clone()),
		true,
		serde_json::json!([{"check": "database_connectivity", "healthy": true}]),
		extra("PostgreSQL 13.12", "2.8.0", 50_000),
	)
	.await?;

	// SENAITE server — healthy, and reporting no version at all. Its extra
	// deliberately carries no `tamanuVersion`, so the detail view has to
	// present nothing rather than "unknown", and the group's headline version
	// has to come from a Tamanu member.
	insert_status(
		conn,
		applications.senaite_lims,
		None,
		now,
		None,
		true,
		serde_json::json!([
			{"check": "database_connectivity", "healthy": true},
			{"check": "disk_space", "healthy": true, "free_pct": 71},
		]),
		serde_json::json!({
			"pgVersion": "PostgreSQL 16.2, compiled by gcc",
			"bestoolVersion": "2.10.5",
			"uptimeSecs": 604_800,
		}),
	)
	.await?;

	// down_facility: intentionally NO recent status row → "down".
	let _ = applications.down_facility;

	Ok(())
}

/// Drive the real issue/event/incident machinery via `NewEvent::save`, then
/// resolve some so the UI shows open and closed states. Because the unhealthy
/// applications are grouped + monitored, error/critical issues roll up into
/// incidents automatically.
async fn seed_issues_and_incidents(
	conn: &mut AsyncPgConnection,
	applications: &SeededServers,
	admins: &[String],
) -> Result<()> {
	let now = Timestamp::now();

	async fn push(
		conn: &mut AsyncPgConnection,
		server_id: Uuid,
		source: &str,
		r#ref: &str,
		result: CheckResult,
		escalates: bool,
		description: &str,
		message: &str,
		occurred_at: Timestamp,
	) -> Result<Issue> {
		let stamp = database::issues::CheckStateStamp {
			check: r#ref.to_string(),
			observed: result,
			effective: result,
			escalates,
			detail: None,
		};
		let active = matches!(
			result,
			CheckResult::Failed | CheckResult::Warning | CheckResult::Broken
		);
		NewEvent {
			source: source.to_string(),
			r#ref: r#ref.to_string(),
			description: Some(description.to_string()),
			message: message.to_string(),
			active: Some(active),
			occurred_at: Some(occurred_at),
		}
		.save_with_state(conn, server_id, None, Some(&stamp), false)
		.await
	}

	// Critical DB issue on the unhealthy facility (opens an incident on Pacific).
	let crit = push(
		conn,
		applications.unhealthy_facility,
		"healthcheck",
		"database_connectivity",
		CheckResult::Failed,
		true,
		"Database connection refused",
		"The server cannot reach its PostgreSQL instance (connection refused). \
		 Sync and API requests are failing.",
		now - SignedDuration::from_mins(45),
	)
	.await?;

	// Push the same event a couple more times so the event coalescing /
	// occurrence count is exercised.
	for _ in 0..2 {
		push(
			conn,
			applications.unhealthy_facility,
			"healthcheck",
			"database_connectivity",
			CheckResult::Failed,
			true,
			"Database connection refused",
			"The server cannot reach its PostgreSQL instance (connection refused). \
			 Sync and API requests are failing.",
			now - SignedDuration::from_mins(40),
		)
		.await?;
	}

	// A warning issue on the same group's healthy central — joins the open
	// incident for context but wouldn't open one on its own.
	push(
		conn,
		applications.healthy_central,
		"healthcheck",
		"certificate_expiry",
		CheckResult::Warning,
		false,
		"TLS certificate expiring soon",
		"The TLS certificate expires in 9 days.",
		now - SignedDuration::from_mins(30),
	)
	.await?;

	// An error issue on the warning facility (Highlands group) → opens a
	// second incident on a different group.
	let highlands_issue = push(
		conn,
		applications.warning_facility,
		"healthcheck",
		"sync_lag",
		CheckResult::Failed,
		false,
		"Sync lag exceeds threshold",
		"Sync lag has been above 30 minutes for the last hour.",
		now - SignedDuration::from_mins(60),
	)
	.await?;

	// A resolved (closed) incident: open an error on the highlands clone, then
	// resolve the issue, which cascades the incident closed.
	let transient = push(
		conn,
		applications.warning_facility,
		"healthcheck",
		"disk_space",
		CheckResult::Failed,
		false,
		"Disk almost full",
		"Disk usage crossed 95% earlier; has since recovered.",
		now - SignedDuration::from_hours(6),
	)
	.await?;
	Issue::resolve(conn, transient.id, &admins[1], ResolvedReason::Fixed).await?;

	// A warning on the ungrouped server (recorded, no incident).
	push(
		conn,
		applications.ungrouped,
		"app",
		"slow-query",
		CheckResult::Warning,
		false,
		"Slow query detected",
		"A report query took 12s; investigate indexing.",
		now - SignedDuration::from_hours(2),
	)
	.await?;

	// A skipped-graded condition (recorded, never participates).
	push(
		conn,
		applications.demo_server,
		"app",
		"debug-trace",
		CheckResult::Skipped,
		false,
		"Verbose trace captured",
		"Captured a debug trace during a demo session.",
		now - SignedDuration::from_hours(1),
	)
	.await?;

	// Snooze the highlands sync_lag issue so the snoozed state is represented.
	Issue::snooze(
		conn,
		highlands_issue.id,
		now + SignedDuration::from_hours(12),
	)
	.await?;

	// Operator notes on the still-open critical issue and its incident.
	IssueNote::add(
		conn,
		crit.id,
		&admins[0],
		"Paged the on-call DBA; investigating the connection pool.",
	)
	.await?;

	// Annotate every currently-open incident so the incident-notes UI has rows.
	// We deliberately leave these incidents OPEN (the Pacific critical above is
	// still active) so the UI shows an in-progress incident; the closed-incident
	// state is covered by the resolved Highlands disk_space error, which cascaded
	// its incident shut.
	let active = Incident::list_active(conn, 50).await?;
	for incident in &active {
		IncidentNote::add(
			conn,
			incident.id,
			&admins[0],
			"Incident acknowledged; bridge call in progress.",
		)
		.await?;
	}

	Ok(())
}

/// Seed a server-scoped and a group-scoped silence so the silence list UI has
/// rows, and the incident workflow's "silenced" branch has coverage.
async fn seed_silences(
	conn: &mut AsyncPgConnection,
	applications: &SeededServers,
	groups: &SeededGroups,
	admins: &[String],
) -> Result<()> {
	ServerSilencedRef::add(
		conn,
		applications.demo_server,
		"app",
		"debug-trace",
		Some(&admins[0]),
	)
	.await?;

	ServerGroupSilencedRef::add(
		conn,
		groups.demo,
		"healthcheck",
		"backup_freshness",
		Some(&ApplicationType::TamanuFacility),
		Some(&admins[0]),
	)
	.await?;

	let _ = (groups.empty, groups.pacific, groups.highlands);
	Ok(())
}

/// Print row counts for the key tables so the operator can eyeball the result.
async fn report(conn: &mut AsyncPgConnection) -> Result<()> {
	#[derive(diesel::QueryableByName)]
	struct Count {
		#[diesel(sql_type = diesel::sql_types::Text)]
		label: String,
		#[diesel(sql_type = diesel::sql_types::BigInt)]
		n: i64,
	}

	let tables = [
		"admins",
		"versions",
		"version_known_issues",
		"check_policies",
		"devices",
		"device_keys",
		"device_connections",
		"server_groups",
		"applications",
		"machine_enrollment_tokens",
		"statuses",
		"issues",
		"incidents",
		"incident_issues",
		"issue_notes",
		"incident_notes",
		"slack_outbox",
		"scoped_check_policies",
	];

	eprintln!("seed: row counts");
	for t in tables {
		let row: Count = diesel::sql_query(format!(
			"SELECT '{t}' AS label, COUNT(*)::bigint AS n FROM {t}"
		))
		.get_result(conn)
		.await?;
		eprintln!("  {:<28} {}", row.label, row.n);
	}
	Ok(())
}
