import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedCheckPolicy,
	seedCheckStability,
	seedIssue,
	seedServer,
	seedServerGroup,
	seedStatus,
} from "./seed";

test.describe("check detail page", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("renders an empty state when no server flags the check", async ({
		page,
	}) => {
		await page.goto("/healthchecks/alertd/application.tamanu-central/postgres");
		await expect(
			page.getByText("Nothing currently flags"),
		).toBeVisible();
		// An application's check reads qualified by the type that reports it:
		// another type's postgres is a different check with its own page.
		await expect(
			page.getByRole("heading", { name: "tamanu-central:postgres", exact: true }),
		).toBeVisible();
	});

	test("lists servers flagging the check under their group heading, with server and group links", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "Coral Coast" });
		const warning = await seedServer(sql, {
			name: "Sigatoka Facility",
			groupId: group.id,
		});
		const failing = await seedServer(sql, {
			name: "Korolevu Facility",
			groupId: group.id,
		});
		const healthy = await seedServer(sql, {
			name: "Pacific Central",
			groupId: group.id,
		});
		const otherCheck = await seedServer(sql, {
			name: "Navua Facility",
			groupId: group.id,
		});
		await seedStatus(sql, {
			serverId: warning.id,
			health: [
				{ check: "postgres", result: "warning", latency_ms: 350 },
			],
		});
		await seedStatus(sql, {
			serverId: failing.id,
			health: [
				{
					check: "postgres",
					result: "failed",
					error: "connection refused",
				},
			],
		});
		await seedStatus(sql, {
			serverId: healthy.id,
			health: [{ check: "postgres", result: "passed", latency_ms: 9 }],
		});
		await seedStatus(sql, {
			serverId: otherCheck.id,
			health: [{ check: "disk_space", result: "failed", free_pct: 3 }],
		});

		await page.goto("/healthchecks/alertd/application.tamanu-central/postgres");

		const failingLink = page.getByRole("link", { name: "Korolevu Facility" });
		const warningLink = page.getByRole("link", { name: "Sigatoka Facility" });
		await expect(failingLink).toBeVisible();
		await expect(warningLink).toBeVisible();
		await expect(page.getByText("Pacific Central")).toHaveCount(0);
		await expect(page.getByText("Navua Facility")).toHaveCount(0);

		// The group renders once, as the section heading linking to the
		// group page; server rows link to their server pages.
		await expect(failingLink).toHaveAttribute(
			"href",
			`/fleet/applications/${failing.id}`,
		);
		await expect(page.locator(`a[href="/fleet/groups/${group.id}"]`)).toHaveCount(
			1,
		);
		await expect(
			page.locator(`a[href="/fleet/groups/${group.id}"]`),
		).toHaveText("Coral Coast");

		// Standard list ordering within the group: kind then name
		// (Korolevu before Sigatoka).
		const failingY = (await failingLink.boundingBox())!.y;
		const warningY = (await warningLink.boundingBox())!.y;
		expect(failingY).toBeLessThan(warningY);

		await failingLink.click();
		await expect(page).toHaveURL(new RegExp(`/fleet/applications/${failing.id}$`));
	});

	test("the healthy-servers toggle reveals servers reporting the check passed", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "Highlands" });
		const failing = await seedServer(sql, {
			name: "Mendi Facility",
			groupId: group.id,
		});
		const healthy = await seedServer(sql, {
			name: "Goroka Facility",
			groupId: group.id,
		});
		await seedStatus(sql, {
			serverId: failing.id,
			health: [
				{ check: "postgres", result: "failed", error: "timeout" },
			],
		});
		await seedStatus(sql, {
			serverId: healthy.id,
			health: [{ check: "postgres", result: "passed", latency_ms: 12 }],
		});

		await page.goto("/healthchecks/alertd/application.tamanu-central/postgres");
		await expect(
			page.getByRole("link", { name: "Mendi Facility" }),
		).toBeVisible();
		await expect(page.getByText("Goroka Facility")).toHaveCount(0);

		await page.getByLabel(/show healthy servers/i).click();

		await expect(
			page.getByRole("link", { name: "Goroka Facility" }),
		).toBeVisible();
		await expect(page.getByText("passed", { exact: true })).toBeVisible();
		// Both are listed in the standard name order (Goroka before
		// Mendi) — position no longer encodes urgency, the result chip
		// does.
		const failingLink = page.getByRole("link", { name: "Mendi Facility" });
		const healthyLink = page.getByRole("link", { name: "Goroka Facility" });
		const failingY = (await failingLink.boundingBox())!.y;
		const healthyY = (await healthyLink.boundingBox())!.y;
		expect(healthyY).toBeLessThan(failingY);
	});

	test("a row expands to the check's full data, like the server detail table", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "Vanua Levu" });
		const rich = await seedServer(sql, {
			name: "Labasa Facility",
			groupId: group.id,
		});
		const bare = await seedServer(sql, {
			name: "Savusavu Facility",
			groupId: group.id,
		});
		await seedStatus(sql, {
			serverId: rich.id,
			health: [
				{
					check: "disk_space",
					result: "failed",
					free_pct: 4,
					used_pct: 96,
					message: "volume /data almost full",
				},
			],
		});
		await seedStatus(sql, {
			serverId: bare.id,
			// A bare entry: no extra fields beyond the reserved keys.
			health: [{ check: "disk_space", result: "warning" }],
		});

		await page.goto("/healthchecks/alertd/application.tamanu-central/disk_space");
		await expect(
			page.getByRole("link", { name: "Labasa Facility" }),
		).toBeVisible();
		// Collapsed: the entry's extra fields are hidden.
		await expect(page.getByText("volume /data almost full")).toHaveCount(0);

		// Expand the rich row (rows sort failed-first, so it's first).
		await page.getByRole("button", { name: /^expand$/i }).first().click();

		await expect(page.getByText("free_pct")).toBeVisible();
		await expect(page.getByText("4", { exact: true })).toBeVisible();
		await expect(page.getByText("used_pct")).toBeVisible();
		await expect(page.getByText("volume /data almost full")).toBeVisible();

		// Expand the bare row: it must say so, not render blank.
		await page.getByRole("button", { name: /^expand$/i }).click();
		await expect(
			page.getByText("No additional data reported for this check."),
		).toBeVisible();
	});

	test("group and canopy states file under their own sections", async ({
		page,
		sql,
	}) => {
		// The group's rank bucket comes from its highest-ranked member.
		const group = await seedServerGroup(sql, { name: "Backup Coast" });
		await seedServer(sql, {
			name: "Anchor Central",
			type: "tamanu-central",
			rank: "production",
			groupId: group.id,
		});
		// A group-scoped canopy check (control-plane condition)...
		await seedIssue(sql, {
			serverGroupId: group.id,
			source: "canopy",
			ref: "backup-maintenance",
			message: "maintenance failing",
		});
		// ...and canopy's own state for the same check.
		await seedIssue(sql, {
			source: "canopy",
			ref: "backup-maintenance",
			message: "self-monitoring",
		});

		await page.goto("/healthchecks/canopy/-/backup-maintenance");

		// The group's state sits under the production rank heading, in the
		// group's section, labelled as the whole group.
		await expect(page.getByText("production", { exact: true })).toBeVisible();
		await expect(
			page.locator(`a[href="/fleet/groups/${group.id}"]`),
		).toHaveText("Backup Coast");
		await expect(page.getByText("whole group", { exact: true })).toBeVisible();

		// Canopy's own state gets the trailing section.
		await expect(page.getByText("canopy", { exact: true })).toBeVisible();
		await expect(page.getByText("Canopy (self-monitoring)")).toBeVisible();
	});

	test("shows since when the check has been failing, from the active issue", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "Western Division" });
		const failing = await seedServer(sql, {
			name: "Lautoka Facility",
			groupId: group.id,
		});
		const fresh = await seedServer(sql, {
			name: "Ba Facility",
			groupId: group.id,
		});
		await seedStatus(sql, {
			serverId: failing.id,
			health: [
				{ check: "postgres", result: "failed", error: "connection refused" },
			],
		});
		await seedStatus(sql, {
			serverId: fresh.id,
			health: [
				{ check: "postgres", result: "failed", error: "connection refused" },
			],
		});
		// The failing server's degradation streak started three hours ago;
		// the other server's state row predates the check-state stamps
		// (inactive, no streak), which renders the placeholder.
		await sql.query(
			`UPDATE issues SET degraded_since = NOW() - INTERVAL '3 hours'
			 WHERE application_id = $1 AND ref = 'health/postgres'`,
			[failing.id],
		);
		await sql.query(
			`UPDATE issues SET active = false, degraded_since = NULL
			 WHERE application_id = $1 AND ref = 'health/postgres'`,
			[fresh.id],
		);

		await page.goto("/healthchecks/alertd/application.tamanu-central/postgres");

		// The state-backed row shows a relative failing-since; the row
		// without an active streak shows the em-dash placeholder.
		await expect(page.getByText("3h ago")).toBeVisible();
		await expect(page.getByText("—", { exact: true })).toBeVisible();
	});

	test("shows the catalog ceiling when one is configured", async ({
		page,
		sql,
	}) => {
		const server = await seedServer(sql, { name: "Nadi Facility" });
		await seedStatus(sql, {
			serverId: server.id,
			health: [{ check: "disk_space", result: "failed", free_pct: 2 }],
		});
		await seedCheckPolicy(sql, {
			checkName: "disk_space",
			ceiling: "failed",
			escalates: true,
		});

		await page.goto("/healthchecks/alertd/application.tamanu-central/disk_space");
		// The heading chip renders the configured ceiling, plus the
		// escalates marker.
		await expect(
			page.getByRole("heading", { name: "disk_space" }),
		).toBeVisible();
		await expect(page.getByText("escalates", { exact: true })).toBeVisible();
	});

	test("shows operator documentation in an expandable panel", async ({
		page,
		sql,
	}) => {
		const server = await seedServer(sql, { name: "Doc Facility" });
		await seedStatus(sql, {
			serverId: server.id,
			health: [{ check: "disk_space", result: "failed" }],
		});
		await seedCheckPolicy(sql, {
			checkName: "disk_space",
			documentation:
				"## Description\n\nWatches free disk space.\n\n## Solve\n\nClear old backups.",
		});

		await page.goto("/healthchecks/alertd/application.tamanu-central/disk_space");
		await page.getByText("About this check").click();
		await expect(page.getByText("Watches free disk space.")).toBeVisible();
		await expect(page.getByText("Clear old backups.")).toBeVisible();
	});

	test("documentation is written from the healthcheck settings page, seeded with the template", async ({
		page,
		sql,
	}) => {
		await seedCheckPolicy(sql, { checkName: "disk_space" });

		await page.goto("/settings/healthchecks/alertd/application.tamanu-central/disk_space");
		await expect(
			page.getByText("Nobody has documented this check yet", { exact: false }),
		).toBeVisible();
		await page.getByRole("button", { name: "Write documentation" }).click();

		// The editor seeds the conventional template.
		const editor = page.getByRole("textbox", {
			name: "Documentation markdown",
		});
		await expect(editor).toHaveValue(/## Description/);
		await expect(editor).toHaveValue(/## Results/);
		await expect(editor).toHaveValue(/## Solve/);

		await editor.fill("## Description\n\nDisk space watcher.");
		await page.getByRole("button", { name: "Save documentation" }).click();

		// The saved document renders as markdown.
		await expect(
			page.getByRole("heading", { name: "Description" }),
		).toBeVisible();
		await expect(page.getByText("Disk space watcher.")).toBeVisible();

		const rows = await sql.query<{ documentation: string | null }>(
			"SELECT documentation FROM check_policies WHERE source = 'alertd' AND check_name = 'disk_space'",
		);
		expect(rows[0]!.documentation).toContain("Disk space watcher.");
	});

	test("URL-encodes check names with special characters", async ({
		page,
		sql,
	}) => {
		const checkName = "disk space check";
		const server = await seedServer(sql, { name: "Levuka Facility" });
		await seedStatus(sql, {
			serverId: server.id,
			health: [{ check: checkName, result: "failed" }],
		});

		await page.goto(`/healthchecks/alertd/application.tamanu-central/${encodeURIComponent(checkName)}`);
		await expect(
			page.getByRole("heading", { name: checkName }),
		).toBeVisible();
		await expect(
			page.getByRole("link", { name: "Levuka Facility" }),
		).toBeVisible();
	});

	test("server detail links a failing check to its healthcheck page", async ({
		page,
		sql,
	}) => {
		const server = await seedServer(sql, { name: "Suva Facility" });
		await seedStatus(sql, {
			serverId: server.id,
			healthy: false,
			health: [
				{ check: "postgres", result: "failed", error: "connection refused" },
			],
		});

		await page.goto(`/fleet/applications/${server.id}`);
		const checkLink = page.getByRole("link", { name: "postgres" });
		await expect(checkLink).toBeVisible();
		await checkLink.click();
		await expect(page).toHaveURL(
			/\/healthchecks\/alertd\/application\.tamanu-central\/postgres$/,
		);
	});
});

test.describe("stability record", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("shows the flap summary and the duty-cycle heatmap", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "Flap Coast" });
		const server = await seedServer(sql, {
			name: "Wobbly Facility",
			groupId: group.id,
		});
		await seedStatus(sql, {
			serverId: server.id,
			health: [{ check: "postgres", result: "failed" }],
		});
		const minsAgo = (m: number) =>
			new Date(Date.now() - m * 60_000).toISOString();
		await seedCheckStability(sql, {
			serverId: server.id,
			check: "postgres",
			observations: 20,
			degradedObservations: 12,
			transitions: [
				{ at: minsAgo(200), degraded: true },
				{ at: minsAgo(150), degraded: false },
				{ at: minsAgo(90), degraded: true },
				{ at: minsAgo(30), degraded: false },
			],
			dutyBuckets: { 0: [10, 8], 1: [10, 1] },
		});

		await page.goto("/healthchecks/alertd/application.tamanu-central/postgres");
		const row = page.getByRole("row", { name: /wobbly facility/i });
		await expect(row.getByText("4 flips/24h")).toBeVisible();

		await row.getByRole("button", { name: /expand/i }).click();
		// Scoped to the expanded row's panel — the fleet stability section
		// above the table shows the same numbers with its own heatmap.
		const panel = page.locator("table");
		await expect(
			panel.getByText(/observed 20 times \(12 degraded\)/i),
		).toBeVisible();
		await expect(
			panel.getByText(/4 state changes in 24 h, 4 in 7 days/i),
		).toBeVisible();
		await expect(panel.getByTestId("duty-cell")).toHaveCount(168);
		// The two seeded buckets carry their degraded fractions.
		await expect(
			panel.locator('[data-testid="duty-cell"][data-fraction="0.8"]'),
		).toHaveCount(1);
		await expect(
			panel.locator('[data-testid="duty-cell"][data-fraction="0.1"]'),
		).toHaveCount(1);
	});

	test("a state without a record reads as unknown", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "Quiet Coast" });
		const server = await seedServer(sql, {
			name: "Quiet Facility",
			groupId: group.id,
		});
		await seedStatus(sql, {
			serverId: server.id,
			health: [{ check: "postgres", result: "failed" }],
		});

		await page.goto("/healthchecks/alertd/application.tamanu-central/postgres");
		const row = page.getByRole("row", { name: /quiet facility/i });
		await expect(row.getByText("no record")).toBeVisible();
	});
});

test.describe("fleet stability", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("rolls every server's record into one heatmap", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "Rollup Coast" });
		const a = await seedServer(sql, { name: "Server A", groupId: group.id });
		const b = await seedServer(sql, { name: "Server B", groupId: group.id });
		await seedStatus(sql, {
			serverId: a.id,
			health: [{ check: "postgres", result: "failed" }],
		});
		await seedStatus(sql, {
			serverId: b.id,
			health: [{ check: "postgres", result: "passed" }],
		});
		const minsAgo = (m: number) =>
			new Date(Date.now() - m * 60_000).toISOString();
		await seedCheckStability(sql, {
			serverId: a.id,
			check: "postgres",
			observations: 10,
			degradedObservations: 6,
			transitions: [
				{ at: minsAgo(90), degraded: true },
				{ at: minsAgo(30), degraded: false },
			],
			dutyBuckets: { 5: [10, 6] },
		});
		await seedCheckStability(sql, {
			serverId: b.id,
			check: "postgres",
			observations: 10,
			degradedObservations: 0,
			transitions: [],
			dutyBuckets: { 5: [10, 0] },
		});

		await page.goto("/healthchecks/alertd/application.tamanu-central/postgres");
		await expect(
			page.getByRole("heading", { name: "Fleet stability" }),
		).toBeVisible();
		// Both servers contribute: the shared bucket blends to 6/20.
		await expect(
			page.getByText(/across 2 servers with a record/i),
		).toBeVisible();
		await expect(
			page.getByText(/2 state changes in 24 h, 2 in 7 days, on 1 target/i),
		).toBeVisible();
		const fleet = page
			.getByRole("heading", { name: "Fleet stability" })
			.locator("..");
		await expect(
			fleet.locator('[data-testid="duty-cell"][data-fraction="0.3"]'),
		).toHaveCount(1);

		// The group heading carries its own aggregate — here identical to
		// the fleet's, since the fleet is one group — and expands to the
		// group-scoped rollup (a second heatmap).
		await expect(
			page.getByText("2 flips/24h across 2 servers"),
		).toBeVisible();
		await expect(page.getByTestId("duty-cell")).toHaveCount(168);
		await page.getByRole("button", { name: "Expand group" }).click();
		await expect(page.getByTestId("duty-cell")).toHaveCount(336);
		await expect(
			page.getByText(/across 2 servers with a record/i),
		).toHaveCount(2);
	});
});
