import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedIncident,
	seedIssue,
	seedServer,
	seedServerGroup,
	seedVersion,
} from "./seed";

// A lingering incident — open, but its last effective failure has
// recovered and it is waiting out the group's linger window — reads
// info-toned ("recovering") rather than error/warning, on every surface
// that differentiates held incidents.
test.describe("lingering incidents", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	async function seedLingering(sql: Parameters<typeof seedIssue>[0]) {
		const group = await seedServerGroup(sql, { name: "linger-group" });
		const server = await seedServer(sql, {
			name: "linger-server",
			type: "tamanu-central",
			rank: "production",
			groupId: group.id,
		});
		// The failure that opened the incident, since recovered.
		const issue = await seedIssue(sql, {
			serverId: server.id,
			ref: "health/flappy",
			active: false,
			message: "was failing, now recovered",
		});
		const opened = new Date(Date.now() - 10 * 60_000).toISOString();
		const cleared = new Date(Date.now() - 2 * 60_000).toISOString();
		const incident = await seedIncident(sql, {
			serverGroupId: group.id,
			rank: "production",
			openedAt: opened,
			closingAt: cleared,
			issues: [{ issueId: issue.id, joinedAt: opened, leftAt: cleared }],
		});
		return { group, server, incident };
	}

	test("incident page shows the recovering chip", async ({ page, sql }) => {
		const { incident } = await seedLingering(sql);

		await page.goto(`/incidents/${incident.id}`);
		await expect(
			page.getByText(/recovering; last failure cleared/i),
		).toBeVisible();
		await expect(
			page.getByText(/a failure returning within the linger window/i),
		).toBeVisible();
	});

	test("server page incident button reads recovering", async ({
		page,
		sql,
	}) => {
		const { server, incident } = await seedLingering(sql);

		await page.goto(`/fleet/applications/${server.id}`);
		const button = page.getByRole("link", {
			name: new RegExp(`incident ${incident.id.slice(0, 8)}.*recovering`, "i"),
		});
		await expect(button).toBeVisible();
	});

	test("status page group card shows the recovering chip", async ({
		page,
		sql,
	}) => {
		// group_details computes version-distance against the latest
		// published version; without one the card 404s.
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const { group } = await seedLingering(sql);

		await page.goto("/status");
		await expect(page.getByRole("heading", { name: group.name })).toBeVisible();
		// The card's status band names the state in one word; that there is an
		// incident at all is said by the segment being coloured.
		await expect(
			page.getByTestId("incident-segment").filter({ hasText: "recovering" }),
		).toBeVisible();
	});
});
