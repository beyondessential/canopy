import { expect, test } from "./test-fixtures";
import {
	seedCheckPolicy,
	resetSeededTables,
	seedGroupSilencedRef,
	seedIssue,
	seedServer,
	seedServerGroup,
	seedServerSilencedRef,
	seedStatus,
	seedVersion,
} from "./seed";

test.describe("server detail health indicator", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("shows the Unhealthy chip when the latest status is unhealthy", async ({
		page,
		sql,
	}) => {
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const server = await seedServer(sql, {
			name: "sick-server",
			type: "tamanu-central",
		});
		await seedStatus(sql, {
			serverId: server.id,
			healthy: false,
			health: [{ check: "database", healthy: false, message: "connection refused" }],
		});

		await page.goto(`/fleet/applications/${server.id}`);

		// Healthy chip lives in the InfoSection header.
		await expect(page.getByText("Unhealthy", { exact: true })).toBeVisible();
		await expect(
			page.getByText("Healthy", { exact: true }),
		).not.toBeVisible();
	});

	test("shows the Healthy chip when the latest status is healthy", async ({
		page,
		sql,
	}) => {
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const server = await seedServer(sql, {
			name: "ok-server",
			type: "tamanu-central",
		});
		await seedStatus(sql, {
			serverId: server.id,
			healthy: true,
			health: [],
		});

		await page.goto(`/fleet/applications/${server.id}`);

		await expect(page.getByText("Healthy", { exact: true })).toBeVisible();
	});
});

test.describe("server detail checks table", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("renders each check with its extras and orders failing first", async ({
		page,
		sql,
	}) => {
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const server = await seedServer(sql, {
			name: "checked-server",
			type: "tamanu-central",
		});
		await seedStatus(sql, {
			serverId: server.id,
			healthy: true,
			health: [
				{ check: "always-passes", healthy: true },
				{
					check: "almost-broken",
					healthy: false,
					hint: "free_pct: 2",
				},
			],
		});

		await page.goto(`/fleet/applications/${server.id}`);

		// Both check names are listed somewhere in the page.
		await expect(page.getByText("almost-broken")).toBeVisible();
		await expect(page.getByText("always-passes")).toBeVisible();

		// The failing entry surfaces its extras as a key/value line.
		await expect(page.getByText("hint")).toBeVisible();
		await expect(page.getByText("free_pct: 2")).toBeVisible();
	});

	test("merges checks from every source, each labelled with its source", async ({
		page,
		sql,
	}) => {
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const server = await seedServer(sql, {
			name: "multi-source-server",
			type: "tamanu-central",
		});
		// alertd reports a failing check (via a status push).
		await seedStatus(sql, {
			serverId: server.id,
			healthy: false,
			health: [{ check: "postgres", result: "failed" }],
		});
		// A second source reports its own check independently.
		await seedIssue(sql, {
			serverId: server.id,
			source: "tamanu",
			ref: "health/sync",
			severity: "warning",
		});

		await page.goto(`/fleet/applications/${server.id}`);

		// Both sources' checks render in the one consolidated table, each
		// linked and labelled with the source it came from.
		await expect(
			page.getByRole("link", { name: "postgres" }),
		).toBeVisible();
		await expect(page.getByRole("link", { name: "sync" })).toBeVisible();
		await expect(page.getByText("alertd", { exact: true }).first()).toBeVisible();
		await expect(page.getByText("tamanu", { exact: true }).first()).toBeVisible();
	});

	test("the ? button pops up the check's rendered documentation", async ({
		page,
		sql,
	}) => {
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const server = await seedServer(sql, {
			name: "documented-server",
			type: "tamanu-central",
		});
		await seedStatus(sql, {
			serverId: server.id,
			health: [
				{ check: "postgres", result: "failed" },
				{ check: "disk", result: "passed" },
			],
		});
		await seedCheckPolicy(sql, {
			checkName: "postgres",
			documentation:
				"## Description\n\nWatches the PostgreSQL connection.\n\n## Solve\n\nCheck pg_hba.conf.",
		});
		// Another source documents a same-named "disk" check: that document
		// must NOT leak into alertd's undocumented one below — documentation
		// is keyed per (source, check).
		await seedCheckPolicy(sql, {
			checkName: "disk",
			source: "seedling",
			documentation: "## Description\n\nSeedling's unrelated disk check.",
		});

		await page.goto(`/fleet/applications/${server.id}`);
		await page
			.getByRole("button", { name: "Documentation for postgres" })
			.click();
		await expect(
			page.getByText("Watches the PostgreSQL connection."),
		).toBeVisible();
		await expect(
			page.getByRole("link", { name: "Edit documentation" }),
		).toBeVisible();

		// An undocumented check still gets the affordance, prompting for
		// the missing document instead of hiding the icon — and another
		// source's same-named document doesn't leak in.
		await page.keyboard.press("Escape");
		await page.getByRole("button", { name: "Documentation for disk" }).click();
		await expect(
			page.getByText("Nobody has documented this check yet."),
		).toBeVisible();
		await expect(
			page.getByText("Seedling's unrelated disk check."),
		).not.toBeVisible();
		await expect(page.getByRole("link", { name: "Write it" })).toBeVisible();
	});

	test("sorts skipped checks last, after passing ones", async ({
		page,
		sql,
	}) => {
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const server = await seedServer(sql, {
			name: "skip-server",
			type: "tamanu-central",
		});
		// Names are chosen so alphabetical order is the reverse of the
		// expected result order: a discriminator proving the sort is by
		// result, not by name.
		await seedStatus(sql, {
			serverId: server.id,
			healthy: false,
			health: [
				{ check: "a-skips", result: "skipped" },
				{ check: "m-passes", result: "passed" },
				{ check: "z-fails", result: "failed" },
			],
		});

		await page.goto(`/fleet/applications/${server.id}`);

		const failed = page.getByText("z-fails");
		const passed = page.getByText("m-passes");
		const skipped = page.getByText("a-skips");
		await expect(failed).toBeVisible();
		await expect(passed).toBeVisible();
		await expect(skipped).toBeVisible();

		const failedY = (await failed.boundingBox())!.y;
		const passedY = (await passed.boundingBox())!.y;
		const skippedY = (await skipped.boundingBox())!.y;
		expect(failedY).toBeLessThan(passedY);
		expect(passedY).toBeLessThan(skippedY);
	});
});

test.describe("silenced healthchecks", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("a server-silenced failing check renders skip-style and doesn't count toward health", async ({
		page,
		sql,
	}) => {
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const server = await seedServer(sql, {
			name: "hushed-server",
			type: "tamanu-central",
		});
		// Named so alphabetical order would put the silenced check first:
		// its position after the passing check proves silenced sorts with
		// the skipped tail, not by name or by its raw failed result.
		await seedStatus(sql, {
			serverId: server.id,
			healthy: true,
			health: [
				{ check: "a-silenced", result: "failed" },
				{ check: "m-passes", result: "passed" },
			],
		});
		await seedServerSilencedRef(sql, {
			serverId: server.id,
			ref: "health/a-silenced",
		});

		await page.goto(`/fleet/applications/${server.id}`);

		// Headline rollup ignores the silenced failure.
		await expect(page.getByText("Healthy", { exact: true })).toBeVisible();
		await expect(
			page.getByText("Unhealthy", { exact: true }),
		).not.toBeVisible();

		// The row is still listed and flagged as silenced. The row labels the
		// check by its namespace, an application check being namespaced to
		// the type that reported it. Exact matching: the Silenced refs
		// section also shows the full "alertd/health/a-silenced" ref, which
		// substring-matches the bare name.
		await expect(
			page.getByText("tamanu-central:a-silenced", { exact: true }),
		).toBeVisible();
		await expect(
			page.getByText("silenced (application)"),
		).toBeVisible();
		await expect(
			page.getByRole("button", { name: "Manage silence for a-silenced" }),
		).toBeVisible();
		// The neutral silenced icon renders three times for the row (result
		// icon, silenced chip, admin silence button) and the red failure
		// icon not at all — neither in the row nor the headline chip.
		await expect(page.getByTestId("NotificationsOffIcon")).toHaveCount(3);
		await expect(page.getByTestId("CancelIcon")).toHaveCount(0);

		// And it sorts with the skipped tail, after passing checks.
		const passed = page.getByText("tamanu-central:m-passes", {
			exact: true,
		});
		const silenced = page.getByText("tamanu-central:a-silenced", {
			exact: true,
		});
		const passedY = (await passed.boundingBox())!.y;
		const silencedY = (await silenced.boundingBox())!.y;
		expect(passedY).toBeLessThan(silencedY);
	});

	test("a group-silenced failing check doesn't count toward member health", async ({
		page,
		sql,
	}) => {
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const group = await seedServerGroup(sql, { name: "hushed-group" });
		const server = await seedServer(sql, {
			name: "grouped-hushed-server",
			type: "tamanu-central",
			groupId: group.id,
		});
		await seedStatus(sql, {
			serverId: server.id,
			healthy: true,
			health: [{ check: "database", result: "failed" }],
		});
		await seedGroupSilencedRef(sql, {
			groupId: group.id,
			ref: "health/database",
		});

		await page.goto(`/fleet/applications/${server.id}`);

		await expect(page.getByText("Healthy", { exact: true })).toBeVisible();
		await expect(
			page.getByText("Unhealthy", { exact: true }),
		).not.toBeVisible();
		await expect(page.getByText("silenced (group)")).toBeVisible();
	});
});
