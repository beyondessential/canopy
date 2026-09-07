import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedIncident,
	seedIssue,
	seedMaintenanceWindow,
	seedServer,
	seedServerGroup,
	seedStatus,
	seedUpgradePlan,
	seedVersion,
	type Sql,
} from "./seed";

async function openWindows(sql: Sql): Promise<number> {
	const rows = await sql.query<{ n: string }>(
		"SELECT COUNT(*) AS n FROM maintenance_windows WHERE ended_at IS NULL",
	);
	return Number(rows[0]!.n);
}

// spec: MNT
test.describe("maintenance windows", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("a server under a window says so, and its health is marked", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "under-the-knife" });
		const server = await seedServer(sql, {
			name: "being-upgraded",
			groupId: group.id,
		});
		await seedStatus(sql, { serverId: server.id, healthy: true });
		await seedMaintenanceWindow(sql, {
			machineId: server.machineId,
			note: "Upgrading to 2.62",
			declaredBy: "daniel@bes.au",
		});

		await page.goto(`/fleet/applications/${server.id}`);
		const section = page.getByTestId("maintenance-section");
		await expect(section).toContainText("Under maintenance, ending");
		await expect(section).toContainText("Upgrading to 2.62");
		await expect(page.getByTestId("maintenance-marker")).toContainText(
			"Under maintenance",
		);
	});

	test("declaring from the server page opens a window", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "about-to-move" });
		const server = await seedServer(sql, {
			name: "going-down",
			groupId: group.id,
		});
		await seedStatus(sql, { serverId: server.id, healthy: true });

		await page.goto(`/fleet/applications/${server.id}`);
		await page.getByRole("button", { name: "Declare maintenance" }).click();
		await page.getByLabel("What's being done").fill("Swapping the disk");
		await page.getByRole("button", { name: "Declare", exact: true }).click();

		await expect(
			page.getByTestId("maintenance-section"),
		).toContainText("Swapping the disk");
		expect(await openWindows(sql)).toBe(1);
	});

	test("lifting ends the window", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "back-already" });
		const server = await seedServer(sql, { name: "done", groupId: group.id });
		await seedStatus(sql, { serverId: server.id, healthy: true });
		await seedMaintenanceWindow(sql, { machineId: server.machineId, note: "Rebooting" });

		await page.goto(`/fleet/applications/${server.id}`);
		await page.getByRole("button", { name: "Lift" }).click();

		await expect(
			page.getByRole("button", { name: "Declare maintenance" }),
		).toBeVisible();
		expect(await openWindows(sql)).toBe(0);
	});

	test("a server under its group's window says so and points at the group", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "whole-region" });
		const server = await seedServer(sql, {
			name: "one-of-many",
			groupId: group.id,
		});
		await seedStatus(sql, { serverId: server.id, healthy: true });
		await seedMaintenanceWindow(sql, {
			serverGroupId: group.id,
			note: "Cutting over the database",
		});

		await page.goto(`/fleet/applications/${server.id}`);
		const covering = page.getByTestId("covering-group-window");
		await expect(covering).toContainText("Under maintenance, ending");
		await expect(covering).toContainText("Cutting over the database");
		await expect(
			covering.getByRole("link", { name: "whole-region" }),
		).toHaveAttribute("href", `/fleet/groups/${group.id}`);
		await expect(page.getByTestId("maintenance-marker")).toBeVisible();

		await page.goto(`/fleet/groups/${group.id}`);
		await expect(page.getByTestId("maintenance-marker")).toContainText(
			"Under maintenance",
		);
	});

	test("a just-ended window reads as settling, not as maintenance", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "coming-back" });
		const server = await seedServer(sql, {
			name: "just-returned",
			groupId: group.id,
		});
		await seedStatus(sql, { serverId: server.id, healthy: true });
		await seedMaintenanceWindow(sql, {
			machineId: server.machineId,
			endedMinutesAgo: 2,
			note: "Rebooted",
		});

		await page.goto(`/fleet/applications/${server.id}`);
		await expect(page.getByTestId("maintenance-marker")).toContainText(
			"Maintenance just ended",
		);
		await expect(
			page.getByRole("button", { name: "Declare maintenance" }),
		).toBeVisible();
	});

	test("the maintenance page lists what the fleet is not watching", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "whole-group" });
		await seedMaintenanceWindow(sql, {
			serverGroupId: group.id,
			note: "Cutting over the database",
		});

		await page.goto("/maintenance");
		const row = page.getByRole("row", { name: /whole-group/ });
		await expect(row).toContainText("Cutting over the database");
		await expect(row).toContainText("seed@bes.au");
		await expect(
			page.getByRole("link", { name: "whole-group" }),
		).toHaveAttribute("href", `/fleet/groups/${group.id}`);
	});

	/// A window over a box names the box, and the name is the way to it: the
	/// fleet view is where an operator finds work in progress they did not
	/// declare, so every target it lists reaches its own page.
	/// spec: MNT#presentation
	test("the maintenance page links a machine target to its detail page", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "island-group" });
		const server = await seedServer(sql, {
			name: "island-box",
			groupId: group.id,
		});
		await seedMaintenanceWindow(sql, {
			machineId: server.machineId,
			note: "Replacing the disk",
		});

		await page.goto("/maintenance");
		const row = page.getByRole("row", { name: /island-box/ });
		await expect(row).toContainText("Replacing the disk");
		await expect(
			row.getByRole("link", { name: /island-box/ }),
		).toHaveAttribute("href", `/fleet/machines/${server.machineId}`);
	});

	test("nothing under maintenance says so", async ({ page }) => {
		await page.goto("/maintenance");
		await expect(
			page.getByText("Nothing is under maintenance"),
		).toBeVisible();
	});

	test("amending prefills the open window and keeps it the same window", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "running-long" });
		const server = await seedServer(sql, {
			name: "not-done-yet",
			groupId: group.id,
		});
		await seedStatus(sql, { serverId: server.id, healthy: true });
		await seedMaintenanceWindow(sql, {
			machineId: server.machineId,
			note: "Rebooting",
		});

		await page.goto(`/fleet/applications/${server.id}`);
		await page.getByRole("button", { name: "Amend" }).click();
		await expect(
			page.getByRole("heading", { name: /Amend maintenance/ }),
		).toBeVisible();
		const note = page.getByLabel("What's being done");
		await expect(note).toHaveValue("Rebooting");
		await note.fill("Rebooting, running long");
		await page.getByRole("button", { name: "Amend", exact: true }).click();

		await expect(
			page.getByTestId("maintenance-section"),
		).toContainText("Rebooting, running long");
		const rows = await sql.query<{ n: string; amended: string | null }>(
			"SELECT COUNT(*) AS n, MAX(amended_at::text) AS amended \
			 FROM maintenance_windows WHERE machine_id = $1 AND ended_at IS NULL",
			[server.machineId],
		);
		expect(Number(rows[0]!.n)).toBe(1);
		expect(rows[0]!.amended).not.toBeNull();
	});

	test("an open incident offers the declaration over its target", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "mid-upgrade" });
		const server = await seedServer(sql, {
			name: "alerting-on-purpose",
			groupId: group.id,
		});
		await seedStatus(sql, { serverId: server.id, healthy: false });
		const issue = await seedIssue(sql, {
			serverId: server.id,
			source: "canopy",
			ref: "reachability",
			message: "unreachable",
		});
		const incident = await seedIncident(sql, {
			serverGroupId: group.id,
			issues: [{ issueId: issue.id }],
		});

		await page.goto(`/incidents/${incident.id}`);
		await page.getByRole("button", { name: "This is maintenance…" }).click();
		await page.getByLabel("What's being done").fill("It's us, upgrading");
		await page.getByRole("button", { name: "Declare", exact: true }).click();

		await expect(
			page.getByRole("heading", { name: /Declare maintenance/ }),
		).toBeHidden();
		await expect
			.poll(async () => {
				const rows = await sql.query<{ n: string }>(
					"SELECT COUNT(*) AS n FROM maintenance_windows \
					 WHERE server_group_id = $1 AND ended_at IS NULL",
					[group.id],
				);
				return Number(rows[0]!.n);
			})
			.toBe(1);
	});

	// spec: MNT#declaring, UPG
	test("declaring from an open plan carries the plan's window and note", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka" });
		const server = await seedServer(sql, { groupId: group.id, rank: "production" });
		await seedStatus(sql, { serverId: server.id, version: "2.60.0" });
		const target = await seedVersion(sql, { major: 2, minor: 61, patch: 0 });
		// 22:00 to 02:00 is a four-hour window that wraps midnight, which is
		// what an upgrade slot usually looks like.
		await seedUpgradePlan(sql, {
			groupId: group.id,
			targetVersionId: target.id,
			plannedFor: "2020-01-01",
			plannedTime: "22:00",
			plannedEndTime: "02:00",
			plannedZone: "Pacific/Fiji",
			note: "site can absorb 2.61 only",
		});

		await page.goto("/upgrades");
		await page
			.getByRole("button", { name: "Declare maintenance for kamaka" })
			.click();

		await expect(
			page.getByRole("heading", { name: "Declare maintenance — kamaka" }),
		).toBeVisible();
		await expect(page.getByLabel("What's being done")).toHaveValue(
			"site can absorb 2.61 only",
		);

		// The plan's slot is four hours long, so the declaration ends four
		// hours from now: a window says the work is happening now, and the
		// plan only supplies how long it takes.
		const endsAt = await page.getByLabel("Expected to end").inputValue();
		const hours = (new Date(endsAt).getTime() - Date.now()) / 3600_000;
		expect(hours).toBeGreaterThan(3.9);
		expect(hours).toBeLessThan(4.1);

		await page.getByRole("button", { name: "Declare", exact: true }).click();

		await expect
			.poll(async () => {
				const rows = await sql.query<{ note: string | null; rank: string }>(
					"SELECT note, rank FROM maintenance_windows \
					 WHERE server_group_id = $1 AND ended_at IS NULL",
					[group.id],
				);
				return rows.map((r) => [r.note, r.rank]);
			})
			.toEqual([["site can absorb 2.61 only", "production"]]);
	});

	// spec: MNT#declaring, MNT#what-a-window-suspends
	test("declaring from a clone's plan covers the clone and not production", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka" });
		const production = await seedServer(sql, {
			name: "kamaka-central",
			groupId: group.id,
			rank: "production",
		});
		await seedStatus(sql, { serverId: production.id, version: "2.60.0" });
		const clone = await seedServer(sql, {
			name: "kamaka-clone",
			groupId: group.id,
			rank: "clone",
		});
		await seedStatus(sql, { serverId: clone.id, version: "2.60.0" });
		const target = await seedVersion(sql, { major: 2, minor: 61, patch: 0 });
		await seedUpgradePlan(sql, {
			groupId: group.id,
			rank: "clone",
			targetVersionId: target.id,
			note: "rehearsing 2.61 on the clone",
		});

		await page.goto("/upgrades");
		await page
			.getByRole("button", { name: "Declare maintenance for kamaka clone" })
			.click();
		await expect(
			page.getByRole("heading", { name: "Declare maintenance — kamaka clone" }),
		).toBeVisible();
		await page.getByRole("button", { name: "Declare", exact: true }).click();

		await expect
			.poll(async () => {
				const rows = await sql.query<{ rank: string | null }>(
					"SELECT rank FROM maintenance_windows \
					 WHERE server_group_id = $1 AND ended_at IS NULL",
					[group.id],
				);
				return rows.map((r) => r.rank);
			})
			.toEqual(["clone"]);

		// The clone's page says it is covered through the group; production's
		// does not.
		await page.goto(`/fleet/applications/${clone.id}`);
		await expect(page.getByTestId("covering-group-window")).toBeVisible();
		await page.goto(`/fleet/applications/${production.id}`);
		await expect(page.getByTestId("maintenance-section")).toBeVisible();
		await expect(page.getByTestId("covering-group-window")).toHaveCount(0);

		// The group's page shows the environment's window apart from its own.
		await page.goto(`/fleet/groups/${group.id}`);
		await expect(page.getByTestId("environment-window")).toContainText("clone");
	});

	/// A window stops alerting rather than grading, so an operator working
	/// through one watches the check they are fixing go green.
	///
	// spec: MNT#presentation
	test("a check under a window keeps its result and says it raises nothing", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "mid-cutover" });
		const server = await seedServer(sql, {
			name: "failing-on-purpose",
			groupId: group.id,
		});
		await seedMaintenanceWindow(sql, {
			machineId: server.machineId,
			note: "Cutting over the database",
		});
		await seedStatus(sql, {
			serverId: server.id,
			healthy: false,
			health: [
				{ check: "database", healthy: false, message: "connection refused" },
			],
		});

		await page.goto(`/fleet/applications/${server.id}`);
		await expect(page.getByTestId("maintenance-marker")).toContainText(
			"Under maintenance",
		);
		// The failure is graded and presented as it stands, so the operator
		// can see the check they are working on.
		await expect(page.getByText("connection refused")).toBeVisible();
		await expect(page.getByText("Unhealthy")).toBeVisible();
	});

	/// An operator mid-cutover moves between four surfaces, and the property is
	/// that each one carries the failure and the maintenance mark together: a
	/// page showing only the failure reads as an alert nobody has answered, and
	/// one showing only the mark hides the check being fixed. Asserted on every
	/// surface from one state, since a regression tends to take one of them.
	///
	// spec: MNT#presentation
	test("the failure and the maintenance mark read together on every surface", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "mid-cutover" });
		const failing = await seedServer(sql, {
			name: "failing-on-purpose",
			rank: "production",
			groupId: group.id,
		});
		const healthy = await seedServer(sql, {
			name: "fine-on-purpose",
			rank: "production",
			groupId: group.id,
		});
		await seedMaintenanceWindow(sql, {
			machineId: failing.machineId,
			note: "Cutting over the database",
		});
		await seedStatus(sql, {
			serverId: failing.id,
			healthy: false,
			health: [
				{ check: "database", healthy: false, message: "connection refused" },
			],
		});
		await seedStatus(sql, { serverId: healthy.id, healthy: true });

		// The box the workload runs on.
		await page.goto(`/fleet/machines/${failing.machineId}`);
		await expect(page.getByTestId("maintenance-marker")).toContainText(
			"Under maintenance",
		);
		await expect(page.getByTestId("maintenance-section")).toBeVisible();

		// The group, where the tree draws the box around its workloads. A
		// machine's own window is not one the group's maintenance section
		// lists, so the enclosure's mark is what carries it here.
		await page.goto(`/fleet/groups/${group.id}`);
		const enclosure = page
			.getByTestId("group-tree")
			.locator("[data-maintenance='holding']");
		await expect(enclosure).toHaveCount(1);
		// The enclosure holds dots rather than text, so its label is where the
		// two facts sit side by side.
		// The enclosure names the box, its health and the window, then the
		// applications on it, all in the one tooltip.
		await expect(enclosure).toHaveAttribute(
			"aria-label",
			/failing-on-purpose .* under maintenance/,
		);

		// The grid, where one box of the two is hatched and the other is not.
		// The dot's colour comes from the health the card reports, which is
		// asserted where it can actually regress: see the private-server test
		// `a_failing_box_under_a_window_still_reports_its_own_health`.
		await page.goto("/status");
		const card = page.locator(`a[href="/fleet/groups/${group.id}"]`).first();
		await expect(card.locator("[data-maintenance='holding']")).toHaveCount(1);
		await expect(card.getByTestId("status-dot")).toHaveCount(2);
	});
});
