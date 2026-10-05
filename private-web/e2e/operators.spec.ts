import { randomUUID } from "node:crypto";

import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedMachineReport,
	seedServer,
	seedServerGroup,
	seedStatus,
	seedTailscaleUser,
	seedVersion,
} from "./seed";

/** `health[]` payload with two identified operators (alice on two ttys —
 * dedupe must collapse her) and one local session without a Tailscale
 * identity. */
const EXTERNAL_USERS_HEALTH = [
	{
		check: "external_users",
		result: "passed",
		count: 4,
		users: [
			{
				name: "ubuntu",
				line: "pts/0",
				source: "100.64.0.1",
				tailscale: "alice@example.com",
				connected_since: "2026-06-01T03:56:40Z",
			},
			{
				name: "ubuntu",
				line: "pts/1",
				source: "100.64.0.1",
				tailscale: "alice@example.com",
				connected_since: "2026-06-01T02:00:00Z",
			},
			{
				name: "ubuntu",
				line: "pts/2",
				source: "100.64.0.2",
				tailscale: "bob@example.com",
				connected_since: "2026-06-01T04:00:00Z",
			},
			{ name: "root", line: "tty1" },
		],
	},
];

test.describe("operator presence", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("server detail shows the headline and formatted sessions", async ({
		page,
		sql,
	}) => {
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		await seedTailscaleUser(sql, {
			login: "alice@example.com",
			name: "Alice Example",
		});
		const server = await seedServer(sql, { name: "occupied-server" });
		await seedStatus(sql, {
			serverId: server.id,
			health: EXTERNAL_USERS_HEALTH,
		});

		await page.goto(`/fleet/applications/${server.id}`);

		// Deduped headline: alice's two sessions count once.
		await expect(
			page.getByText("2 operators in the machine right now"),
		).toBeVisible();

		// The sessions are the box's check, read on the box.
		// spec: CHK#presentation
		await page.goto(`/fleet/machines/${server.machineId}`);

		// The check row formats sessions instead of dumping `users` JSON:
		// identified sessions by Tailscale login (one row per session)...
		await expect(page.getByText("alice@example.com")).toHaveCount(2);
		await expect(page.getByText("bob@example.com")).toBeVisible();
		await expect(page.getByText("pts/0", { exact: false })).toBeVisible();
		// ...unidentified ones by OS username.
		await expect(page.getByText("root", { exact: true })).toBeVisible();
		// No raw JSON blob for the users array.
		await expect(page.getByText('"connected_since"')).not.toBeVisible();
	});

	test("server detail withholds the headline when the status is stale", async ({
		page,
		sql,
	}) => {
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const server = await seedServer(sql, { name: "stale-server" });
		await seedStatus(sql, {
			serverId: server.id,
			health: EXTERNAL_USERS_HEALTH,
			createdAt: "NOW() - INTERVAL '45 minutes'",
		});

		await page.goto(`/fleet/applications/${server.id}`);

		// A 45-minutes-old push can't claim "right now"…
		await expect(
			page.getByText(/operators? in the machine right now/),
		).not.toBeVisible();

		// …though the box's checks table still shows the sessions as last
		// known.
		await page.goto(`/fleet/machines/${server.machineId}`);
		await expect(page.getByText("bob@example.com")).toBeVisible();
		await expect(
			page.getByText(/operators? in the machine right now/),
		).not.toBeVisible();
	});

	test("status page marks occupied groups with a person-count chip", async ({
		page,
		sql,
	}) => {
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const group = await seedServerGroup(sql, { name: "occupied-cluster" });
		const server = await seedServer(sql, {
			name: "occupied-member",
			rank: "production",
			groupId: group.id,
		});
		await seedStatus(sql, {
			serverId: server.id,
			health: EXTERNAL_USERS_HEALTH,
		});

		await page.goto("/status");

		const card = page
			.locator(`a[href="/fleet/groups/${group.id}"]`)
			.locator(".MuiCard-root");
		await expect(card.getByText("2", { exact: true })).toBeVisible();

		// Tooltip names who's on which box.
		await card.getByText("2", { exact: true }).hover();
		await expect(
			page.getByText("alice@example.com · occupied-member"),
		).toBeVisible();
	});

	test("group page lists the aggregated operators", async ({ page, sql }) => {
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		await seedTailscaleUser(sql, {
			login: "alice@example.com",
			name: "Alice Example",
		});
		const group = await seedServerGroup(sql, { name: "staffed-cluster" });
		const one = await seedServer(sql, {
			name: "member-one",
			rank: "production",
			groupId: group.id,
		});
		const two = await seedServer(sql, {
			name: "member-two",
			rank: "production",
			groupId: group.id,
		});
		// Alice is on both members; the aggregate counts her once and
		// names both servers.
		await seedStatus(sql, { serverId: one.id, health: EXTERNAL_USERS_HEALTH });
		await seedStatus(sql, {
			serverId: two.id,
			health: [
				{
					check: "external_users",
					result: "passed",
					count: 1,
					users: [
						{
							name: "ubuntu",
							line: "pts/0",
							source: "100.64.0.1",
							tailscale: "alice@example.com",
							connected_since: "2026-06-01T05:00:00Z",
						},
					],
				},
			],
		});

		await page.goto(`/fleet/groups/${group.id}`);

		await expect(
			page.getByRole("heading", {
				name: "2 operators in the group right now",
			}),
		).toBeVisible();
		await expect(
			page.getByText("Alice Example (alice@example.com)"),
		).toBeVisible();
		await expect(page.getByText(/on member-one, member-two/)).toBeVisible();
		await expect(page.getByText(/on member-two, member-one/)).not.toBeVisible();
	});

	/// A person is logged in to a box, not to the software on it. Two workloads
	/// on one box is one place someone is, so the count and the tooltip both
	/// speak of the box.
	///
	/// spec: FLT
	test("a person on two workloads of one box counts once, and names the box", async ({
		page,
		sql,
	}) => {
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const group = await seedServerGroup(sql, { name: "one-box-cluster" });
		const central = await seedServer(sql, {
			name: "central-on-box",
			rank: "production",
			groupId: group.id,
		});
		// A second workload sharing the first's box, which is what makes this
		// one place rather than two.
		const facilityId = randomUUID();
		await sql.query(
			`INSERT INTO applications (id, name, host, type, rank, group_id, machine_id)
			 VALUES ($1, 'facility-on-box', 'https://f.e2e.invalid',
			         'tamanu-facility', 'production', $2, $3)`,
			[facilityId, group.id, central.machineId],
		);
		// The same person is reported logged in by both workloads, because
		// both are reading the same box's sessions.
		await seedStatus(sql, {
			serverId: central.id,
			health: EXTERNAL_USERS_HEALTH,
		});
		await seedStatus(sql, {
			serverId: facilityId,
			health: EXTERNAL_USERS_HEALTH,
		});

		await page.goto("/status");

		const card = page
			.locator(`a[href="/fleet/groups/${group.id}"]`)
			.locator(".MuiCard-root");
		// Two people on one box, not four sessions and not two boxes' worth.
		await expect(card.getByText("2", { exact: true })).toBeVisible();

		await card.getByText("2", { exact: true }).hover();
		const tooltip = page.getByRole("tooltip");
		// The box is named once. The workloads on it are not what someone is
		// logged in to, so neither appears.
		await expect(
			tooltip.getByText("alice@example.com · central-on-box", { exact: true }),
		).toBeVisible();
		await expect(tooltip.getByText(/facility-on-box/)).toHaveCount(0);
	});

	/// Across boxes the tooltip names each of them, so "where is this person"
	/// is answered by the tooltip alone.
	///
	/// spec: FLT
	test("the tooltip names every box a person is on", async ({ page, sql }) => {
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const group = await seedServerGroup(sql, { name: "two-box-cluster" });
		const one = await seedServer(sql, {
			name: "box-one",
			rank: "production",
			groupId: group.id,
		});
		const two = await seedServer(sql, {
			name: "box-two",
			rank: "production",
			groupId: group.id,
		});
		await seedStatus(sql, { serverId: one.id, health: EXTERNAL_USERS_HEALTH });
		await seedStatus(sql, {
			serverId: two.id,
			health: [
				{
					check: "external_users",
					result: "passed",
					count: 1,
					users: [
						{
							name: "ubuntu",
							line: "pts/0",
							source: "100.64.0.1",
							tailscale: "alice@example.com",
							connected_since: "2026-06-01T05:00:00Z",
						},
					],
				},
			],
		});

		await page.goto("/status");

		const card = page
			.locator(`a[href="/fleet/groups/${group.id}"]`)
			.locator(".MuiCard-root");
		await expect(card.getByText("2", { exact: true })).toBeVisible();

		await card.getByText("2", { exact: true }).hover();
		const tooltip = page.getByRole("tooltip");
		await expect(
			tooltip.getByText("alice@example.com · box-one, box-two", {
				exact: true,
			}),
		).toBeVisible();
		// Bob is only on the one box, and reads as such.
		await expect(
			tooltip.getByText("bob@example.com · box-one", { exact: true }),
		).toBeVisible();
	});

	/// A session is on the box, so the box's own page is where "who is here"
	/// is answered, whichever workload happened to report it.
	///
	/// spec: FLT
	test("machine detail shows the people on the box", async ({ page, sql }) => {
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		await seedTailscaleUser(sql, {
			login: "alice@example.com",
			name: "Alice Example",
		});
		const server = await seedServer(sql, { name: "occupied-box" });
		// The box is reporting, so "right now" holds.
		await seedMachineReport(sql, { machineId: server.machineId });
		await seedStatus(sql, {
			serverId: server.id,
			health: EXTERNAL_USERS_HEALTH,
		});

		await page.goto(`/fleet/machines/${server.machineId}`);

		await expect(
			page.getByText("2 operators in the machine right now"),
		).toBeVisible();
		await expect(page.getByText("bob@example.com")).toBeVisible();
	});

	test("server detail header links to the group page", async ({
		page,
		sql,
	}) => {
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const group = await seedServerGroup(sql, { name: "linked-cluster" });
		const server = await seedServer(sql, {
			name: "linked-server",
			groupId: group.id,
		});

		await page.goto(`/fleet/applications/${server.id}`);

		// The Group section below also links to the group; the new link
		// lives in the page's h1.
		await page
			.getByRole("heading", { level: 1 })
			.getByRole("link", { name: "linked-cluster" })
			.click();
		await expect(page).toHaveURL(new RegExp(`/fleet/groups/${group.id}$`));
	});
});
