import type { Sql } from "./seed";
import {
	resetSeededTables,
	seedApplicationReport,
	seedDevice,
	seedReportingSchemaBuild,
	seedRestoreConsumerCapability,
	seedRestoreReplica,
	seedServer,
	seedServerGroup,
	seedVersion,
} from "./seed";
import { expect, test } from "./test-fixtures";

/// A consumer that advertises a schema-building intent, declared against the
/// group. That declaration is what brings the group's pairs into being: canopy
/// owes a schema only where something is there to build one.
///
/// spec: RPT#pairs
async function declareBuilder(sql: Sql, groupId: string): Promise<string> {
	const consumer = await seedDevice(sql, { role: "backup-restore" });
	await seedRestoreConsumerCapability(sql, {
		deviceId: consumer.id,
		intents: [
			{
				intent: "reporting-schema",
				semantics: ["check", "once", "migrate", "reporting-schema"],
			},
		],
	});
	await seedRestoreReplica(sql, {
		consumerDeviceId: consumer.id,
		groupId,
		intent: "reporting-schema",
		name: "kamaka-schemas",
	});
	return consumer.id;
}

/// How a group's reporting-schema pairs are presented, and how an operator asks
/// for one to be built.
///
/// spec: RPT
test.describe("reporting schemas", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	/// One pair per version the group's Tamanu applications report running. A
	/// facility mid-rollout is on a different version from its central, so both
	/// are pairs: a schema follows the version rather than the application.
	///
	/// spec: RPT#pairs
	test("a group shows a pair per version its applications run", async ({
		page,
		sql,
	}) => {
		await seedVersion(sql, { major: 2, minor: 59, patch: 0, status: "published" });
		await seedVersion(sql, { major: 2, minor: 60, patch: 0, status: "published" });
		const group = await seedServerGroup(sql, { name: "kamaka" });
		await declareBuilder(sql, group.id);

		const central = await seedServer(sql, {
			name: "central",
			groupId: group.id,
			type: "tamanu-central",
		});
		const facility = await seedServer(sql, {
			name: "facility",
			groupId: group.id,
			type: "tamanu-facility",
		});
		await seedApplicationReport(sql, {
			applicationId: central.id,
			version: "2.60.0",
		});
		await seedApplicationReport(sql, {
			applicationId: facility.id,
			version: "2.59.0",
		});

		await page.goto(`/groups/${group.id}`);

		const section = page.getByTestId("reporting-schemas");
		await expect(section).toBeVisible();
		await expect(section.getByTestId("reporting-schema-row")).toHaveCount(2);
		await expect(section.getByText("2.59.0")).toBeVisible();
		await expect(section.getByText("2.60.0")).toBeVisible();

		// Nothing has been built yet, so both are awaiting one.
		await expect(section.getByText("Awaiting build")).toHaveCount(2);
	});

	/// An operator asking for a build is what reinstates a pair, so the ask has
	/// to be visible once made.
	///
	/// spec: RPT#pairs
	test("asking for a build records the ask", async ({ page, sql }) => {
		await seedVersion(sql, { major: 2, minor: 60, patch: 0, status: "published" });
		const group = await seedServerGroup(sql, { name: "kamaka" });
		await declareBuilder(sql, group.id);
		const central = await seedServer(sql, {
			name: "central",
			groupId: group.id,
			type: "tamanu-central",
		});
		await seedApplicationReport(sql, {
			applicationId: central.id,
			version: "2.60.0",
		});

		await page.goto(`/groups/${group.id}`);

		const section = page.getByTestId("reporting-schemas");
		await section.getByRole("button", { name: "Build sooner" }).click();

		await expect(section.getByText("Build asked for")).toBeVisible();

		const asks = await sql.query("SELECT requested_by FROM reporting_schema_requests");
		expect(asks).toHaveLength(1);
	});

	/// A group nothing builds schemas for is owed none, so it shows no pairs
	/// even where its applications report published versions. Listing them
	/// would offer an operator a build nothing will pick up, and a row stuck on
	/// "Awaiting build" reads as a backlog rather than as an absent builder.
	///
	/// spec: RPT#pairs
	test("a group with no builder declared shows no pairs", async ({
		page,
		sql,
	}) => {
		await seedVersion(sql, { major: 2, minor: 60, patch: 0, status: "published" });
		const group = await seedServerGroup(sql, { name: "drifting" });
		const central = await seedServer(sql, {
			name: "central",
			groupId: group.id,
			type: "tamanu-central",
		});
		await seedApplicationReport(sql, {
			applicationId: central.id,
			version: "2.60.0",
		});

		await page.goto(`/groups/${group.id}`);

		const section = page.getByTestId("reporting-schemas");
		await expect(section).toBeVisible();
		await expect(section.getByTestId("reporting-schema-row")).toHaveCount(0);
		await expect(section.getByText(/no builder is declared/i)).toBeVisible();
	});

	/// A built pair and a failed one read differently on the screen, and the
	/// failed one carries the builder's own description, which is the only
	/// place an operator can read why it failed.
	///
	/// spec: RPT#presentation
	test("a built pair and a failed one read differently", async ({
		page,
		sql,
	}) => {
		const built = await seedVersion(sql, {
			major: 2,
			minor: 59,
			patch: 0,
			status: "published",
		});
		const failed = await seedVersion(sql, {
			major: 2,
			minor: 60,
			patch: 0,
			status: "published",
		});
		const group = await seedServerGroup(sql, { name: "kamaka" });
		const consumer = await declareBuilder(sql, group.id);

		const central = await seedServer(sql, {
			name: "central",
			groupId: group.id,
			type: "tamanu-central",
		});
		const facility = await seedServer(sql, {
			name: "facility",
			groupId: group.id,
			type: "tamanu-facility",
		});
		await seedApplicationReport(sql, {
			applicationId: central.id,
			version: "2.60.0",
		});
		await seedApplicationReport(sql, {
			applicationId: facility.id,
			version: "2.59.0",
		});

		await seedReportingSchemaBuild(sql, {
			consumerDeviceId: consumer,
			groupId: group.id,
			machineId: central.machineId,
			applicationId: central.id,
			versionId: built.id,
			built: true,
		});
		await seedReportingSchemaBuild(sql, {
			consumerDeviceId: consumer,
			groupId: group.id,
			machineId: central.machineId,
			applicationId: central.id,
			versionId: failed.id,
			built: false,
			error: "views did not compile",
		});

		await page.goto(`/groups/${group.id}`);

		const section = page.getByTestId("reporting-schemas");
		await expect(section.getByText("Built", { exact: true })).toBeVisible();
		await expect(section.getByText("Failed", { exact: true })).toBeVisible();
		await expect(
			section.getByText("Awaiting build", { exact: true }),
		).toHaveCount(0);

		// The description is only reachable by hovering the chip, which is the
		// whole of an operator's access to why the build failed.
		await section.getByText("Failed", { exact: true }).hover();
		await expect(page.getByText("views did not compile")).toBeVisible();

		// A settled pair offers a rebuild rather than a first build.
		await expect(
			section.getByRole("button", { name: "Build again" }),
		).toHaveCount(2);
	});
});
