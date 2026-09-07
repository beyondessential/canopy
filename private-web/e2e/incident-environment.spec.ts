import {
	resetSeededTables,
	seedIncident,
	seedIssue,
	seedServer,
	seedServerGroup,
	seedVersion,
} from "./seed";
import { expect, test } from "./test-fixtures";

// An incident targets one of a group's environments, so every surface that
// names its target has to say which environment it is: a site's test trouble
// must never read as the site itself. A production environment reads as the
// group's name alone, which is how production trouble has always read.
test.describe("an incident names the environment it is on", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("the incidents list and detail page name the environment", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka" });
		const testBox = await seedServer(sql, {
			name: "kamaka-test",
			groupId: group.id,
			rank: "test",
		});
		const issue = await seedIssue(sql, {
			serverId: testBox.id,
			ref: "health/postgres",
			message: "postgres is down",
		});
		const incident = await seedIncident(sql, {
			serverGroupId: group.id,
			rank: "test",
			issues: [{ issueId: issue.id }],
		});

		await page.goto("/incidents");
		await expect(
			page.getByRole("link", { name: /kamaka test/ }),
		).toBeVisible();

		await page.goto(`/incidents/${incident.id}`);
		await expect(
			page.getByRole("heading", { name: /on kamaka test/ }),
		).toBeVisible();
	});

	test("a production environment reads as the group alone", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "drifting" });
		const central = await seedServer(sql, {
			name: "drifting-central",
			groupId: group.id,
			rank: "production",
		});
		const issue = await seedIssue(sql, {
			serverId: central.id,
			ref: "health/postgres",
			message: "postgres is down",
		});
		const incident = await seedIncident(sql, {
			serverGroupId: group.id,
			rank: "production",
			issues: [{ issueId: issue.id }],
		});

		await page.goto(`/incidents/${incident.id}`);
		const heading = page.getByRole("heading", { name: /on drifting/ });
		await expect(heading).toBeVisible();
		await expect(heading).not.toHaveText(/production/);
	});

	// The group's name is the page's heading, so an environment reads by its
	// rank alone here and the group's own incident carries none.
	// spec: INC#notification
	test("a group's page presents each environment's incident", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka" });
		const central = await seedServer(sql, {
			name: "kamaka-central",
			groupId: group.id,
			rank: "production",
		});
		const testBox = await seedServer(sql, {
			name: "kamaka-test",
			groupId: group.id,
			rank: "test",
		});
		const production = await seedIssue(sql, {
			serverId: central.id,
			ref: "health/postgres",
			message: "postgres is down",
		});
		const testing = await seedIssue(sql, {
			serverId: testBox.id,
			ref: "health/postgres",
			message: "the test box is down",
		});
		await seedIncident(sql, {
			serverGroupId: group.id,
			rank: "production",
			issues: [{ issueId: production.id }],
		});
		await seedIncident(sql, {
			serverGroupId: group.id,
			rank: "test",
			issues: [{ issueId: testing.id }],
		});

		await page.goto(`/fleet/groups/${group.id}`);
		const cards = page.getByTestId("active-incident");
		await expect(cards).toHaveCount(2);
		await expect(
			cards.filter({ hasText: "Active incident in production" }),
		).toBeVisible();
		await expect(
			cards.filter({ hasText: "Active incident in test" }),
		).toBeVisible();
	});

	test("the status card marks the environment in trouble", async ({
		page,
		sql,
	}) => {
		// group_details computes version-distance against the latest published
		// version; without one the card 404s.
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const group = await seedServerGroup(sql, { name: "kamaka" });
		await seedServer(sql, {
			name: "kamaka-central",
			type: "tamanu-central",
			groupId: group.id,
			rank: "production",
		});
		const testBox = await seedServer(sql, {
			name: "kamaka-test",
			type: "tamanu-central",
			groupId: group.id,
			rank: "test",
		});
		const testing = await seedIssue(sql, {
			serverId: testBox.id,
			ref: "health/postgres",
			message: "the test box is down",
		});
		await seedIncident(sql, {
			serverGroupId: group.id,
			rank: "test",
			issues: [{ issueId: testing.id }],
		});

		await page.goto("/status");
		await expect(page.getByRole("heading", { name: group.name })).toBeVisible();
		await expect(
			page.locator('[data-testid="rank-row"][data-rank="test"]'),
		).toHaveAttribute("data-incident", "loud");
		await expect(
			page.locator('[data-testid="rank-row"][data-rank="production"]'),
		).not.toHaveAttribute("data-incident");
		// The card still carries the group-wide mark, so it reads as in
		// trouble from across the grid.
		await expect(
			page.getByTestId("incident-segment").filter({ hasText: "incident" }),
		).toBeVisible();
	});

	test("a group's own incident marks no environment row", async ({
		page,
		sql,
	}) => {
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const group = await seedServerGroup(sql, { name: "drifting" });
		await seedServer(sql, {
			name: "drifting-central",
			type: "tamanu-central",
			groupId: group.id,
			rank: "production",
		});
		const backups = await seedIssue(sql, {
			serverGroupId: group.id,
			ref: "backup-staleness",
			message: "the repository is stale",
		});
		await seedIncident(sql, {
			serverGroupId: group.id,
			issues: [{ issueId: backups.id }],
		});

		await page.goto("/status");
		await expect(
			page.getByTestId("incident-segment").filter({ hasText: "incident" }),
		).toBeVisible();
		await expect(
			page.locator('[data-testid="rank-row"][data-rank="production"]'),
		).not.toHaveAttribute("data-incident");
	});

	/// A card is read from across the grid before any row is, so its own mark
	/// has to stand for the worst thing in the group. A production outage beside
	/// a test box that has recovered must not read as recovering.
	///
	/// spec: CHK#presentation
	test("the card's mark takes the most serious of its environments", async ({
		page,
		sql,
	}) => {
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const group = await seedServerGroup(sql, { name: "kamaka" });
		const central = await seedServer(sql, {
			name: "kamaka-central",
			type: "tamanu-central",
			groupId: group.id,
			rank: "production",
		});
		const testBox = await seedServer(sql, {
			name: "kamaka-test",
			type: "tamanu-central",
			groupId: group.id,
			rank: "test",
		});
		const down = await seedIssue(sql, {
			serverId: central.id,
			ref: "health/postgres",
			message: "postgres is down",
		});
		await seedIncident(sql, {
			serverGroupId: group.id,
			rank: "production",
			issues: [{ issueId: down.id }],
		});
		// The test box's incident is lingering: currently green, and the
		// quieter of the two states.
		const recovered = await seedIssue(sql, {
			serverId: testBox.id,
			ref: "health/postgres",
			active: false,
			message: "was failing, now recovered",
		});
		const opened = new Date(Date.now() - 10 * 60_000).toISOString();
		const cleared = new Date(Date.now() - 2 * 60_000).toISOString();
		await seedIncident(sql, {
			serverGroupId: group.id,
			rank: "test",
			openedAt: opened,
			closingAt: cleared,
			issues: [{ issueId: recovered.id, joinedAt: opened, leftAt: cleared }],
		});

		await page.goto("/status");

		await expect(page.getByTestId("incident-segment")).toHaveAttribute(
			"data-loudness",
			"loud",
		);
		await expect(
			page.locator('[data-testid="rank-row"][data-rank="production"]'),
		).toHaveAttribute("data-incident", "loud");
		await expect(
			page.locator('[data-testid="rank-row"][data-rank="test"]'),
		).toHaveAttribute("data-incident", "lingering");
	});

	test("a group's own incident is presented beside its environments'", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka" });
		const backups = await seedIssue(sql, {
			serverGroupId: group.id,
			ref: "backup-staleness",
			message: "the repository is stale",
		});
		await seedIncident(sql, {
			serverGroupId: group.id,
			issues: [{ issueId: backups.id }],
		});

		await page.goto(`/fleet/groups/${group.id}`);
		const card = page.getByTestId("active-incident");
		await expect(card).toHaveCount(1);
		await expect(card).toHaveText(/Active incident/);
		await expect(card).not.toHaveText(/Active incident in/);
	});
});
