import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedBackupCredentialIssuance,
	seedDevice,
	seedRestoreCheck,
	seedRestoreConsumerCapability,
	seedRestoreReplica,
	seedServer,
	seedServerGroup,
	seedServerGroupBackupConfig,
	type Sql,
} from "./seed";

// The e2e fixture runs the private-server in a debug build, so the Tailscale
// auth bypass treats every caller as `admin@localhost` (an admin).
//
// The restore-replica UI lives inside each group's backup page
// (`/fleet/groups/:id/backups`, shown once the group has a ready backup config); the
// fleet-wide consumer roster lives in Settings.

test.describe("restore replicas", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	/** A group with a ready backup config, so its backup page renders the panels
	 * (including the restore-replicas section). */
	async function groupWithBackups(
		sql: Sql,
		name: string,
	): Promise<string> {
		const group = await seedServerGroup(sql, { name });
		await seedServerGroupBackupConfig(sql, { groupId: group.id, status: "ready" });
		return group.id;
	}

	test("empty state shows the no-declarations banner", async ({ page, sql }) => {
		const groupId = await groupWithBackups(sql, "empty-group");
		await page.goto(`/fleet/groups/${groupId}/backups`);
		await expect(
			page.getByText(/no restore replicas declared for this group/i),
		).toBeVisible();
	});

	test("restore checks show a measured duration and an in-progress restore", async ({
		page,
		sql,
	}) => {
		const consumer = await seedDevice(sql, { role: "backup-restore" });
		const groupId = await groupWithBackups(sql, "rr-duration");
		const server = await seedServer(sql, { name: "rr-srv", groupId });
		const reportedRun = "11111111-1111-1111-1111-111111111111";
		const inflightRun = "22222222-2222-2222-2222-222222222222";

		// A reported check plus the issuance that started it 5 minutes before it
		// reported → the row carries a 5m duration.
		await seedRestoreCheck(sql, {
			consumerDeviceId: consumer.id,
			groupId,
			machineId: server.machineId,
			intent: "verify",
			outcome: "success",
			replicaHealthy: true,
			runId: reportedRun,
		});
		await seedBackupCredentialIssuance(sql, {
			deviceId: consumer.id,
			groupId,
			purpose: "restore",
			issuedAgoSecs: 300,
			runId: reportedRun,
		});
		// A consumer restore still in flight (creds valid, no report yet).
		await seedBackupCredentialIssuance(sql, {
			deviceId: consumer.id,
			groupId,
			purpose: "restore",
			issuedAgoSecs: 30,
			ttlSecs: 3600,
			runId: inflightRun,
		});

		await page.goto(`/fleet/groups/${groupId}/backups`);
		await expect(page.getByText(/recent restore checks/i)).toBeVisible();
		// The reported check's row shows its Canopy-measured duration.
		await expect(page.getByRole("row", { name: /verify/ })).toContainText("5m");
		// The unreported restore surfaces as in progress.
		await expect(page.getByRole("row", { name: /in progress/i })).toBeVisible();
	});

	test("a seeded declaration renders; an unsupported intent is flagged as a gap", async ({
		page,
		sql,
	}) => {
		const consumer = await seedDevice(sql, { role: "backup-restore" });
		await seedRestoreConsumerCapability(sql, {
			deviceId: consumer.id,
			intents: ["verify"],
		});
		const groupId = await groupWithBackups(sql, "rr-group");

		await seedRestoreReplica(sql, {
			consumerDeviceId: consumer.id,
			groupId,
			intent: "verify",
			name: "verify-all",
		});
		await seedRestoreReplica(sql, {
			consumerDeviceId: consumer.id,
			groupId,
			intent: "analytics",
			name: "analytics-all",
		});

		await page.goto(`/fleet/groups/${groupId}/backups`);

		const verifyRow = page.getByRole("row", { name: /verify-all/ });
		const analyticsRow = page.getByRole("row", { name: /analytics-all/ });
		await expect(verifyRow).toBeVisible();
		await expect(analyticsRow).toBeVisible();
		await expect(analyticsRow.getByText("gap")).toBeVisible();
		await expect(verifyRow.getByText("gap")).toHaveCount(0);
	});

	/** A consumer advertising `analytics` with `redact` and the three masking
	 * parameters Canopy takes over. */
	async function redactingConsumer(sql: Sql): Promise<string> {
		const consumer = await seedDevice(sql, { role: "backup-restore" });
		await seedRestoreConsumerCapability(sql, {
			deviceId: consumer.id,
			intents: [
				{
					intent: "analytics",
					semantics: ["check", "url", "redact"],
					params: {
						minimum_uptime: { type: "duration", default: 7200 },
						redaction_manifest_url: { type: "text" },
						redaction_version_query: { type: "text" },
						redaction_version_fallback_to_base: {
							type: "boolean",
							default: false,
						},
					},
				},
			],
		});
		return consumer.id;
	}

	test("declaring a redacting replica offers the switch, not the manifest fields", async ({
		page,
		sql,
	}) => {
		const consumer = await redactingConsumer(sql);
		const groupId = await groupWithBackups(sql, "redact-declare");
		await seedServer(sql, { groupId, name: "redact-srv" });

		await page.goto(`/fleet/groups/${groupId}/backups`);
		await page.getByRole("button", { name: /declare replica/i }).click();

		const dialog = page.getByRole("dialog");
		await expect(dialog.getByText(/redact this replica/i)).toBeVisible();
		// Canopy owns these, so they get no field for an operator to fill in.
		await expect(dialog.getByLabel(/^redaction_manifest_url/)).toHaveCount(0);
		await expect(dialog.getByLabel(/^redaction_version_query/)).toHaveCount(0);
		// Other parameters of the same intent are unaffected.
		await expect(dialog.getByLabel(/^minimum_uptime/)).toBeVisible();

		await dialog.getByRole("switch", { name: /redact this replica/i }).check();
		await dialog.getByRole("button", { name: "Declare" }).click();

		await expect(dialog).toHaveCount(0);
		const rows = await sql.query<{ redacts: boolean }>(
			`SELECT redacts FROM restore_replicas WHERE consumer_device_id = $1`,
			[consumer],
		);
		expect(rows[0]?.redacts).toBe(true);
	});

	/** A consumer advertising an intent that builds reporting schemas. */
	async function schemaBuildingConsumer(sql: Sql): Promise<string> {
		const consumer = await seedDevice(sql, { role: "backup-restore" });
		await seedRestoreConsumerCapability(sql, {
			deviceId: consumer.id,
			intents: [
				{
					intent: "schema-build",
					semantics: ["check", "once", "migrate", "reporting-schema"],
				},
			],
		});
		return consumer.id;
	}

	/// Publishing a group's schema is the operator's grant, so it is set on the
	/// declaration rather than followed from what the consumer advertises.
	///
	/// spec: RPT#the-build-contract
	test("an operator marks which declaration publishes the group's schema", async ({
		page,
		sql,
	}) => {
		const consumer = await schemaBuildingConsumer(sql);
		const groupId = await groupWithBackups(sql, "publish-declare");
		await seedServer(sql, { groupId, name: "publish-srv" });

		await page.goto(`/fleet/groups/${groupId}/backups`);
		await page.getByRole("button", { name: /declare replica/i }).click();

		const dialog = page.getByRole("dialog");
		const publishes = dialog.getByRole("switch", {
			name: /publish this group's reporting schema/i,
		});
		await expect(publishes).not.toBeChecked();
		await publishes.check();
		await dialog.getByRole("button", { name: /^declare$/i }).click();

		await expect(dialog).toHaveCount(0);
		const rows = await sql.query<{ publishes_schemas: boolean }>(
			`SELECT publishes_schemas FROM restore_replicas WHERE consumer_device_id = $1`,
			[consumer],
		);
		expect(rows[0]?.publishes_schemas).toBe(true);

		await expect(
			page.getByRole("row", { name: /publish-declare/ }).first(),
		).toBeVisible();
		await expect(page.getByText("publishes schema").first()).toBeVisible();
	});

	/// A build is dispatched per group from data the masking manifest has not
	/// altered, so a declaration narrowed to one machine cannot be the group's
	/// publisher.
	///
	/// spec: RPT#the-build-contract
	test("a machine-scoped declaration cannot publish the schema", async ({
		page,
		sql,
	}) => {
		await schemaBuildingConsumer(sql);
		const groupId = await groupWithBackups(sql, "publish-scope");
		await seedServer(sql, { groupId, name: "publish-one" });

		await page.goto(`/fleet/groups/${groupId}/backups`);
		await page.getByRole("button", { name: /declare replica/i }).click();

		const dialog = page.getByRole("dialog");
		const publishes = dialog.getByRole("switch", {
			name: /publish this group's reporting schema/i,
		});
		await publishes.check();
		await expect(publishes).toBeChecked();

		await dialog.getByLabel("Machine").click();
		await page.getByRole("option", { name: "publish-one" }).click();

		await expect(publishes).toBeDisabled();
		await expect(publishes).not.toBeChecked();
	});

	test("a partial redaction shows against the report that carried it", async ({
		page,
		sql,
	}) => {
		const consumer = await redactingConsumer(sql);
		const groupId = await groupWithBackups(sql, "redact-partial");
		const server = await seedServer(sql, { groupId, name: "partial-srv" });
		const replica = await seedRestoreReplica(sql, {
			consumerDeviceId: consumer,
			groupId,
			intent: "analytics",
			name: "masked-analytics",
			redacts: true,
		});
		// The restore is healthy and the redaction is not: two signals from one
		// report.
		await seedRestoreCheck(sql, {
			consumerDeviceId: consumer,
			groupId,
			machineId: server.machineId,
			replicaId: replica.id,
			intent: "analytics",
			snapshotId: "snap-1",
			outcome: "success",
			replicaHealthy: true,
			redaction: {
				outcome: "partial",
				manifestVersion: "2.41.3",
				columnsMasked: 118,
				columnsSkipped: 3,
			},
		});

		await page.goto(`/fleet/groups/${groupId}/backups`);

		// The declaration asked for masking; what it got is what the list has
		// to show, so a partial redaction reads as partial from the row.
		await expect(
			page.getByRole("row", { name: /masked-analytics/ }),
		).toContainText("partial");
		const reportRow = page.getByRole("row", { name: /analytics/ }).last();
		await expect(reportRow).toContainText("healthy");
		await expect(reportRow).toContainText("partial");

		// The counts and manifest version are promoted out of the health-details
		// JSON, so the columns left in the clear are readable without one.
		await reportRow.getByRole("button", { name: /show health details/i }).click();
		await expect(page.getByText("Columns left in the clear: 3")).toBeVisible();
		await expect(page.getByText("Manifest version: 2.41.3")).toBeVisible();
	});

	test("a redacting declaration names the servers it can't redact", async ({
		page,
		sql,
	}) => {
		const consumer = await redactingConsumer(sql);
		const groupId = await groupWithBackups(sql, "redact-gap");
		await seedServer(sql, { groupId, name: "tamanu-srv" });
		await seedServer(sql, {
			groupId,
			name: "lims-srv",
			type: "senaite",
		});
		await seedRestoreReplica(sql, {
			consumerDeviceId: consumer,
			groupId,
			intent: "analytics",
			name: "group-wide-redacted",
			redacts: true,
		});

		await page.goto(`/fleet/groups/${groupId}/backups`);
		await expect(
			page.getByRole("row", { name: /group-wide-redacted/ }),
		).toContainText("1 unmaskable");
	});

	test("settings lists restore consumers and their capabilities", async ({
		page,
		sql,
	}) => {
		const consumer = await seedDevice(sql, { role: "backup-restore" });
		await seedRestoreConsumerCapability(sql, {
			deviceId: consumer.id,
			intents: ["verify", "disaster-recovery"],
		});

		await page.goto("/settings/restore-consumers");
		await expect(page.getByText("verify").first()).toBeVisible();
		await expect(page.getByText("disaster-recovery").first()).toBeVisible();
	});

	test("deleting a declaration removes it", async ({ page, sql }) => {
		const consumer = await seedDevice(sql, { role: "backup-restore" });
		await seedRestoreConsumerCapability(sql, {
			deviceId: consumer.id,
			intents: ["verify"],
		});
		const groupId = await groupWithBackups(sql, "del-group");
		const server = await seedServer(sql, { groupId, name: "del-srv" });
		const replica = await seedRestoreReplica(sql, {
			consumerDeviceId: consumer.id,
			groupId,
			intent: "verify",
			name: "doomed",
		});
		// A declaration that has been reported on deletes like any other; its
		// reports are kept, detached.
		await seedRestoreCheck(sql, {
			consumerDeviceId: consumer.id,
			groupId,
			machineId: server.machineId,
			replicaId: replica.id,
			snapshotId: "snap-1",
		});

		await page.goto(`/fleet/groups/${groupId}/backups`);
		await expect(page.getByRole("row", { name: /doomed/ })).toBeVisible();
		await page.getByRole("button", { name: "delete doomed" }).click();
		await expect(page.getByRole("row", { name: /doomed/ })).toHaveCount(0);

		const rows = await sql.query<{ count: string }>(
			"SELECT count(*) AS count FROM restore_replicas",
		);
		expect(Number(rows[0]!.count)).toBe(0);

		const checks = await sql.query<{ replica_id: string | null }>(
			"SELECT replica_id FROM backup_restore_checks",
		);
		expect(checks).toHaveLength(1);
		expect(checks[0]!.replica_id).toBeNull();
	});

	test("toggling enabled flips the row in the database", async ({
		page,
		sql,
	}) => {
		const consumer = await seedDevice(sql, { role: "backup-restore" });
		await seedRestoreConsumerCapability(sql, {
			deviceId: consumer.id,
			intents: ["verify"],
		});
		const groupId = await groupWithBackups(sql, "tog-group");
		const replica = await seedRestoreReplica(sql, {
			consumerDeviceId: consumer.id,
			groupId,
			intent: "verify",
			name: "togglable",
			enabled: true,
		});

		await page.goto(`/fleet/groups/${groupId}/backups`);
		await page
			.getByRole("row", { name: /togglable/ })
			.locator('input[type="checkbox"]')
			.click();

		await expect
			.poll(async () => {
				const rows = await sql.query<{ enabled: boolean }>(
					"SELECT enabled FROM restore_replicas WHERE id = $1",
					[replica.id],
				);
				return rows[0]?.enabled;
			})
			.toBe(false);
	});

	test("recent restore checks render with their outcome", async ({
		page,
		sql,
	}) => {
		const consumer = await seedDevice(sql, { role: "backup-restore" });
		const groupId = await groupWithBackups(sql, "chk-group");
		const server = await seedServer(sql, { groupId, name: "chk-srv" });
		await seedRestoreCheck(sql, {
			consumerDeviceId: consumer.id,
			groupId,
			machineId: server.machineId,
			outcome: "failure",
			replicaHealthy: false,
			error: "restore blew up",
		});

		await page.goto(`/fleet/groups/${groupId}/backups`);
		await expect(page.getByText(/recent restore checks/i)).toBeVisible();
		await expect(page.getByText("failed")).toBeVisible();
	});

	test("a restore check shows its postgres version and expandable health details", async ({
		page,
		sql,
	}) => {
		const consumer = await seedDevice(sql, { role: "backup-restore" });
		const groupId = await groupWithBackups(sql, "health-group");
		const server = await seedServer(sql, { groupId, name: "health-srv" });
		await seedRestoreCheck(sql, {
			consumerDeviceId: consumer.id,
			groupId,
			machineId: server.machineId,
			outcome: "success",
			replicaHealthy: true,
			postgresVersion: "16.3",
			healthDetails: { indexes_fixed: true, live_tuples: 4242 },
		});

		await page.goto(`/fleet/groups/${groupId}/backups`);
		// The checks table shows the server as a truncated id, not its name, so
		// locate the check row by its own expand button rather than the server.
		const detailsButton = page.getByRole("button", {
			name: /show health details/i,
		});
		const row = page.getByRole("row").filter({ has: detailsButton });
		// The postgres version is now surfaced in the table.
		await expect(row.getByText("16.3")).toBeVisible();

		// Health details are collapsed until expanded, then shown as JSON.
		await expect(page.getByText(/live_tuples/)).toBeHidden();
		await detailsButton.click();
		await expect(page.getByText(/"indexes_fixed": true/)).toBeVisible();
		await expect(page.getByText(/"live_tuples": 4242/)).toBeVisible();
	});

	test("declaring a replica through the dialog persists it", async ({
		page,
		sql,
	}) => {
		const consumer = await seedDevice(sql, { role: "backup-restore" });
		await seedRestoreConsumerCapability(sql, {
			deviceId: consumer.id,
			intents: ["verify"],
		});
		const groupId = await groupWithBackups(sql, "create-group");
		await seedServer(sql, { groupId, name: "srv-a" });

		await page.goto(`/fleet/groups/${groupId}/backups`);
		await page.getByRole("button", { name: /declare replica/i }).click();

		const dialog = page.getByRole("dialog");
		await dialog.getByLabel("Consumer").click();
		await page.getByRole("option").first().click();
		await dialog.getByLabel("Name").fill("dialog-made");
		await dialog.getByRole("button", { name: /^declare$/i }).click();

		await expect(page.getByRole("row", { name: /dialog-made/ })).toBeVisible();
		const rows = await sql.query<{ name: string }>(
			"SELECT name FROM restore_replicas WHERE name = 'dialog-made'",
		);
		expect(rows).toHaveLength(1);
	});

	test("the dialog auto-selects the sole consumer and defaults the name", async ({
		page,
		sql,
	}) => {
		const consumer = await seedDevice(sql, { role: "backup-restore" });
		await seedRestoreConsumerCapability(sql, {
			deviceId: consumer.id,
			intents: ["verify"],
		});
		const groupId = await groupWithBackups(sql, "solo-group");
		await seedServer(sql, { groupId, name: "srv-a" });

		await page.goto(`/fleet/groups/${groupId}/backups`);
		await page.getByRole("button", { name: /declare replica/i }).click();
		const dialog = page.getByRole("dialog");

		// Name defaults to the kebab-cased group name and intent (whole-group
		// scope).
		await expect(dialog.getByLabel("Name")).toHaveValue("solo-group-verify");
		// Picking a machine folds its name into the default.
		await dialog.getByLabel("Machine").click();
		await page.getByRole("option", { name: "srv-a" }).click();
		await expect(dialog.getByLabel("Name")).toHaveValue(
			"solo-group-srv-a-verify",
		);

		// The consumer was never picked, yet Declare succeeds — the sole consumer
		// was auto-selected.
		await dialog.getByRole("button", { name: /^declare$/i }).click();
		await expect(
			page.getByRole("row", { name: /solo-group-srv-a-verify/ }),
		).toBeVisible();
	});

	test("a name already used by the consumer is refused", async ({
		page,
		sql,
	}) => {
		const consumer = await seedDevice(sql, { role: "backup-restore" });
		await seedRestoreConsumerCapability(sql, {
			deviceId: consumer.id,
			intents: ["verify", "analytics"],
		});
		const groupId = await groupWithBackups(sql, "dupe-group");
		await seedRestoreReplica(sql, {
			consumerDeviceId: consumer.id,
			groupId,
			intent: "verify",
			name: "taken",
		});

		await page.goto(`/fleet/groups/${groupId}/backups`);
		await page.getByRole("button", { name: /declare replica/i }).click();

		// A different intent is a different scope, but the name is the consumer's
		// already — the declaration is refused rather than silently duplicated.
		const dialog = page.getByRole("dialog");
		await dialog.getByLabel("Intent").click();
		await page.getByRole("option", { name: "analytics" }).click();
		await dialog.getByLabel("Name").fill("taken");
		await dialog.getByRole("button", { name: /^declare$/i }).click();

		await expect(dialog.getByRole("alert")).toContainText(/name/i);
		const rows = await sql.query<{ name: string }>(
			"SELECT name FROM restore_replicas WHERE name = 'taken'",
		);
		expect(rows).toHaveLength(1);
	});

	test("a second replica of one scope is declared under its own name", async ({
		page,
		sql,
	}) => {
		const consumer = await seedDevice(sql, { role: "backup-restore" });
		await seedRestoreConsumerCapability(sql, {
			deviceId: consumer.id,
			intents: ["verify"],
		});
		const groupId = await groupWithBackups(sql, "twice-group");
		await seedRestoreReplica(sql, {
			consumerDeviceId: consumer.id,
			groupId,
			intent: "verify",
			name: "twice-group-verify",
		});

		await page.goto(`/fleet/groups/${groupId}/backups`);
		await page.getByRole("button", { name: /declare replica/i }).click();
		const dialog = page.getByRole("dialog");

		// The scope is the one already declared, so the suggested name counts
		// past the one in use rather than arriving already taken.
		await expect(dialog.getByLabel("Name")).toHaveValue("twice-group-verify-2");
		await dialog.getByRole("button", { name: /^declare$/i }).click();

		// Both stand: the scope repeats, the names do not.
		await expect(
			page.getByRole("row", { name: /twice-group-verify-2/ }),
		).toBeVisible();
		const rows = await sql.query<{ name: string }>(
			"SELECT name FROM restore_replicas WHERE group_id = $1 ORDER BY name",
			[groupId],
		);
		expect(rows.map((r) => r.name)).toEqual([
			"twice-group-verify",
			"twice-group-verify-2",
		]);
	});

	test("the intent dropdown offers only intents the consumer registered", async ({
		page,
		sql,
	}) => {
		const consumer = await seedDevice(sql, { role: "backup-restore" });
		await seedRestoreConsumerCapability(sql, {
			deviceId: consumer.id,
			intents: ["verify"],
		});
		const groupId = await groupWithBackups(sql, "intent-group");

		await page.goto(`/fleet/groups/${groupId}/backups`);
		await page.getByRole("button", { name: /declare replica/i }).click();
		const dialog = page.getByRole("dialog");

		await dialog.getByLabel("Intent").click();
		await expect(page.getByRole("option", { name: "verify" })).toBeVisible();
		// The formerly-hardcoded well-known intents are gone.
		await expect(page.getByRole("option", { name: "analytics" })).toHaveCount(0);
		await expect(
			page.getByRole("option", { name: "disaster-recovery" }),
		).toHaveCount(0);
	});

	test("the dialog shows the intent description and typed parameter fields, and persists a value", async ({
		page,
		sql,
	}) => {
		const consumer = await seedDevice(sql, { role: "backup-restore" });
		await seedRestoreConsumerCapability(sql, {
			deviceId: consumer.id,
			intents: [
				{
					intent: "analytics",
					description: "Keeps a queryable replica running.",
					semantics: ["check", "url"],
					params: {
						minimum_uptime: { type: "duration", default: 7200 },
						anonymisation: { type: "boolean", default: true },
					},
				},
			],
		});
		const groupId = await groupWithBackups(sql, "param-group");

		await page.goto(`/fleet/groups/${groupId}/backups`);
		await page.getByRole("button", { name: /declare replica/i }).click();
		const dialog = page.getByRole("dialog");

		// The sole consumer is auto-selected and its sole intent chosen, so the
		// description and parameter fields for `analytics` render.
		await expect(dialog.getByText("Keeps a queryable replica running.")).toBeVisible();
		const uptime = dialog.getByLabel("minimum_uptime (duration)");
		await expect(uptime).toBeVisible();
		await expect(dialog.getByLabel("anonymisation")).toBeVisible();

		// Duration values are typed with human units and stored as raw seconds.
		await uptime.fill("1h");
		await dialog.getByLabel("Name").fill("with-params");
		await dialog.getByRole("button", { name: /^declare$/i }).click();

		await expect(page.getByRole("row", { name: /with-params/ })).toBeVisible();
		const rows = await sql.query<{ params: { minimum_uptime?: number } }>(
			"SELECT params FROM restore_replicas WHERE name = 'with-params'",
		);
		expect(rows).toHaveLength(1);
		expect(rows[0]!.params.minimum_uptime).toBe(3600);
	});

	test("editing a declaration through the dialog updates name, overdue bound, and enabled", async ({
		page,
		sql,
	}) => {
		const consumer = await seedDevice(sql, { role: "backup-restore" });
		await seedRestoreConsumerCapability(sql, {
			deviceId: consumer.id,
			intents: ["verify"],
		});
		const groupId = await groupWithBackups(sql, "edit-group");
		const replica = await seedRestoreReplica(sql, {
			consumerDeviceId: consumer.id,
			groupId,
			intent: "verify",
			name: "before-edit",
			overdueAfterSeconds: 3600,
			enabled: true,
		});

		await page.goto(`/fleet/groups/${groupId}/backups`);
		await page.getByRole("button", { name: "edit before-edit" }).click();

		const dialog = page.getByRole("dialog");
		await expect(dialog.getByLabel("Name")).toHaveValue("before-edit");
		// The stored 3600s bound is displayed in the human-friendly format.
		await expect(dialog.getByLabel("Overdue after (optional)")).toHaveValue("1h");

		await dialog.getByLabel("Name").fill("after-edit");
		await dialog.getByLabel("Overdue after (optional)").fill("4h");
		await dialog.getByLabel("Enabled").click();
		await dialog.getByRole("button", { name: /^save$/i }).click();

		await expect(page.getByRole("row", { name: /after-edit/ })).toBeVisible();
		await expect(page.getByRole("row", { name: /before-edit/ })).toHaveCount(0);

		const rows = await sql.query<{
			name: string;
			overdue_after_secs: string;
			enabled: boolean;
		}>(
			`SELECT name, enabled, EXTRACT(EPOCH FROM overdue_after)::text AS overdue_after_secs
			 FROM restore_replicas WHERE id = $1`,
			[replica.id],
		);
		expect(rows).toHaveLength(1);
		expect(rows[0]!.name).toBe("after-edit");
		expect(rows[0]!.enabled).toBe(false);
		expect(Number(rows[0]!.overdue_after_secs)).toBe(4 * 3600);
	});

	test("editing a declaration's parameters persists the typed values", async ({
		page,
		sql,
	}) => {
		const consumer = await seedDevice(sql, { role: "backup-restore" });
		await seedRestoreConsumerCapability(sql, {
			deviceId: consumer.id,
			intents: [
				{
					intent: "analytics",
					description: "Keeps a queryable replica running.",
					semantics: ["check", "url"],
					params: {
						minimum_uptime: { type: "duration", default: 7200 },
						anonymisation: { type: "boolean", default: true },
					},
				},
			],
		});
		const groupId = await groupWithBackups(sql, "edit-param-group");
		await seedRestoreReplica(sql, {
			consumerDeviceId: consumer.id,
			groupId,
			intent: "analytics",
			name: "param-edit",
			params: { minimum_uptime: 3600, anonymisation: true },
		});

		await page.goto(`/fleet/groups/${groupId}/backups`);
		await page.getByRole("button", { name: "edit param-edit" }).click();

		const dialog = page.getByRole("dialog");
		const uptime = dialog.getByLabel("minimum_uptime (duration)");
		// The stored raw 3600 seconds display as a human-friendly duration.
		await expect(uptime).toHaveValue("1h");
		await uptime.fill("30m");
		await dialog.getByRole("button", { name: /^save$/i }).click();

		await expect(page.getByRole("row", { name: /param-edit/ })).toBeVisible();
		const rows = await sql.query<{ params: { minimum_uptime?: number } }>(
			"SELECT params FROM restore_replicas WHERE name = 'param-edit'",
		);
		expect(rows).toHaveLength(1);
		expect(rows[0]!.params.minimum_uptime).toBe(1800);
	});

	test("size params accept 1024-based units and display in Kubernetes notation", async ({
		page,
		sql,
	}) => {
		const consumer = await seedDevice(sql, { role: "backup-restore" });
		await seedRestoreConsumerCapability(sql, {
			deviceId: consumer.id,
			intents: [
				{
					intent: "analytics",
					description: "Keeps a queryable replica running.",
					semantics: ["check", "url"],
					params: {
						minimum_uptime: { type: "duration", default: 7200 },
						max_disk: { type: "bytes" },
					},
				},
			],
		});
		const groupId = await groupWithBackups(sql, "size-group");

		await page.goto(`/fleet/groups/${groupId}/backups`);
		await page.getByRole("button", { name: /declare replica/i }).click();
		const dialog = page.getByRole("dialog");

		// Bare "20G" means 20Gi (1024-based), never 20×10⁹.
		await dialog.getByLabel("max_disk (size)").fill("20G");
		await dialog.getByLabel("minimum_uptime (duration)").fill("1h 30m");
		await dialog.getByLabel("Name").fill("sized");
		await dialog.getByRole("button", { name: /^declare$/i }).click();

		const row = page.getByRole("row", { name: /sized/ });
		await expect(row).toBeVisible();
		// The table's param summary shows the display forms.
		await expect(row).toContainText("max_disk=20Gi");
		await expect(row).toContainText("minimum_uptime=1h 30m");

		const rows = await sql.query<{
			params: { max_disk?: number; minimum_uptime?: number };
		}>("SELECT params FROM restore_replicas WHERE name = 'sized'");
		expect(rows).toHaveLength(1);
		expect(rows[0]!.params.max_disk).toBe(20 * 1024 ** 3);
		expect(rows[0]!.params.minimum_uptime).toBe(5400);

		// Reopening the edit dialog shows the display forms back in the fields.
		await page.getByRole("button", { name: "edit sized" }).click();
		const edit = page.getByRole("dialog");
		await expect(edit.getByLabel("max_disk (size)")).toHaveValue("20Gi");
		await expect(edit.getByLabel("minimum_uptime (duration)")).toHaveValue(
			"1h 30m",
		);
	});

	test("an invalid overdue bound is rejected inline and nothing is saved", async ({
		page,
		sql,
	}) => {
		const consumer = await seedDevice(sql, { role: "backup-restore" });
		await seedRestoreConsumerCapability(sql, {
			deviceId: consumer.id,
			intents: ["verify"],
		});
		const groupId = await groupWithBackups(sql, "badunit-group");

		await page.goto(`/fleet/groups/${groupId}/backups`);
		await page.getByRole("button", { name: /declare replica/i }).click();
		const dialog = page.getByRole("dialog");

		await dialog.getByLabel("Name").fill("bad-bound");
		await dialog.getByLabel("Overdue after (optional)").fill("banana");
		await dialog.getByRole("button", { name: /^declare$/i }).click();

		// The backend rejects the bound; the dialog stays open with the error.
		await expect(dialog.getByRole("alert")).toBeVisible();
		await expect(dialog).toBeVisible();
		const rows = await sql.query<{ count: string }>(
			"SELECT count(*) AS count FROM restore_replicas",
		);
		expect(Number(rows[0]!.count)).toBe(0);
	});

	test("editing a declaration's intent retargets it and re-derives its parameters", async ({
		page,
		sql,
	}) => {
		const consumer = await seedDevice(sql, { role: "backup-restore" });
		await seedRestoreConsumerCapability(sql, {
			deviceId: consumer.id,
			intents: [
				"verify",
				{
					intent: "analytics",
					description: "Keeps a queryable replica running.",
					semantics: ["check", "url"],
					params: { anonymisation: { type: "boolean", default: true } },
				},
			],
		});
		const groupId = await groupWithBackups(sql, "retarget-group");
		const replica = await seedRestoreReplica(sql, {
			consumerDeviceId: consumer.id,
			groupId,
			intent: "verify",
			name: "retarget-me",
		});

		await page.goto(`/fleet/groups/${groupId}/backups`);
		await page.getByRole("button", { name: "edit retarget-me" }).click();

		const dialog = page.getByRole("dialog");
		// Prefilled with the declaration's current intent.
		await expect(dialog.getByText("verify", { exact: true })).toBeVisible();
		await dialog.getByLabel("Intent").click();
		await page.getByRole("option", { name: "analytics" }).click();

		// Switching intent re-derives the parameter fields from analytics' schema.
		await expect(dialog.getByText("Keeps a queryable replica running.")).toBeVisible();
		await expect(dialog.getByLabel("anonymisation")).toBeVisible();

		await dialog.getByRole("button", { name: /^save$/i }).click();

		const row = page.getByRole("row", { name: /retarget-me/ });
		await expect(row).toBeVisible();
		await expect(row.getByText("analytics")).toBeVisible();
		await expect(row.getByText("gap")).toHaveCount(0);

		const rows = await sql.query<{ intent: string }>(
			"SELECT intent FROM restore_replicas WHERE id = $1",
			[replica.id],
		);
		expect(rows).toHaveLength(1);
		expect(rows[0]!.intent).toBe("analytics");
	});

	test("editing a declaration onto an existing declaration's scope is allowed, but not onto its name", async ({
		page,
		sql,
	}) => {
		const consumer = await seedDevice(sql, { role: "backup-restore" });
		await seedRestoreConsumerCapability(sql, {
			deviceId: consumer.id,
			intents: ["verify", "analytics"],
		});
		const groupId = await groupWithBackups(sql, "conflict-group");
		await seedRestoreReplica(sql, {
			consumerDeviceId: consumer.id,
			groupId,
			intent: "verify",
			name: "taken-scope",
		});
		await seedRestoreReplica(sql, {
			consumerDeviceId: consumer.id,
			groupId,
			intent: "analytics",
			name: "movable",
		});

		await page.goto(`/fleet/groups/${groupId}/backups`);
		await page.getByRole("button", { name: "edit movable" }).click();

		// Retargeting onto the other declaration's scope leaves two replicas of
		// one scope, which is what the name is there to tell apart.
		const dialog = page.getByRole("dialog");
		await dialog.getByLabel("Intent").click();
		await page.getByRole("option", { name: "verify" }).click();
		await dialog.getByRole("button", { name: /^save$/i }).click();

		await expect(dialog).toBeHidden();
		let rows = await sql.query<{ intent: string }>(
			"SELECT intent FROM restore_replicas WHERE name = 'movable'",
		);
		expect(rows).toHaveLength(1);
		expect(rows[0]!.intent).toBe("verify");

		// Taking the other declaration's name is what's refused.
		await page.getByRole("button", { name: "edit movable" }).click();
		const renaming = page.getByRole("dialog");
		await renaming.getByLabel("Name").fill("taken-scope");
		await renaming.getByRole("button", { name: /^save$/i }).click();

		await expect(renaming.getByRole("alert")).toContainText(/name/i);
		await expect(renaming).toBeVisible();
		rows = await sql.query<{ intent: string }>(
			"SELECT intent FROM restore_replicas WHERE name = 'movable'",
		);
		expect(rows).toHaveLength(1);
	});

	test("a restore check surfaces a replica url as a link", async ({
		page,
		sql,
	}) => {
		const consumer = await seedDevice(sql, { role: "backup-restore" });
		const groupId = await groupWithBackups(sql, "url-group");
		const server = await seedServer(sql, { groupId, name: "url-srv" });
		await seedRestoreCheck(sql, {
			consumerDeviceId: consumer.id,
			groupId,
			machineId: server.machineId,
			outcome: "success",
			replicaHealthy: true,
			healthDetails: { url: "https://replica.example.test/db" },
		});

		await page.goto(`/fleet/groups/${groupId}/backups`);
		const link = page.getByRole("link", { name: /open/i });
		await expect(link).toBeVisible();
		await expect(link).toHaveAttribute("href", "https://replica.example.test/db");
	});
});
