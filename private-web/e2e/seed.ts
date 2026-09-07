// Seeding helpers for Playwright e2e specs. Connect once per worker
// to the per-worker Postgres database; expose typed insert helpers
// for the rows the UI tests need to see. Each helper returns the
// inserted row's id (and a few readable fields) so the test can
// assert on the value it just produced.
//
// We talk to the database directly rather than going through the
// private-server API because (a) most state we want isn't reachable
// over admin endpoints (statuses, issues, devices in arbitrary
// roles) and (b) the API enforces validation and side-effects we
// often don't want in setup (e.g. submitting a status fires events
// and opens incidents).

import { createHash, randomBytes, randomUUID } from "node:crypto";
import { Client } from "pg";

export interface Sql {
	query<R extends Record<string, unknown> = Record<string, unknown>>(
		text: string,
		params?: unknown[],
	): Promise<R[]>;
	end(): Promise<void>;
}

export async function connect(databaseUrl: string): Promise<Sql> {
	const client = new Client({ connectionString: databaseUrl });
	await client.connect();
	return {
		async query<R extends Record<string, unknown> = Record<string, unknown>>(
			text: string,
			params: unknown[] = [],
		): Promise<R[]> {
			const result = await client.query(text, params);
			return result.rows as R[];
		},
		end: () => client.end(),
	};
}

function randomLabel(prefix: string): string {
	return `${prefix}-${randomBytes(4).toString("hex")}`;
}

// ── The check namespace ─────────────────────────────────────────────────────
//
// A check's identity is its source, its namespace, and its name, so a seeded
// catalog row or silence that leaves the namespace off is one ingestion could
// never produce: the UI would then be reading a row nothing files into.
// These helpers derive it the way `Namespace::of` does, so the rows a spec
// seeds are the rows a real report would have made.

/** The checks that describe the box rather than the workload on it.
 *
 * A snapshot of MACHINE_SUBJECT_CHECKS in crates/commons-types/src/subject.rs.
 * `the_e2e_seed_snapshot_matches_the_subject_list`, beside that list, parses
 * this array to hold the two together, so keep the quoting as it is. */
const MACHINE_SUBJECT_CHECKS = [
	"billing_tags",
	"btrfs",
	"caddy_resolvers",
	"caddy_version",
	"caddyfile_version",
	"canopy_registration",
	"disk_free",
	"external_users",
	"held_captures",
	"inodes",
	"ips",
	"load",
	"memory",
	"munin",
	"tailscale",
	"tailscale_config",
	"time_sync",
	"uptime",
];

/** The reported detail fields that describe the box rather than the workload
 * on it.
 *
 * A snapshot of MACHINE_SUBJECT_DETAIL in crates/commons-types/src/subject.rs.
 * `the_e2e_seed_snapshot_matches_the_detail_list`, beside that list, parses
 * this array to hold the two together, so keep the quoting as it is. */
const MACHINE_SUBJECT_DETAIL = [
	"arch",
	"bestoolVersion",
	"cpuCores",
	"filesystems",
	"hostname",
	"instanceTags",
	"ipv4",
	"ipv6",
	"kernel",
	"lanIps",
	"munin",
	"nat64",
	"osKind",
	"osName",
	"osTimezone",
	"osVersion",
	"reportingSchemaVersion",
	"services",
	"totalMemoryBytes",
	"uptimeSecs",
	"virtualisation",
	"virtualised",
	"wanIpv4",
	"wanIpv6",
];

/** Split a push's detail the way ingestion does: the box's fields to the
 * machine, everything else to the application. */
export function splitDetail(extra: Record<string, unknown>): {
	machine: Record<string, unknown>;
	application: Record<string, unknown>;
} {
	const machine: Record<string, unknown> = {};
	const application: Record<string, unknown> = {};
	for (const [key, value] of Object.entries(extra)) {
		if (MACHINE_SUBJECT_DETAIL.includes(key)) machine[key] = value;
		else application[key] = value;
	}
	return { machine, application };
}

/** Sources whose check names canopy curates itself. Their names mean one
 * thing fleet-wide, so they are namespaced flat. */
const RESERVED_SOURCES = ["canopy", "manual"];

/** The two namespace columns, as `Namespace::to_columns` writes them. */
export interface SeedNamespace {
	subject: string | null;
	applicationType: string | null;
}

/** The namespace a check lands in: flat for a curated source, the machine's
 * for a name that describes the box, and the reporting application's type
 * otherwise. */
export function namespaceOf(
	source: string,
	checkName: string,
	applicationType: string,
): SeedNamespace {
	if (RESERVED_SOURCES.includes(source)) {
		return { subject: null, applicationType: null };
	}
	if (MACHINE_SUBJECT_CHECKS.includes(checkName)) {
		return { subject: "machine", applicationType: null };
	}
	return { subject: "application", applicationType };
}

/** What kind of application a seeded row is, for the namespace its checks
 * land in. Falls back to a Tamanu central, which is what `seedServer`
 * defaults to. */
async function applicationTypeOf(sql: Sql, applicationId: string): Promise<string> {
	const rows = await sql.query<{ type: string }>(
		"SELECT type FROM applications WHERE id = $1",
		[applicationId],
	);
	return rows[0]?.type ?? "tamanu-central";
}

/** Wipe every table this suite seeds into. Use from a `beforeEach`
 * when a test needs to make assertions that depend on the absence
 * of data (the "empty state" UI banners, mostly). Fast — single
 * statement with CASCADE. */
export async function resetSeededTables(sql: Sql): Promise<void> {
	await sql.query(
		"TRUNCATE statuses, application_reported_detail, machine_reported_detail, issues, device_keys, applications, machines, server_groups, server_group_domains, devices, versions, tailscale_users, check_policies, scoped_check_policies, source_policies, server_group_backup_config, server_group_backup_schedule, machine_backup_capabilities, backup_requests, backup_runs, backup_run_progress, backup_repo_stats, backup_maintenance_runs, backup_credential_issuances, restore_replicas, restore_consumer_capabilities, backup_restore_checks, migration_tests, migration_timings, upgrade_plans, maintenance_windows, version_known_issues, recovery_vault_writes, application_names, application_certificates, compromised_keys RESTART IDENTITY CASCADE",
	);
	// The truncate takes the migration-seeded nil "Canopy" application with
	// it; self-alerts attach to that row, so put it back.
	// The machine goes back with it: an application runs on exactly one, and
	// the split's backfill gave this row a machine sharing its id.
	await sql.query(
		"INSERT INTO machines (id, name) VALUES ('00000000-0000-0000-0000-000000000000', 'Canopy')",
	);
	await sql.query(
		"INSERT INTO applications (id, type, name, host, machine_id) VALUES ('00000000-0000-0000-0000-000000000000', 'canopy', 'Canopy', 'http://localhost', '00000000-0000-0000-0000-000000000000')",
	);
	// Same for the migration's one seeded source policy: tamanu reports on
	// its own schedule, so its silence is not a reachability signal.
	await sql.query(
		"INSERT INTO source_policies (source, reachability) VALUES ('tamanu', 'quiet')",
	);
	// And the catalog row the server registers at startup for canopy's own
	// reachability check: every server presents a passing reachability, and
	// that presentation is gated on this row existing.
	await sql.query(
		"INSERT INTO check_policies (source, check_name, ceiling, reviewed_at, reviewed_by) \
		 VALUES ('canopy', 'reachability', 'failed', NOW(), 'canopy')",
	);
}

export interface SeededServerGroup {
	id: string;
	name: string;
}

export async function seedServerGroup(
	sql: Sql,
	opts: {
		name?: string;
		notes?: string;
		tags?: Record<string, string>;
		/** Slack open cooldown in seconds. Omit to keep the migration default. */
		slackOpenDelaySeconds?: number;
		/** Incident linger window in seconds. Omit to keep the migration default. */
		slackCloseDelaySeconds?: number;
	} = {},
): Promise<SeededServerGroup> {
	const id = randomUUID();
	const name = opts.name ?? randomLabel("group");
	const columns = ["id", "name", "notes", "tags"];
	const values: unknown[] = [
		id,
		name,
		opts.notes ?? "",
		JSON.stringify(opts.tags ?? {}),
	];
	const exprs = ["$1", "$2", "$3", "$4::jsonb"];
	if (opts.slackOpenDelaySeconds !== undefined) {
		columns.push("slack_open_delay");
		values.push(opts.slackOpenDelaySeconds);
		exprs.push(`make_interval(secs => $${values.length})`);
	}
	if (opts.slackCloseDelaySeconds !== undefined) {
		columns.push("slack_close_delay");
		values.push(opts.slackCloseDelaySeconds);
		exprs.push(`make_interval(secs => $${values.length})`);
	}
	await sql.query(
		`INSERT INTO server_groups (${columns.join(", ")})
		 VALUES (${exprs.join(", ")})`,
		values,
	);
	return { id, name };
}

export type ServerRank = "production" | "clone" | "demo" | "test" | "dev";
export type ApplicationType =
	| "tamanu-central"
	| "tamanu-facility"
	| "senaite"
	| "canopy";

export interface SeededServer {
	id: string;
	/** `null` when the application was seeded unnamed, which is how one Canopy
	 * learned about from a report arrives: a name is the operator's to set. */
	name: string | null;
	host: string;
	type: ApplicationType;
	rank: ServerRank | null;
	/** The box this workload runs on. Maintenance is declared over it. */
	machineId: string;
}

/** A box on its own, with no workload on it — the state a machine is in
 * between an operator adding it and the first report arriving. */
export async function seedMachine(
	sql: Sql,
	opts: { name?: string; groupId?: string | null; deviceId?: string } = {},
): Promise<{ id: string; name: string }> {
	const id = randomUUID();
	const name = opts.name ?? randomLabel("box");
	await sql.query(
		`INSERT INTO machines (id, name, group_id, device_id) VALUES ($1, $2, $3, $4)`,
		[id, name, opts.groupId ?? null, opts.deviceId ?? null],
	);
	return { id, name };
}

/** What a box reports about itself. */
export async function seedMachineReport(
	sql: Sql,
	opts: {
		machineId: string;
		source?: string;
		extra?: Record<string, unknown>;
	},
): Promise<void> {
	await sql.query(
		`INSERT INTO machine_reported_detail (machine_id, source, extra, reported_at)
		 VALUES ($1, $2, $3::jsonb, NOW())`,
		[opts.machineId, opts.source ?? "alertd", JSON.stringify(opts.extra ?? {})],
	);
}

/** What an application last reported, written straight to the current-state
 * projection with no matching status row.
 *
 * This is the state a long-quiet application is really in: `statuses` is
 * pruned and read through a lookback window, so history from months ago is
 * gone, while the projection carries one row per (application, source) for as
 * long as the application exists. Seeding it alone gives an application that
 * has reported, just not recently, which is what separates unreachable from
 * never heard from. */
export async function seedApplicationReport(
	sql: Sql,
	opts: {
		applicationId: string;
		source?: string;
		version?: string | null;
		extra?: Record<string, unknown>;
		/** ISO 8601 timestamp or relative SQL like `NOW() - INTERVAL '90 days'`.
		 * Defaults to NOW(). */
		reportedAt?: string;
	},
): Promise<void> {
	const useSqlExpr =
		opts.reportedAt !== undefined && opts.reportedAt.toUpperCase().startsWith("NOW");
	const reportedAtClause = opts.reportedAt === undefined
		? "NOW()"
		: useSqlExpr
			? opts.reportedAt
			: "$5";
	const params: unknown[] = [
		opts.applicationId,
		opts.source ?? "alertd",
		JSON.stringify(opts.extra ?? {}),
		opts.version ?? null,
	];
	if (!useSqlExpr && opts.reportedAt !== undefined) params.push(opts.reportedAt);
	await sql.query(
		`INSERT INTO application_reported_detail (application_id, source, extra, version, reported_at)
		 VALUES ($1, $2, $3::jsonb, $4, ${reportedAtClause})
		 ON CONFLICT (application_id, source) DO UPDATE
		 SET extra = EXCLUDED.extra, version = EXCLUDED.version, reported_at = EXCLUDED.reported_at`,
		params,
	);
}

export async function seedServer(
	sql: Sql,
	opts: {
		/** Pass `null` for an application nobody has named. */
		name?: string | null;
		host?: string;
		/** What the application is. Defaults to a Tamanu central. */
		type?: ApplicationType;
		rank?: ServerRank | null;
		groupId?: string | null;
		deviceId?: string;
		notes?: string;
		tags?: Record<string, string>;
		/** Whether canopy actively monitors this server. Off by default so
		 * e2e seeds don't accidentally trip the reachability sweep. */
		isMonitored?: boolean;
		/** Threshold in seconds; defaults to 600 (10 min). Must be > 0. */
		alertWhenDownFor?: number;
		/** Whether the server may manage its own DNS records / obtain its own
		 * TLS certificates under its group's domains. Both off by default, as
		 * they are for a real server. */
		mayManageDns?: boolean;
		mayManageTls?: boolean;
		/** The box to put this workload on. Omit for a box of its own; pass
		 * another application's `machineId` for the two-workloads-on-one-box
		 * case. */
		machineId?: string;
	} = {},
): Promise<SeededServer> {
	const id = randomUUID();
	const name = opts.name === null ? null : (opts.name ?? randomLabel("srv"));
	const host = opts.host ?? `https://${randomLabel("host")}.e2e.invalid`;
	const type = opts.type ?? "tamanu-central";
	const rank = opts.rank ?? "production";
	const isMonitored = opts.isMonitored ?? false;
	const alertWhenDownFor = opts.alertWhenDownFor ?? 600;
	// A box of its own for each seeded workload unless the caller names one,
	// carrying the same group so the machine and the application agree on
	// which group they're in.
	//
	// An identity belongs to the box, so a seeded device binds to the machine.
	// Anything that resolves a device to what it speaks for — backups, reports —
	// goes through the machine.
	const machineId = opts.machineId ?? randomUUID();
	if (opts.machineId === undefined) {
		await sql.query(
			`INSERT INTO machines (id, name, group_id, device_id) VALUES ($1, $2, $3, $4)`,
			// A machine is always named, whether or not the workload on it is.
			[
				machineId,
				name ?? randomLabel("box"),
				opts.groupId ?? null,
				opts.deviceId ?? null,
			],
		);
	}
	await sql.query(
		`INSERT INTO applications (id, name, host, type, rank, group_id, is_monitored, alert_when_down_for, notes, tags, may_manage_dns, may_manage_tls, machine_id)
		 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10::jsonb, $11, $12, $13)`,
		[
			id,
			name,
			host,
			type,
			rank,
			opts.groupId ?? null,
			isMonitored,
			alertWhenDownFor,
			opts.notes ?? "",
			JSON.stringify(opts.tags ?? {}),
			opts.mayManageDns ?? false,
			opts.mayManageTls ?? false,
			machineId,
		],
	);
	return { id, name, host, type, rank, machineId };
}

export interface SeededDevice {
	id: string;
	role: string;
}

export async function seedDevice(
	sql: Sql,
	opts: { role?: string; tailscaleNodeName?: string } = {},
): Promise<SeededDevice> {
	const id = randomUUID();
	const role = opts.role ?? "machine";
	await sql.query(
		`INSERT INTO devices (id, role, tailscale_node_name) VALUES ($1, $2, $3)`,
		[id, role, opts.tailscaleNodeName ?? null],
	);
	return { id, role };
}

/** Add a device_key row for a device (active or inactive). */
export async function seedDeviceKey(
	sql: Sql,
	opts: {
		deviceId: string;
		name?: string;
		isActive?: boolean;
		keyData?: Buffer;
	},
): Promise<{ id: string }> {
	const id = randomUUID();
	await sql.query(
		`INSERT INTO device_keys (id, device_id, name, is_active, key_data)
		 VALUES ($1, $2, $3, $4, $5)`,
		[
			id,
			opts.deviceId,
			opts.name ?? randomLabel("key"),
			opts.isActive ?? true,
			opts.keyData ?? randomBytes(32),
		],
	);
	return { id };
}

/** A connection an identity made to canopy, carrying the User-Agent it
 * presented.
 *
 * The runtime an application reports is read out of this when a push doesn't
 * name one, so it goes on the identity bound to the application's *machine*,
 * not on whichever identity happened to file a push. */
export async function seedDeviceConnection(
	sql: Sql,
	opts: { deviceId: string; userAgent: string; ip?: string },
): Promise<{ id: string }> {
	const id = randomUUID();
	await sql.query(
		`INSERT INTO device_connections (id, device_id, ip, user_agent)
		 VALUES ($1, $2, $3, $4)`,
		[id, opts.deviceId, opts.ip ?? "203.0.113.7", opts.userAgent],
	);
	return { id };
}

export interface SeededStatus {
	id: string;
	createdAt: string;
}

export async function seedStatus(
	sql: Sql,
	opts: {
		serverId: string;
		deviceId?: string | null;
		version?: string | null;
		healthy?: boolean;
		health?: unknown[];
		extra?: Record<string, unknown>;
		/** Reporting source. Defaults to `alertd`, as ingestion does. */
		source?: string;
		/** ISO 8601 timestamp or relative SQL like `NOW() - INTERVAL '1 hour'`.
		 * Defaults to NOW(). */
		createdAt?: string;
	},
): Promise<SeededStatus> {
	const id = randomUUID();
	const source = opts.source ?? "alertd";
	const useSqlExpr =
		opts.createdAt !== undefined && opts.createdAt.toUpperCase().startsWith("NOW");
	const createdAtClause = opts.createdAt === undefined
		? "NOW()"
		: useSqlExpr
			? opts.createdAt
			: "$9";
	const params: unknown[] = [
		id,
		opts.serverId,
		opts.deviceId ?? null,
		opts.version ?? null,
		opts.healthy ?? true,
		JSON.stringify(opts.health ?? []),
		JSON.stringify(opts.extra ?? {}),
		source,
	];
	if (!useSqlExpr && opts.createdAt !== undefined) params.push(opts.createdAt);
	const rows = await sql.query<{ created_at: string }>(
		`INSERT INTO statuses
		 (id, server_id, device_id, version, healthy, health, extra, source, created_at)
		 VALUES ($1, $2, $3, $4, $5, $6::jsonb, $7::jsonb, $8, ${createdAtClause})
		 RETURNING created_at`,
		params,
	);

	const machineRows = await sql.query<{ machine_id: string }>(
		"SELECT machine_id FROM applications WHERE id = $1",
		[opts.serverId],
	);
	const machineId = machineRows[0]?.machine_id ?? null;

	// Mirror ingestion: the push is the source's current detail, which is what
	// the live figures read — they never search status history. Ordered by the
	// status's own timestamp so out-of-order seeding still resolves
	// newest-wins correctly.
	//
	// Ingestion splits the push by grain, so the seed does too: a field about
	// the box is the machine's report, and a fleet spread that counts machines
	// reads it there. Writing it all to the application would make a
	// two-workload box report the same fact twice.
	const detail = splitDetail(opts.extra ?? {});
	await sql.query(
		`INSERT INTO application_reported_detail (application_id, source, extra, version, reported_at)
		 VALUES ($1, $2, $3::jsonb, $4, $5)
		 ON CONFLICT (application_id, source) DO UPDATE
		 SET extra = EXCLUDED.extra, version = EXCLUDED.version, reported_at = EXCLUDED.reported_at
		 WHERE application_reported_detail.reported_at <= EXCLUDED.reported_at`,
		[
			opts.serverId,
			source,
			JSON.stringify(detail.application),
			opts.version ?? null,
			rows[0]!.created_at,
		],
	);
	if (machineId !== null && Object.keys(detail.machine).length > 0) {
		await sql.query(
			`INSERT INTO machine_reported_detail (machine_id, source, extra, reported_at)
			 VALUES ($1, $2, $3::jsonb, $4)
			 ON CONFLICT (machine_id, source) DO UPDATE
			 SET extra = EXCLUDED.extra, reported_at = EXCLUDED.reported_at
			 WHERE machine_reported_detail.reported_at <= EXCLUDED.reported_at`,
			[machineId, source, JSON.stringify(detail.machine), rows[0]!.created_at],
		);
	}

	// Mirror ingestion: each check in the push has a check-state row, which
	// is what the health rollup and attention pages read. Degraded checks
	// carry the degraded-streak stamps; healthy ones record inactive state.
	// The reporting application's type decides the namespace a check that
	// names the workload belongs to.
	const applicationType = await applicationTypeOf(sql, opts.serverId);
	// Ingestion splits a unified push by each check's subject: a check about
	// the box files against the machine, not against the workload that
	// happened to report it. Seeded state has to land the same way or a box
	// with two workloads reads as two sets of the same facts.
	for (const entry of (opts.health ?? []) as Record<string, unknown>[]) {
		const check = entry.check;
		if (typeof check !== "string") continue;
		const result =
			typeof entry.result === "string"
				? entry.result
				: typeof entry.healthy === "boolean"
					? entry.healthy
						? "passed"
						: "failed"
					: null;
		if (result === null) continue;
		// Mirror ingestion's upsert_default: a check-state only presents and
		// counts if a live catalog row backs it, in the namespace the reporter
		// files into. Never clobbers an explicit seedCheckPolicy for the same
		// entry.
		const ns = namespaceOf(source, check, applicationType);
		await sql.query(
			`INSERT INTO check_policies (source, subject, application_type, check_name)
			 VALUES ($1, $2, $3, $4)
			 ON CONFLICT (source, subject, application_type, check_name) DO NOTHING`,
			[source, ns.subject, ns.applicationType, check],
		);
		const degraded = ["failed", "warning", "broken"].includes(result);
		const onMachine = ns.subject === "machine" && machineId !== null;
		await sql.query(
			`INSERT INTO issues
			 (application_id, machine_id, source, ref, check_name, observed_result, effective_result, detail, message, active, first_seen, last_seen, degraded_since, last_degraded_at)
			 VALUES ($1, $11, $10, $2, $3, $4, $4, $5::jsonb, $6, $7, NOW(), NOW(), $8, $9)
			 ON CONFLICT DO NOTHING`,
			[
				onMachine ? null : opts.serverId,
				`health/${check}`,
				check,
				result,
				JSON.stringify(entry),
				`Health check '${check}' ${degraded ? "degraded" : "recorded"}`,
				degraded,
				degraded ? new Date().toISOString() : null,
				degraded ? new Date().toISOString() : null,
				source,
				onMachine ? machineId : null,
			],
		);
	}

	return { id, createdAt: String(rows[0]!.created_at) };
}

/** Stability record for a check state previously seeded via `seedStatus`
 * (or `seedIssue`), keyed by the (server, source, ref) it created.
 * `transitions` is the healthy↔degraded ring, oldest first; `dutyBuckets`
 * maps hour-of-week bucket indexes (UTC, Monday 00:00 = 0) to
 * [observations, degraded] pairs, all other buckets zero. */
export async function seedCheckStability(
	sql: Sql,
	opts: {
		serverId: string;
		source?: string;
		check: string;
		observations?: number;
		degradedObservations?: number;
		transitions?: Array<{ at: string; degraded: boolean }>;
		dutyBuckets?: Record<number, [number, number]>;
	},
): Promise<void> {
	const transitions = opts.transitions ?? [];
	const duty: [number, number][] = Array.from({ length: 168 }, (_, i) => {
		const bucket = opts.dutyBuckets?.[i];
		return bucket ? [bucket[0], bucket[1]] : [0, 0];
	});
	const last = transitions[transitions.length - 1];
	await sql.query(
		`INSERT INTO check_stability
		 (issue_id, observations, degraded_observations, last_observed_at, last_observed_degraded, transitions, duty_cycle)
		 SELECT id, $4, $5, $6::timestamptz, $7, $8::jsonb, $9::jsonb
		 FROM issues WHERE application_id = $1 AND source = $2 AND ref = $3`,
		[
			opts.serverId,
			opts.source ?? "alertd",
			`health/${opts.check}`,
			opts.observations ?? transitions.length,
			opts.degradedObservations ?? transitions.filter((t) => t.degraded).length,
			last?.at ?? null,
			last?.degraded ?? null,
			JSON.stringify(transitions),
			JSON.stringify(duty),
		],
	);
}

export interface SeededCheckPolicy {
	source: string;
	checkName: string;
	namespace: SeedNamespace;
}

/** Policy row for a (source, check), as ingestion would have upserted
 * (plus an operator-set ceiling). Upserts so tests can call it without
 * worrying whether ingestion already created a default row. */
export async function seedCheckPolicy(
	sql: Sql,
	opts: {
		checkName: string;
		source?: string;
		ceiling?: string;
		escalates?: boolean;
		notes?: string | null;
		documentation?: string | null;
		/** Fleet-wide last-seen, as the liveness reconciler would set it.
		 * Backdate it past 7 days to make the check a decommissioning
		 * candidate ("gone quiet"). */
		lastSeen?: string | null;
		decommissionedAt?: string | null;
		/** Which application type reports it, for a check that names the
		 * workload. Defaults to a Tamanu central, as `seedServer` does. */
		applicationType?: ApplicationType;
	},
): Promise<SeededCheckPolicy> {
	const source = opts.source ?? "alertd";
	const namespace = namespaceOf(
		source,
		opts.checkName,
		opts.applicationType ?? "tamanu-central",
	);
	await sql.query(
		`INSERT INTO check_policies (source, subject, application_type, check_name, ceiling, escalates, notes, documentation, last_seen, decommissioned_at)
		 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
		 ON CONFLICT (source, subject, application_type, check_name)
		 DO UPDATE SET ceiling = EXCLUDED.ceiling, escalates = EXCLUDED.escalates, notes = EXCLUDED.notes, documentation = EXCLUDED.documentation, last_seen = EXCLUDED.last_seen, decommissioned_at = EXCLUDED.decommissioned_at`,
		[
			source,
			namespace.subject,
			namespace.applicationType,
			opts.checkName,
			opts.ceiling ?? "warning",
			opts.escalates ?? false,
			opts.notes ?? null,
			opts.documentation ?? null,
			opts.lastSeen ?? null,
			opts.decommissionedAt ?? null,
		],
	);
	return { source, checkName: opts.checkName, namespace };
}

/** The check name a silence ref maps to in scoped-policy storage:
 * refs of source-reported checks carry the `health/` prefix. */
function refToCheck(ref: string): string {
	return ref.startsWith("health/") ? ref.slice("health/".length) : ref;
}

/** Server-scope silence for a `(source, ref)` pair, as the UI's
 * silence button would create: a scoped check policy with a skipped
 * ceiling. For a healthcheck, pass `ref: "health/<check>"` (source
 * defaults to "alertd"). */
export async function seedServerSilencedRef(
	sql: Sql,
	opts: {
		serverId: string;
		ref: string;
		source?: string;
		createdBy?: string | null;
	},
): Promise<void> {
	const source = opts.source ?? "alertd";
	const check = refToCheck(opts.ref);
	// A silence names a check, so it names a namespace: quieting one
	// application type's check leaves another type's same-named check alone.
	const ns = namespaceOf(source, check, await applicationTypeOf(sql, opts.serverId));
	await sql.query(
		`INSERT INTO scoped_check_policies (application_id, source, subject, application_type, check_name, ceiling, created_by)
		 VALUES ($1, $2, $3, $4, $5, 'skipped', $6)
		 ON CONFLICT DO NOTHING`,
		[
			opts.serverId,
			source,
			ns.subject,
			ns.applicationType,
			check,
			opts.createdBy ?? null,
		],
	);
}

/** Group-scope silence for a `(source, ref)` pair; see
 * {@link seedServerSilencedRef}. */
export async function seedGroupSilencedRef(
	sql: Sql,
	opts: {
		groupId: string;
		ref: string;
		source?: string;
		createdBy?: string | null;
		/** Which application type's check is silenced. A group spans several,
		 * so the same name reported by another type is another silence.
		 * Defaults to a Tamanu central, as `seedServer` does. */
		applicationType?: ApplicationType;
	},
): Promise<void> {
	const source = opts.source ?? "alertd";
	const check = refToCheck(opts.ref);
	const ns = namespaceOf(source, check, opts.applicationType ?? "tamanu-central");
	await sql.query(
		`INSERT INTO scoped_check_policies (server_group_id, source, subject, application_type, check_name, ceiling, created_by)
		 VALUES ($1, $2, $3, $4, $5, 'skipped', $6)
		 ON CONFLICT DO NOTHING`,
		[
			opts.groupId,
			source,
			ns.subject,
			ns.applicationType,
			check,
			opts.createdBy ?? null,
		],
	);
}

/** Cache row for a Tailscale user's display info, as the auth layer
 * would have upserted it. Used to test avatar/name enrichment. */
export async function seedTailscaleUser(
	sql: Sql,
	opts: { login: string; name?: string; profilePic?: string | null },
): Promise<void> {
	await sql.query(
		`INSERT INTO tailscale_users (login, name, profile_pic) VALUES ($1, $2, $3)`,
		[
			opts.login,
			opts.name ?? opts.login.split("@")[0]!,
			opts.profilePic ?? null,
		],
	);
}

export interface SeededIssue {
	id: string;
}

export async function seedIssue(
	sql: Sql,
	opts: {
		/** Server-scoped issue. Mutually exclusive with `serverGroupId` — the
		 * `issues` scope CHECK allows at most one set. */
		serverId?: string | null;
		/** Machine-scoped issue: a fact about the box rather than a workload on
		 * it. Mutually exclusive with the other two. */
		machineId?: string | null;
		/** Group-scoped issue (e.g. a backup issue spanning the group). When set,
		 * leave `serverId` unset so the row satisfies the scope constraint.
		 * Leaving all unset seeds a canopy-wide issue (a self-alert). */
		serverGroupId?: string | null;
		source?: string;
		ref?: string;
		severity?: string;
		message?: string;
		description?: string | null;
		active?: boolean;
		deviceId?: string | null;
		/** Mark the issue resolved. `resolvedBy` null (with `resolved: true`)
		 * is the "retired on its own" case — the healthcheck recovered with
		 * no operator attributed. A login string attributes it to that
		 * operator (seed a matching tailscale_user for the name lookup). */
		resolved?: boolean;
		resolvedBy?: string | null;
		resolvedReason?: string | null;
		/** ISO 8601 timestamp for `first_seen`. Defaults to NOW(). Set it in
		 * the past to test "since when" displays (e.g. the per-healthcheck
		 * page's failing-since column). */
		firstSeen?: string;
		/** Structured detail the condition attached, as the filing path stores
		 * it. The self-alert surface links from this. */
		detail?: unknown;
	},
): Promise<SeededIssue> {
	const id = randomUUID();
	const resolved = opts.resolved ?? false;
	const active = resolved ? false : (opts.active ?? true);
	const severity = opts.severity ?? "error";
	// Mirror the filing path's result stamping so seeded rows look like
	// real check state: active rows degraded per the legacy severity the
	// caller speaks, closed rows recovered. Critical means escalating.
	const result = !active
		? "passed"
		: severity === "warning"
			? "warning"
			: "failed";
	const check = (opts.ref ?? "health").replace(/^health\//, "");
	// A server-scoped check-state only presents/counts if a live catalog row
	// backs it (mirrors ingestion's upsert_default); never clobbers an
	// explicit seedCheckPolicy for the same (source, check).
	if (opts.serverId || opts.machineId) {
		const source = opts.source ?? "alertd";
		// A machine-scoped issue names a check about the box, which derives to
		// the machine namespace whatever type is passed; the fallback only
		// matters for a server-scoped one, and there the application says.
		const ns = namespaceOf(
			source,
			check,
			opts.serverId
				? await applicationTypeOf(sql, opts.serverId)
				: "tamanu-central",
		);
		await sql.query(
			`INSERT INTO check_policies (source, subject, application_type, check_name)
			 VALUES ($1, $2, $3, $4)
			 ON CONFLICT (source, subject, application_type, check_name) DO NOTHING`,
			[source, ns.subject, ns.applicationType, check],
		);
	}
	// Every seeded issue was degraded at some point — that's what makes it
	// an issue rather than healthy check state, which the listings exclude.
	await sql.query(
		`INSERT INTO issues
		 (id, application_id, machine_id, server_group_id, device_id, source, ref, check_name, observed_result, effective_result, escalates, message, description, active, first_seen, last_seen, resolved_at, resolved_by, resolved_reason, degraded_since, last_degraded_at, detail)
		 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $9, $10, $11, $12, $13, COALESCE($14::timestamptz, NOW()), NOW(), $15, $16, $17, $18, NOW(), $19)`,
		[
			id,
			opts.serverId ?? null,
			opts.machineId ?? null,
			opts.serverGroupId ?? null,
			opts.deviceId ?? null,
			opts.source ?? "alertd",
			opts.ref ?? "health",
			check,
			result,
			severity === "critical",
			opts.message ?? "Issue message",
			opts.description ?? null,
			active,
			opts.firstSeen ?? null,
			resolved ? new Date().toISOString() : null,
			resolved ? (opts.resolvedBy ?? null) : null,
			resolved ? (opts.resolvedReason ?? null) : null,
			active ? (opts.firstSeen ?? new Date().toISOString()) : null,
			opts.detail === undefined ? null : JSON.stringify(opts.detail),
		],
	);
	return { id };
}

export interface SeededIncident {
	id: string;
}

/** Seed an open incident directly, optionally lingering (`closingAt` set:
 * its last effective failure recovered then and it closes if things stay
 * quiet) and optionally linking issues into its timeline. */
export async function seedIncident(
	sql: Sql,
	opts: {
		/** Group the incident targets; null/absent seeds a canopy-wide one. */
		serverGroupId?: string | null;
		/** ISO 8601; defaults to NOW(). */
		openedAt?: string;
		/** ISO 8601; sets the incident lingering since this time. */
		closingAt?: string | null;
		/** Issues to link, each with optional join/leave times. */
		issues?: Array<{
			issueId: string;
			joinedAt?: string;
			leftAt?: string | null;
		}>;
	} = {},
): Promise<SeededIncident> {
	const id = randomUUID();
	await sql.query(
		`INSERT INTO incidents (id, server_group_id, opened_at, closing_at)
		 VALUES ($1, $2, COALESCE($3::timestamptz, NOW()), $4::timestamptz)`,
		[id, opts.serverGroupId ?? null, opts.openedAt ?? null, opts.closingAt ?? null],
	);
	for (const link of opts.issues ?? []) {
		await sql.query(
			`INSERT INTO incident_issues (incident_id, issue_id, joined_at, left_at)
			 VALUES ($1, $2, COALESCE($3::timestamptz, NOW()), $4::timestamptz)`,
			[id, link.issueId, link.joinedAt ?? null, link.leftAt ?? null],
		);
	}
	return { id };
}

/** Add an operator note to an incident's timeline. */
export async function seedIncidentNote(
	sql: Sql,
	opts: {
		incidentId: string;
		author?: string;
		body?: string;
		/** ISO 8601; defaults to NOW(). */
		createdAt?: string;
	},
): Promise<{ id: string }> {
	const id = randomUUID();
	await sql.query(
		`INSERT INTO incident_notes (id, incident_id, author, body, created_at)
		 VALUES ($1, $2, $3, $4, COALESCE($5::timestamptz, NOW()))`,
		[
			id,
			opts.incidentId,
			opts.author ?? "operator@example.com",
			opts.body ?? "a note",
			opts.createdAt ?? null,
		],
	);
	return { id };
}

export interface SeededVersion {
	id: string;
	major: number;
	minor: number;
	patch: number;
}

/** Flag a version and every later patch in its minor as carrying a known issue.
 * Pass `fixedIn` to close the range at the first unaffected patch. */
export async function seedVersionKnownIssue(
	sql: Sql,
	opts: {
		major: number;
		minor: number;
		patch: number;
		fixedIn?: number;
		description?: string;
	},
): Promise<void> {
	await sql.query(
		`INSERT INTO version_known_issues
			(author, description, min_major, min_minor, min_patch, max_major, max_minor, max_patch)
		 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)`,
		[
			"e2e@example.com",
			opts.description ?? "breaks on upgrade",
			opts.major,
			opts.minor,
			opts.patch,
			opts.fixedIn == null ? null : opts.major,
			opts.fixedIn == null ? null : opts.minor,
			opts.fixedIn ?? null,
		],
	);
}

export async function seedVersion(
	sql: Sql,
	opts: {
		major?: number;
		minor?: number;
		patch?: number;
		status?: "draft" | "published" | "deprecated";
		changelog?: string;
	} = {},
): Promise<SeededVersion> {
	const id = randomUUID();
	const major = opts.major ?? 1;
	const minor = opts.minor ?? Math.floor(Math.random() * 1000);
	const patch = opts.patch ?? 0;
	await sql.query(
		`INSERT INTO versions (id, major, minor, patch, status, changelog)
		 VALUES ($1, $2, $3, $4, $5, $6)`,
		[id, major, minor, patch, opts.status ?? "published", opts.changelog ?? ""],
	);
	return { id, major, minor, patch };
}

/** Seed an artifact for a version. Naming a group makes Canopy hold the bytes
 * rather than record a location. */
export async function seedArtifact(
	sql: Sql,
	opts: {
		versionId?: string | null;
		artifactType?: string;
		platform?: string;
		downloadUrl?: string;
		rangePattern?: string | null;
		groupId?: string | null;
		content?: string;
	},
): Promise<string> {
	const id = randomUUID();
	const scoped = opts.groupId != null;
	const content = opts.content ?? "held bytes";
	const digest = scoped
		? `sha256:${createHash("sha256").update(content).digest("hex")}`
		: null;

	await sql.query(
		`INSERT INTO artifacts
			(id, version_id, artifact_type, platform, download_url, version_range_pattern,
			 group_id, content, content_type, digest)
		 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)`,
		[
			id,
			opts.versionId ?? null,
			opts.artifactType ?? "installer",
			opts.platform ?? "windows",
			scoped ? null : (opts.downloadUrl ?? "https://example.com/installer.exe"),
			opts.rangePattern ?? null,
			opts.groupId ?? null,
			scoped ? Buffer.from(content) : null,
			scoped ? "application/sql" : null,
			digest,
		],
	);
	return id;
}

// ── Backup-credentials seeding ──────────────────────────────────────────────

export type BackupConfigStatus = "provisioning" | "ready";
export type BackupRepoMode = "from_birth" | "passphrase";

/** Seed a `server_group_backup_config` row for a group, optionally with a
 * `(group, tamanu-postgres)` schedule. */
export async function seedServerGroupBackupConfig(
	sql: Sql,
	opts: {
		groupId: string;
		bucket?: string;
		prefix?: string;
		targetRoleArn?: string;
		maintenanceRoleArn?: string;
		region?: string | null;
		repoPasswordRef?: string;
		status?: BackupConfigStatus;
		mode?: BackupRepoMode;
		lastInitError?: string | null;
		/** Seconds; null = manual-only. Omit to skip seeding a schedule row. */
		intervalSeconds?: number | null;
		retention?: Record<string, number>;
	},
): Promise<void> {
	await sql.query(
		`INSERT INTO server_group_backup_config
		 (group_id, bucket, prefix, target_role_arn, maintenance_role_arn, region, repo_password_ref, status, mode, last_init_error)
		 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)`,
		[
			opts.groupId,
			opts.bucket ?? "bes-kopia-e2e",
			opts.prefix ?? "",
			opts.targetRoleArn ?? "arn:aws:iam::123456789012:role/e2e",
			opts.maintenanceRoleArn ?? "arn:aws:iam::123456789012:role/e2e-maint",
			opts.region ?? null,
			opts.repoPasswordRef ?? `backup-repo-${opts.groupId}`,
			opts.status ?? "ready",
			opts.mode ?? "from_birth",
			opts.lastInitError ?? null,
		],
	);
	if (opts.intervalSeconds !== undefined || opts.retention !== undefined) {
		const retention = opts.retention ?? {
			keep_latest: 1,
			keep_daily: 7,
			keep_weekly: 4,
			keep_monthly: 6,
			keep_annual: 0,
		};
		if (opts.intervalSeconds == null) {
			await sql.query(
				`INSERT INTO server_group_backup_schedule (group_id, type, expected_interval, retention)
				 VALUES ($1, 'tamanu-postgres', NULL, $2::jsonb)`,
				[opts.groupId, JSON.stringify(retention)],
			);
		} else {
			await sql.query(
				`INSERT INTO server_group_backup_schedule (group_id, type, expected_interval, retention)
				 VALUES ($1, 'tamanu-postgres', make_interval(secs => $2), $3::jsonb)`,
				[opts.groupId, opts.intervalSeconds, JSON.stringify(retention)],
			);
		}
	}
}

/** Seed a reported `backup_runs` row. */
export async function seedBackupRun(
	sql: Sql,
	opts: {
		deviceId: string;
		groupId: string;
		machineId?: string | null;
		type?: string;
		purpose?: "backup" | "restore";
		outcome?: "success" | "failure";
		error?: string | null;
		bytesUploaded?: number | null;
		snapshotId?: string | null;
		snapshotLogicalBytes?: number | null;
		s3SentRawBytes?: number | null;
		s3SentPayloadBytes?: number | null;
		s3ReceivedRawBytes?: number | null;
		s3ReceivedPayloadBytes?: number | null;
		/** Backdate `reported_at` by this many seconds (default: now). */
		reportedAgoSecs?: number;
		/** How long before now the run froze its data. Omit for a run that
		 * reported no freeze moment (the pre-progress client behaviour). */
		snapshotTakenAgoSecs?: number | null;
		/** Force the run's id, so progress samples can be correlated to it. */
		id?: string;
	},
): Promise<{ id: string }> {
	const id = opts.id ?? randomUUID();
	await sql.query(
		`INSERT INTO backup_runs
		 (id, device_id, group_id, machine_id, type, purpose, outcome, error, bytes_uploaded, snapshot_id,
		  s3_sent_raw_bytes, s3_sent_payload_bytes, s3_received_raw_bytes, s3_received_payload_bytes,
		  snapshot_logical_bytes, reported_at, snapshot_taken_at)
		 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
		  NOW() - make_interval(secs => $16),
		  CASE WHEN $17::float8 IS NULL THEN NULL ELSE NOW() - make_interval(secs => $17) END)`,
		[
			id,
			opts.deviceId,
			opts.groupId,
			opts.machineId ?? null,
			opts.type ?? "tamanu-postgres",
			opts.purpose ?? "backup",
			opts.outcome ?? "success",
			opts.error ?? null,
			opts.bytesUploaded ?? null,
			opts.snapshotId ?? null,
			opts.s3SentRawBytes ?? null,
			opts.s3SentPayloadBytes ?? null,
			opts.s3ReceivedRawBytes ?? null,
			opts.s3ReceivedPayloadBytes ?? null,
			opts.snapshotLogicalBytes ?? null,
			opts.reportedAgoSecs ?? 0,
			opts.snapshotTakenAgoSecs ?? null,
		],
	);
	return { id };
}

/** Seed a `backup_maintenance_runs` row. `finishedAgoSecs` backdates
 * `finished_at`, and `started_at` a further `durationSecs` (default 0) before
 * that; omit `outcome` for an in-flight run. */
export async function seedBackupMaintenanceRun(
	sql: Sql,
	opts: {
		groupId: string;
		kind?: "quick" | "full";
		outcome?: "success" | "failure" | null;
		error?: string | null;
		bytesReclaimed?: number | null;
		finishedAgoSecs?: number;
		durationSecs?: number;
	},
): Promise<void> {
	const ago = String(opts.finishedAgoSecs ?? 0);
	const startedAgo = String(
		(opts.finishedAgoSecs ?? 0) + (opts.durationSecs ?? 0),
	);
	const outcome = opts.outcome ?? null;
	await sql.query(
		`INSERT INTO backup_maintenance_runs
		 (group_id, kind, started_at, finished_at, outcome, error, bytes_reclaimed)
		 VALUES ($1, $2, NOW() - ($3 || ' seconds')::interval,
		         CASE WHEN $5::text IS NULL THEN NULL ELSE NOW() - ($4 || ' seconds')::interval END,
		         $5, $6, $7)`,
		[
			opts.groupId,
			opts.kind ?? "full",
			startedAgo,
			ago,
			outcome,
			opts.error ?? null,
			opts.bytesReclaimed ?? null,
		],
	);
}

/** Seed a `backup_credential_issuances` row. `issuedAgoSecs` controls how long
 * ago the creds were issued (default 0 = now); `ttlSecs` their lifetime. */
export async function seedBackupCredentialIssuance(
	sql: Sql,
	opts: {
		deviceId: string;
		groupId: string;
		type?: string;
		purpose?: "backup" | "restore";
		issuedAgoSecs?: number;
		ttlSecs?: number;
		/** Optional run correlation id, matching a run/check's run_id. */
		runId?: string | null;
	},
): Promise<void> {
	await sql.query(
		`INSERT INTO backup_credential_issuances
		 (device_id, group_id, type, issued_at, expires_at, purpose, sts_assumed_role, bucket, prefix, run_id)
		 VALUES ($1, $2, $3, NOW() - ($4 || ' seconds')::interval,
		         NOW() - ($4 || ' seconds')::interval + ($5 || ' seconds')::interval,
		         $6, $7, $8, $9, $10)`,
		[
			opts.deviceId,
			opts.groupId,
			opts.type ?? "tamanu-postgres",
			String(opts.issuedAgoSecs ?? 0),
			String(opts.ttlSecs ?? 3600),
			opts.purpose ?? "backup",
			"arn:aws:iam::000:role/test",
			"bes-test-bucket",
			"",
			opts.runId ?? null,
		],
	);
}

/** Seed a `backup_run_progress` sample.
 *
 * `observedAgoSecs` backdates `observed_at`, which is normally server-stamped on
 * receipt — a series is built by seeding several samples at decreasing ages.
 * Counters are cumulative from the start of the run, so each successive sample's
 * figures should be equal to or larger than the previous one's. */
export async function seedBackupRunProgress(
	sql: Sql,
	opts: {
		runId: string;
		deviceId: string;
		groupId: string;
		machineId?: string | null;
		type?: string;
		purpose?: "backup" | "restore";
		observedAgoSecs?: number;
		snapshotTakenAgoSecs?: number | null;
		bytesRead?: number | null;
		bytesHashed?: number | null;
		bytesUploaded?: number | null;
		bytesCached?: number | null;
		bytesEstimated?: number | null;
		filesDone?: number | null;
		filesEstimated?: number | null;
		errors?: number | null;
		ignoredErrors?: number | null;
		currentPath?: string | null;
		s3SentRawBytes?: number | null;
		s3SentPayloadBytes?: number | null;
		s3ReceivedRawBytes?: number | null;
		s3ReceivedPayloadBytes?: number | null;
		extra?: unknown;
	},
): Promise<void> {
	await sql.query(
		`INSERT INTO backup_run_progress
		 (run_id, device_id, group_id, machine_id, type, purpose, observed_at, snapshot_taken_at,
		  bytes_read, bytes_hashed, bytes_uploaded, bytes_cached, bytes_estimated,
		  files_done, files_estimated, errors, ignored_errors, current_path,
		  s3_sent_raw_bytes, s3_sent_payload_bytes, s3_received_raw_bytes, s3_received_payload_bytes,
		  extra)
		 VALUES ($1, $2, $3, $4, $5, $6,
		         NOW() - make_interval(secs => $7),
		         CASE WHEN $8::float8 IS NULL THEN NULL ELSE NOW() - make_interval(secs => $8) END,
		         $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23)`,
		[
			opts.runId,
			opts.deviceId,
			opts.groupId,
			opts.machineId ?? null,
			opts.type ?? "tamanu-postgres",
			opts.purpose ?? "backup",
			opts.observedAgoSecs ?? 0,
			opts.snapshotTakenAgoSecs ?? null,
			opts.bytesRead ?? null,
			opts.bytesHashed ?? null,
			opts.bytesUploaded ?? null,
			opts.bytesCached ?? null,
			opts.bytesEstimated ?? null,
			opts.filesDone ?? null,
			opts.filesEstimated ?? null,
			opts.errors ?? null,
			opts.ignoredErrors ?? null,
			opts.currentPath ?? null,
			opts.s3SentRawBytes ?? null,
			opts.s3SentPayloadBytes ?? null,
			opts.s3ReceivedRawBytes ?? null,
			opts.s3ReceivedPayloadBytes ?? null,
			JSON.stringify(opts.extra ?? {}),
		],
	);
}

/** Seed the cached `backup_repo_stats` row for a group. */
export async function seedBackupRepoStats(
	sql: Sql,
	opts: {
		groupId: string;
		snapshotCount?: number | null;
		sourceCount?: number | null;
		logicalBytes?: number | null;
		physicalBytes?: number | null;
		bucketBytes?: number | null;
		/** ISO timestamp of the bucket-bytes measurement. */
		bucketBytesObservedAt?: string | null;
	},
): Promise<void> {
	await sql.query(
		`INSERT INTO backup_repo_stats
		 (group_id, snapshot_count, source_count, logical_bytes, physical_bytes, bucket_bytes, bucket_bytes_observed_at)
		 VALUES ($1, $2, $3, $4, $5, $6, $7)`,
		[
			opts.groupId,
			opts.snapshotCount ?? null,
			opts.sourceCount ?? null,
			opts.logicalBytes ?? null,
			opts.physicalBytes ?? null,
			opts.bucketBytes ?? null,
			opts.bucketBytesObservedAt ?? null,
		],
	);
}

/** Seed a `machine_backup_capabilities` row (what a box advertises it can back
 * up, plus the operator-set enabled flag). A capability is the machine's. */
export async function seedServerBackupCapability(
	sql: Sql,
	opts: {
		machineId: string;
		type?: string;
		enabled?: boolean;
	},
): Promise<void> {
	await sql.query(
		`INSERT INTO machine_backup_capabilities (machine_id, type, enabled)
		 VALUES ($1, $2, $3)`,
		[opts.machineId, opts.type ?? "tamanu-postgres", opts.enabled ?? true],
	);
}

/** Seed a pending `backup_requests` row (one-off "backup now"). */
export async function seedBackupRequest(
	sql: Sql,
	opts: {
		machineId: string;
		type?: string;
		purpose?: "backup" | "restore";
		requestedBy?: string | null;
	},
): Promise<void> {
	await sql.query(
		`INSERT INTO backup_requests (machine_id, type, purpose, requested_by)
		 VALUES ($1, $2, $3, $4)`,
		[
			opts.machineId,
			opts.type ?? "tamanu-postgres",
			opts.purpose ?? "backup",
			opts.requestedBy ?? null,
		],
	);
}

/** One advertised intent: a bare name (no semantics/params) or a full
 * descriptor with the Canopy semantics it opts into and its parameter schema. */
export type SeedIntent =
	| string
	| {
			intent: string;
			description?: string | null;
			semantics?: string[];
			params?: Record<string, unknown>;
	  };

/** Register the intents a restore consumer (a `backup-restore` device)
 * advertises, with their descriptions, semantics, and parameter schemas. */
export async function seedRestoreConsumerCapability(
	sql: Sql,
	opts: { deviceId: string; intents: SeedIntent[] },
): Promise<void> {
	for (const raw of opts.intents) {
		const d = typeof raw === "string" ? { intent: raw } : raw;
		await sql.query(
			`INSERT INTO restore_consumer_capabilities
			 (consumer_device_id, intent, description, semantics, params)
			 VALUES ($1, $2, $3, $4::jsonb, $5::jsonb)`,
			[
				opts.deviceId,
				d.intent,
				d.description ?? null,
				JSON.stringify(d.semantics ?? []),
				JSON.stringify(d.params ?? {}),
			],
		);
	}
}

export interface SeededRestoreReplica {
	id: string;
}

/** Seed a declared restore replica. */
export async function seedRestoreReplica(
	sql: Sql,
	opts: {
		consumerDeviceId: string;
		groupId: string;
		/** The machine whose snapshot is restored. Omit for a whole-group declaration. */
		machineId?: string | null;
		type?: string;
		intent?: string;
		name?: string;
		/** Whole seconds; omit for "no overdue bound". */
		overdueAfterSeconds?: number | null;
		/** Operator-supplied parameter values. */
		params?: Record<string, unknown>;
		enabled?: boolean;
		/** Whether the replica is served de-identified. */
		redacts?: boolean;
	},
): Promise<SeededRestoreReplica> {
	const id = randomUUID();
	const overdue = opts.overdueAfterSeconds ?? null;
	const params = JSON.stringify(opts.params ?? {});
	if (overdue == null) {
		await sql.query(
			`INSERT INTO restore_replicas
			 (id, consumer_device_id, group_id, machine_id, type, intent, name, params, enabled, redacts)
			 VALUES ($1, $2, $3, $4, $5, $6, $7, $8::jsonb, $9, $10)`,
			[
				id,
				opts.consumerDeviceId,
				opts.groupId,
				opts.machineId ?? null,
				opts.type ?? "tamanu-postgres",
				opts.intent ?? "verify",
				opts.name ?? randomLabel("replica"),
				params,
				opts.enabled ?? true,
				opts.redacts ?? false,
			],
		);
	} else {
		await sql.query(
			`INSERT INTO restore_replicas
			 (id, consumer_device_id, group_id, machine_id, type, intent, name, overdue_after, params, enabled, redacts)
			 VALUES ($1, $2, $3, $4, $5, $6, $7, make_interval(secs => $8), $9::jsonb, $10, $11)`,
			[
				id,
				opts.consumerDeviceId,
				opts.groupId,
				opts.machineId ?? null,
				opts.type ?? "tamanu-postgres",
				opts.intent ?? "verify",
				opts.name ?? randomLabel("replica"),
				overdue,
				params,
				opts.enabled ?? true,
				opts.redacts ?? false,
			],
		);
	}
	return { id };
}

/** Seed a restore-health report row. */
export async function seedRestoreCheck(
	sql: Sql,
	opts: {
		consumerDeviceId: string;
		groupId: string;
		machineId?: string | null;
		replicaId?: string | null;
		type?: string;
		intent?: string;
		snapshotId?: string | null;
		outcome?: "success" | "failure";
		replicaHealthy?: boolean;
		error?: string | null;
		postgresVersion?: string | null;
		/** Arbitrary consumer-sent health data, stored as jsonb. */
		healthDetails?: unknown;
		/** ISO 8601; defaults to NOW(). */
		observedAt?: string;
		/** Optional run correlation id, matching a restore issuance's run_id. */
		runId?: string | null;
		/** What the masking manifest did, for a replica that redacts. */
		redaction?: {
			outcome: "complete" | "partial" | "failed";
			manifestVersion?: string | null;
			columnsMasked?: number | null;
			columnsSkipped?: number | null;
			error?: string | null;
		};
	},
): Promise<void> {
	const redaction = opts.redaction;
	await sql.query(
		`INSERT INTO backup_restore_checks
		 (replica_id, consumer_device_id, group_id, machine_id, type, intent, snapshot_id, outcome, error, replica_healthy, postgres_version, health_details, observed_at, run_id,
		  redaction_outcome, redaction_manifest_version, redaction_columns_masked, redaction_columns_skipped, redaction_error)
		 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12::jsonb, COALESCE($13::timestamptz, NOW()), $14, $15, $16, $17, $18, $19)`,
		[
			opts.replicaId ?? null,
			opts.consumerDeviceId,
			opts.groupId,
			opts.machineId ?? null,
			opts.type ?? "tamanu-postgres",
			opts.intent ?? "verify",
			opts.snapshotId ?? null,
			opts.outcome ?? "success",
			opts.error ?? null,
			opts.replicaHealthy ?? true,
			opts.postgresVersion ?? null,
			opts.healthDetails === undefined ? null : JSON.stringify(opts.healthDetails),
			opts.observedAt ?? null,
			opts.runId ?? null,
			redaction?.outcome ?? null,
			redaction?.manifestVersion ?? null,
			redaction?.columnsMasked ?? null,
			redaction?.columnsSkipped ?? null,
			redaction?.error ?? null,
		],
	);
}

/** Seed a migration-test result: the restore-health report that carries the
 * common fields, plus the migration outcome hung off it. A named
 * `failedMigration` is what makes the verdict a failure. */
export async function seedMigrationTest(
	sql: Sql,
	opts: {
		consumerDeviceId: string;
		groupId: string;
		/** The machine whose snapshot was restored. */
		machineId: string;
		/** The application whose candidate version was tried. */
		applicationId: string;
		targetVersionId: string;
		snapshotId?: string;
		failedMigration?: string | null;
		totalElapsedSecs?: number;
		dataBytesBefore?: number;
		dataBytesAfter?: number;
		timings?: Array<{ name: string; elapsedSecs: number }>;
	},
): Promise<void> {
	const rows = await sql.query<{ id: string }>(
		`INSERT INTO backup_restore_checks
		 (consumer_device_id, group_id, machine_id, type, intent, snapshot_id, outcome,
		  replica_healthy, observed_at)
		 VALUES ($1, $2, $3, 'tamanu-postgres', 'migrate', $4, 'success', true, NOW())
		 RETURNING id`,
		[opts.consumerDeviceId, opts.groupId, opts.machineId, opts.snapshotId ?? "snap-1"],
	);
	const checkId = rows[0]!.id;

	await sql.query(
		`INSERT INTO migration_tests
		 (check_id, application_id, target_version_id, total_elapsed, failed_migration,
		  data_bytes_before, data_bytes_after)
		 VALUES ($1, $2, $3, make_interval(secs => $4), $5, $6, $7)`,
		[
			checkId,
			opts.applicationId,
			opts.targetVersionId,
			opts.totalElapsedSecs ?? 60,
			opts.failedMigration ?? null,
			opts.dataBytesBefore ?? 0,
			opts.dataBytesAfter ?? 0,
		],
	);

	for (const [ordinal, timing] of (opts.timings ?? []).entries()) {
		await sql.query(
			`INSERT INTO migration_timings (check_id, ordinal, name, elapsed)
			 VALUES ($1, $2, $3, make_interval(secs => $4))`,
			[checkId, ordinal, timing.name, timing.elapsedSecs],
		);
	}
}

/** Record where a group is going. `plannedFor` is `YYYY-MM-DD`; omit for a plan
 * with no date. */
export interface SeededMaintenanceWindow {
	id: string;
}

/** A maintenance window over a machine or a group. `endsInHours` places the
 * expected end, so a negative value seeds one the sweep will end. */
export async function seedMaintenanceWindow(
	sql: Sql,
	opts: {
		machineId?: string;
		serverGroupId?: string;
		endsInHours?: number;
		/** Seed the window already ended this many minutes ago (still inside
		 * the settle period when under 10). */
		endedMinutesAgo?: number;
		note?: string | null;
		declaredBy?: string | null;
	},
): Promise<SeededMaintenanceWindow> {
	const rows = await sql.query<{ id: string }>(
		`INSERT INTO maintenance_windows
		   (machine_id, server_group_id, expected_end, ended_at, note, declared_by)
		 VALUES ($1, $2, NOW() + make_interval(mins => $3),
		         CASE WHEN $4::int IS NULL THEN NULL ELSE NOW() - make_interval(mins => $4::int) END,
		         $5, $6)
		 RETURNING id`,
		[
			opts.machineId ?? null,
			opts.serverGroupId ?? null,
			opts.endedMinutesAgo != null
				? -opts.endedMinutesAgo
				: Math.round((opts.endsInHours ?? 2) * 60),
			opts.endedMinutesAgo ?? null,
			opts.note ?? null,
			opts.declaredBy ?? "seed@bes.au",
		],
	);
	return { id: rows[0]!.id };
}

export async function seedUpgradePlan(
	sql: Sql,
	opts: {
		groupId: string;
		targetVersionId: string;
		plannedFor?: string | null;
		plannedTime?: string | null;
		plannedEndTime?: string | null;
		plannedZone?: string | null;
		note?: string | null;
		createdBy?: string;
		/** Retire the group's open plan first, as recording a second one does.
		 * A group holds one open plan at a time, so a second insert without this
		 * breaks the unique index. */
		supersedes?: boolean;
	},
): Promise<void> {
	if (opts.supersedes) {
		await sql.query(
			`UPDATE upgrade_plans SET superseded_at = NOW()
			 WHERE group_id = $1
			   AND met_at IS NULL AND superseded_at IS NULL AND withdrawn_at IS NULL`,
			[opts.groupId],
		);
	}
	await sql.query(
		`INSERT INTO upgrade_plans
		   (group_id, target_version_id, planned_for, planned_time, planned_end_time,
		    planned_zone, note, created_by)
		 VALUES ($1, $2, $3::date, $4::time, $5::time, $6, $7, $8)`,
		[
			opts.groupId,
			opts.targetVersionId,
			opts.plannedFor ?? null,
			opts.plannedTime ?? null,
			opts.plannedEndTime ?? null,
			opts.plannedZone ?? null,
			opts.note ?? null,
			opts.createdBy ?? "e2e@example.com",
		],
	);
}

/** Seed a `recovery_vault_writes` row. `writtenAgoSecs` backdates the write;
 * omit for NOW(). */
export async function seedRecoveryVaultWrite(
	sql: Sql,
	opts: { bytes?: number; writtenAgoSecs?: number } = {},
): Promise<void> {
	await sql.query(
		`INSERT INTO recovery_vault_writes (written_at, bytes)
		 VALUES (NOW() - make_interval(secs => $1), $2)`,
		[opts.writtenAgoSecs ?? 0, opts.bytes ?? 4096],
	);
}

/** Seed a `server_group_domains` row: a domain the group controls. Claims must
 * not overlap, so give each seeded group its own name. */
export async function seedServerGroupDomain(
	sql: Sql,
	opts: { groupId: string; domain: string; createdBy?: string },
): Promise<{ id: string; domain: string }> {
	const id = randomUUID();
	await sql.query(
		`INSERT INTO server_group_domains (id, group_id, domain, created_by)
		 VALUES ($1, $2, $3, $4)`,
		[id, opts.groupId, opts.domain, opts.createdBy ?? null],
	);
	return { id, domain: opts.domain };
}

/** Seed a `server_names` row: a public name a server registered, with the
 * addresses it asked for and (optionally) the ones Canopy has published. Leave
 * `publishedAddresses` unset for a registration the zone has not caught up with. */
export async function seedServerName(
	sql: Sql,
	opts: {
		serverId: string;
		name: string;
		addresses: string[];
		publishedAddresses?: string[];
		lastError?: string;
	},
): Promise<{ id: string; name: string }> {
	const id = randomUUID();
	const toInet = (addresses: string[]) =>
		`{${addresses.map((a) => `"${a}"`).join(",")}}`;
	await sql.query(
		`INSERT INTO application_names
		   (id, application_id, name, addresses, published_addresses, published_at, last_error)
		 VALUES ($1, $2, $3, $4::inet[], $5::inet[], $6, $7)`,
		[
			id,
			opts.serverId,
			opts.name,
			toInet(opts.addresses),
			toInet(opts.publishedAddresses ?? []),
			opts.publishedAddresses && opts.publishedAddresses.length > 0
				? new Date()
				: null,
			opts.lastError ?? null,
		],
	);
	return { id, name: opts.name };
}

/** Seed a `server_certificates` row.
 *
 * `expiresInDays` and `lifetimeDays` place the certificate anywhere in its life,
 * which is what drives the risk grading — that is a fraction of each
 * certificate's own lifetime rather than a fixed duration, so both are needed. */
export async function seedServerCertificate(
	sql: Sql,
	opts: {
		serverId: string;
		name: string;
		state?: "pending" | "issued" | "failed" | "revoked";
		keyFingerprint?: string;
		profile?: string;
		expiresInDays?: number;
		lifetimeDays?: number;
		renewing?: boolean;
		attempts?: number;
		lastError?: string;
		revokedBy?: string;
		revocationReason?: string;
	},
): Promise<{ id: string; name: string }> {
	const id = randomUUID();
	const state = opts.state ?? "issued";
	const issued = state === "issued" || state === "revoked";
	const lifetimeDays = opts.lifetimeDays ?? 90;
	const expiresInDays = opts.expiresInDays ?? 80;
	const notAfter = issued
		? new Date(Date.now() + expiresInDays * 86400_000)
		: null;
	const issuedAt = issued
		? new Date(Date.now() - (lifetimeDays - expiresInDays) * 86400_000)
		: null;
	await sql.query(
		`INSERT INTO application_certificates
		   (id, application_id, name, key_fingerprint, csr, state, chain, not_after,
		    issued_at, renewing, attempts, last_error, profile, renew_after,
		    revoked_at, revoked_by, revocation_reason)
		 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)`,
		[
			id,
			opts.serverId,
			opts.name,
			opts.keyFingerprint ?? randomUUID().replace(/-/g, "").repeat(2).slice(0, 64),
			Buffer.from("csr"),
			state,
			issued ? "-----BEGIN CERTIFICATE-----\n" : null,
			notAfter,
			issuedAt,
			opts.renewing ?? false,
			opts.attempts ?? 0,
			opts.lastError ?? null,
			opts.profile ?? null,
			// A third of the life left is where Canopy would renew, so this is what
			// makes an aged certificate read as overdue rather than merely old.
			issued
				? new Date(
						notAfter!.getTime() - (lifetimeDays / 3) * 86400_000,
					)
				: null,
			state === "revoked" ? new Date() : null,
			opts.revokedBy ?? null,
			opts.revocationReason ?? null,
		],
	);
	return { id, name: opts.name };
}
