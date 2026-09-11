import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedDevice,
	seedMigrationTest,
	seedServer,
	seedServerGroup,
	seedStatus,
	seedUpgradePlan,
	seedVersion,
} from "./seed";

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

		// One server has been tried and its migrations failed; the other has not
		// been tried yet.
		await seedMigrationTest(sql, {
			consumerDeviceId: consumer.id,
			groupId: group.id,
			machineId: failed.machineId,
			applicationId: failed.id,
			targetVersionId: target.id,
			failedMigration: "backfillNoteTypeIds",
			totalElapsedSecs: 5400,
			dataBytesBefore: 200_000_000_000,
			dataBytesAfter: 260_000_000_000,
			timings: [
				{ name: "addIndexToFhirJobs", elapsedSecs: 12 },
				{ name: "backfillNoteTypeIds", elapsedSecs: 5388 },
			],
		});

		await page.goto(`/fleet/groups/${group.id}`);

		const section = page.getByTestId("migration-tests");
		await expect(section).toBeVisible();
		await expect(
			section.getByRole("heading", { name: "Pre-upgrade migration tests" }),
		).toBeVisible();

		const failedRow = section
			.getByTestId("migration-test-row")
			.filter({ hasText: "kamaka-central" });
		await expect(failedRow).toContainText("2.63.0");
		await expect(failedRow).toContainText("failed");
		// The window estimate and the growth a heavy backfill leaves behind are
		// the numbers an operator schedules against.
		await expect(failedRow).toContainText("1.5h");
		await expect(failedRow).toContainText("30%");

		const untestedRow = section
			.getByTestId("migration-test-row")
			.filter({ hasText: "kamaka-facility" });
		await expect(untestedRow).toContainText("not yet tested");

		// Which migration to blame for the window is only visible expanded.
		await failedRow.getByRole("button", { name: "Show migrations" }).click();
		const timings = section.getByTestId("migration-timings");
		await expect(timings).toContainText("addIndexToFhirJobs");
		await expect(timings).toContainText("12s");
		const culprit = timings
			.getByRole("row")
			.filter({ hasText: "backfillNoteTypeIds" });
		await expect(culprit).toContainText("failed");
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
});
