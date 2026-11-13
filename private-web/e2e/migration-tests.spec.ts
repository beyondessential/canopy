import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedDevice,
	seedMigrationTest,
	seedRestoreConsumerCapability,
	seedRestoreReplica,
	seedServer,
	seedServerGroup,
	seedStatus,
	seedUpgradePlan,
	seedVersion,
} from "./seed";

/** Declare a migrating replica for the consumer, over the whole group or over
 * one machine: what sets a server up for testing. */
async function declareTesting(
	sql: Parameters<typeof seedRestoreReplica>[0],
	consumerDeviceId: string,
	groupId: string,
	machineId?: string,
): Promise<void> {
	await seedRestoreConsumerCapability(sql, {
		deviceId: consumerDeviceId,
		intents: [{ intent: "upgrade", semantics: ["check", "once", "migrate"] }],
	});
	await seedRestoreReplica(sql, {
		consumerDeviceId,
		groupId,
		machineId,
		intent: "upgrade",
	});
}

test.describe("pre-upgrade migration tests on the group page", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("shows each server's verdict against the version it would take next", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka" });
		const consumer = await seedDevice(sql, { role: "backup-restore" });
		await seedVersion(sql, { major: 2, minor: 62, patch: 0 });
		const target = await seedVersion(sql, { major: 2, minor: 63, patch: 0 });

		const failed = await seedServer(sql, {
			name: "kamaka-central",
			groupId: group.id,
		});
		const untested = await seedServer(sql, {
			name: "kamaka-facility",
			groupId: group.id,
		});
		for (const server of [failed, untested]) {
			await seedStatus(sql, { serverId: server.id, version: "2.62.0" });
		}
		await seedUpgradePlan(sql, {
			groupId: group.id,
			targetVersionId: target.id,
		});
		await declareTesting(sql, consumer.id, group.id);

		// One server has been tried and its migrations failed; the other has not
		// been tried yet.
		await seedMigrationTest(sql, {
			consumerDeviceId: consumer.id,
			groupId: group.id,
			machineId: failed.machineId,
			applicationId: failed.id,
			targetVersionId: target.id,
			failedMigration: "backfillNoteTypeIds",
			error: 'column "note_type_id" does not exist',
			totalElapsedSecs: 5400,
			dataBytesBefore: 200_000_000_000,
			dataBytesAfter: 260_000_000_000,
			timings: [
				{ name: "addIndexToFhirJobs", elapsedSecs: 12 },
				{ name: "backfillNoteTypeIds", elapsedSecs: 5388 },
			],
			postMigration: {
				elapsedSecs: 1860,
				steps: [
					{ name: "encounters", rows: 341_744, elapsedSecs: 298 },
					{ name: "self-heal", rows: null, elapsedSecs: 840 },
				],
			},
		});

		await page.goto(`/fleet/groups/${group.id}`);

		const section = page.getByTestId("migration-tests");
		await expect(section).toBeVisible();
		await expect(
			section.getByRole("heading", { name: "Pre-upgrade migration tests" }),
		).toBeVisible();
		// Every row is going to the same version, so the heading says it once.
		await expect(section).toContainText("to 2.63.0");
		await expect(
			section.getByRole("columnheader", { name: "Upgrading to" }),
		).toHaveCount(0);
		await expect(section.getByTestId("migration-tests-not-set-up")).toHaveCount(0);

		const failedRow = section
			.getByTestId("migration-test-row")
			.filter({ hasText: "kamaka-central" });
		await expect(failedRow).toContainText("failed");
		await expect(failedRow.getByRole("button", { name: "Test again" })).toBeVisible();
		// The window estimate is the number an operator schedules against.
		await expect(failedRow).toContainText("1.5h");

		const untestedRow = section
			.getByTestId("migration-test-row")
			.filter({ hasText: "kamaka-facility" });
		await expect(untestedRow).toContainText("not yet tested");
		await expect(untestedRow.getByRole("button", { name: "Test now" })).toBeVisible();

		// Which migration broke, and what it said.
		await failedRow.getByText("failed").hover();
		const tip = page.getByRole("tooltip");
		await expect(tip).toContainText("backfillNoteTypeIds");
		await expect(tip).toContainText('column "note_type_id" does not exist');

		// Which migration to blame for the window takes opening the run.
		await failedRow.getByRole("button", { name: "Show migrations" }).click();
		const run = page.getByTestId("migration-run");
		await expect(run).toContainText("kamaka-central");
		await expect(run).toContainText('column "note_type_id" does not exist');
		const timings = run.getByTestId("migration-timing");
		await expect(timings.filter({ hasText: "addIndexToFhirJobs" })).toContainText(
			"12s",
		);
		await expect(
			timings.filter({ hasText: "backfillNoteTypeIds" }),
		).toContainText("1.5h");

		// The work after the migrations is part of the same window.
		await expect(run).toContainText("After migrations took");
		const steps = run.getByTestId("post-migration-timing");
		await expect(steps).toHaveCount(2);
		await expect(steps.filter({ hasText: "encounters" })).toContainText("341,744");

		// The open run is in the URL, so the link shares the migrations.
		await expect(page).toHaveURL(new RegExp(`migrations=${failed.id}`));
		await page.reload();
		await expect(page.getByTestId("migration-run")).toContainText(
			"backfillNoteTypeIds",
		);

		await page.getByRole("button", { name: "Close" }).click();
		await expect(page.getByTestId("migration-run")).toBeHidden();
	});

	/// spec: FLT#naming
	test("an application nobody has named reads as its type", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "unnamed-tests" });
		await seedVersion(sql, { major: 2, minor: 62, patch: 0 });
		const target = await seedVersion(sql, { major: 2, minor: 63, patch: 0 });
		const server = await seedServer(sql, {
			name: null,
			type: "tamanu-facility",
			groupId: group.id,
		});
		await seedStatus(sql, { serverId: server.id, version: "2.62.0" });
		await seedUpgradePlan(sql, {
			groupId: group.id,
			targetVersionId: target.id,
		});
		const consumer = await seedDevice(sql, { role: "backup-restore" });
		await declareTesting(sql, consumer.id, group.id);

		await page.goto(`/fleet/groups/${group.id}`);
		const row = page.getByTestId("migration-test-row");
		await expect(row).toContainText("Tamanu facility");
		await expect(row).not.toContainText(server.id);
	});

	test("says so when the group has no open plan", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "unplanned" });
		await seedVersion(sql, { major: 2, minor: 63, patch: 0 });
		const server = await seedServer(sql, {
			name: "unplanned-central",
			groupId: group.id,
		});
		await seedStatus(sql, { serverId: server.id, version: "2.62.0" });

		await page.goto(`/fleet/groups/${group.id}`);

		await expect(page.getByTestId("migration-tests")).toContainText(
			"nothing to test against",
		);
	});

	test("a link to the section opens on it", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "drifting" });
		await seedVersion(sql, { major: 2, minor: 63, patch: 0 });
		const server = await seedServer(sql, {
			name: "drifting-central",
			groupId: group.id,
		});
		await seedStatus(sql, { serverId: server.id, version: "2.62.0" });

		await page.setViewportSize({ width: 1280, height: 400 });
		await page.goto(`/fleet/groups/${group.id}#migration-tests`);

		await expect(page.getByTestId("migration-tests")).toBeInViewport();
	});

	test("only servers set up for testing get a row", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "kamaka" });
		const consumer = await seedDevice(sql, { role: "backup-restore" });
		await seedVersion(sql, { major: 2, minor: 62, patch: 0 });
		const target = await seedVersion(sql, { major: 2, minor: 63, patch: 0 });
		const central = await seedServer(sql, {
			name: "kamaka-central",
			groupId: group.id,
		});
		const facility = await seedServer(sql, {
			name: "kamaka-facility",
			type: "tamanu-facility",
			groupId: group.id,
		});
		for (const server of [central, facility]) {
			await seedStatus(sql, { serverId: server.id, version: "2.62.0" });
		}
		await seedUpgradePlan(sql, {
			groupId: group.id,
			targetVersionId: target.id,
		});
		await declareTesting(sql, consumer.id, group.id, central.machineId);

		await page.goto(`/fleet/groups/${group.id}`);

		const section = page.getByTestId("migration-tests");
		await expect(section.getByTestId("migration-test-row")).toHaveCount(1);
		await expect(section.getByTestId("migration-test-row")).toContainText(
			"kamaka-central",
		);
		const rest = section.getByTestId("migration-tests-not-set-up");
		await expect(rest).toContainText("1 other server is not set up for testing");
		await expect(rest.getByTestId("migration-test-not-set-up")).toBeHidden();
		await rest.getByRole("button", { name: "Show" }).click();
		const listed = rest.getByTestId("migration-test-not-set-up");
		await expect(listed).toContainText("kamaka-facility");
		await expect(listed).toContainText("production");
	});

	/// spec: RST#dispatching-a-migration-test
	test("a server's environment is tested when asked from its row", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka" });
		const consumer = await seedDevice(sql, { role: "backup-restore" });
		await seedVersion(sql, { major: 2, minor: 62, patch: 0 });
		const target = await seedVersion(sql, { major: 2, minor: 63, patch: 0 });
		const central = await seedServer(sql, {
			name: "kamaka-central",
			groupId: group.id,
		});
		await seedStatus(sql, { serverId: central.id, version: "2.62.0" });
		await seedUpgradePlan(sql, {
			groupId: group.id,
			targetVersionId: target.id,
		});
		await declareTesting(sql, consumer.id, group.id);

		await page.goto(`/fleet/groups/${group.id}`);
		const row = page.getByTestId("migration-test-row");
		await row.getByRole("button", { name: "Test now" }).click();

		await expect(row.getByTestId("migration-test-requested")).toBeVisible();
		await expect(row.getByRole("button", { name: "Test now" })).toHaveCount(0);
		const requests = await sql.query<{ machine_id: string }>(
			"SELECT machine_id FROM migration_test_requests",
		);
		expect(requests.map((r) => r.machine_id)).toEqual([central.machineId]);
	});

	test("the fleet view names the failure and links to the detail", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka" });
		const consumer = await seedDevice(sql, { role: "backup-restore" });
		await seedVersion(sql, { major: 2, minor: 62, patch: 0 });
		const target = await seedVersion(sql, { major: 2, minor: 63, patch: 0 });

		const failed = await seedServer(sql, {
			name: "kamaka-central",
			groupId: group.id,
		});
		await seedStatus(sql, { serverId: failed.id, version: "2.62.0" });
		await seedUpgradePlan(sql, {
			groupId: group.id,
			targetVersionId: target.id,
		});
		await declareTesting(sql, consumer.id, group.id, failed.machineId);
		await seedMigrationTest(sql, {
			consumerDeviceId: consumer.id,
			groupId: group.id,
			machineId: failed.machineId,
			applicationId: failed.id,
			targetVersionId: target.id,
			failedMigration: "backfillNoteTypeIds",
			error: 'column "note_type_id" does not exist',
			totalElapsedSecs: 5400,
			dataBytesBefore: 200_000_000_000,
			dataBytesAfter: 260_000_000_000,
			timings: [{ name: "backfillNoteTypeIds", elapsedSecs: 5388 }],
		});

		await page.goto("/upgrades");

		const header = page
			.getByTestId("planned-group-header")
			.filter({ hasText: "kamaka" });
		await expect(header.getByTestId("migration-verdict-detail")).toContainText(
			"kamaka-central · backfillNoteTypeIds",
		);
		const verdict = header.getByTestId("migration-verdict");
		// Which application broke and why, without opening the group.
		await verdict.hover();
		const tip = page.getByRole("tooltip");
		await expect(tip).toContainText("kamaka-central");
		await expect(tip).toContainText("backfillNoteTypeIds");
		await expect(tip).toContainText('column "note_type_id" does not exist');

		await verdict.click();
		await expect(page).toHaveURL(new RegExp(`/fleet/groups/${group.id}`));
		await expect(page.getByTestId("migration-tests")).toBeVisible();
	});
});
