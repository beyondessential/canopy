import { randomUUID } from "node:crypto";

import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedMachine,
	seedServer,
	seedServerGroup,
} from "./seed";

/// Every page in a group ends with the same picture of it: rank, then the
/// boxes at that rank, then the workloads on each box. An operator learns one
/// arrangement and reads it everywhere, and moving sideways never goes back
/// through the group.
///
/// spec: FLT
test.describe("the group's tree on the detail pages", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	/// A group with one shared box carrying two workloads and one box of its
	/// own, which is the arrangement the tree exists to show.
	async function seedTree(sql: Parameters<typeof seedServerGroup>[0]) {
		const group = await seedServerGroup(sql, { name: "tree-group" });
		const shared = await seedMachine(sql, {
			name: "shared-box",
			groupId: group.id,
		});
		const centralId = randomUUID();
		const facilityId = randomUUID();
		await sql.query(
			`INSERT INTO applications (id, name, host, type, rank, group_id, machine_id)
			 VALUES ($1, 'central-on-shared', 'https://c.e2e.invalid',
			         'tamanu-central', 'production', $3, $4),
			        ($2, 'facility-on-shared', 'https://f.e2e.invalid',
			         'tamanu-facility', 'production', $3, $4)`,
			[centralId, facilityId, group.id, shared.id],
		);
		const solo = await seedServer(sql, {
			name: "solo-app",
			groupId: group.id,
		});
		return { group, shared, centralId, facilityId, solo };
	}

	test("the application page ends with the tree, marking itself in place", async ({
		page,
		sql,
	}) => {
		const { shared, centralId, facilityId, solo } = await seedTree(sql);

		await page.goto(`/fleet/applications/${centralId}`);

		const tree = page.getByTestId("group-tree");
		await expect(tree).toBeVisible();

		// The box this workload is on, and the other box in the group, are both
		// reachable from here: the tree is a map of the group.
		await expect(tree.locator(`a[href="/fleet/machines/${shared.id}"]`)).toBeVisible();
		await expect(
			tree.locator(`a[href="/fleet/machines/${solo.machineId}"]`),
		).toBeVisible();

		// The workload sharing its box, and the one on the other box, are named
		// and linked.
		await expect(tree.locator(`a[href="/fleet/applications/${facilityId}"]`)).toBeVisible();
		await expect(tree.locator(`a[href="/fleet/applications/${solo.id}"]`)).toBeVisible();

		// This page is in the tree, but marked in place rather than linking back
		// to where the operator already is.
		await expect(tree.getByText("central-on-shared")).toBeVisible();
		await expect(tree.locator(`a[href="/fleet/applications/${centralId}"]`)).toHaveCount(0);
	});

	test("the machine page ends with the same tree, marking itself in place", async ({
		page,
		sql,
	}) => {
		const { shared, centralId, facilityId, solo } = await seedTree(sql);

		await page.goto(`/fleet/machines/${shared.id}`);

		const tree = page.getByTestId("group-tree");
		await expect(tree).toBeVisible();

		// Both workloads on this box, and the workload on the other one.
		await expect(tree.locator(`a[href="/fleet/applications/${centralId}"]`)).toBeVisible();
		await expect(tree.locator(`a[href="/fleet/applications/${facilityId}"]`)).toBeVisible();
		await expect(tree.locator(`a[href="/fleet/applications/${solo.id}"]`)).toBeVisible();

		// The other box links; this one is named without linking back.
		await expect(
			tree.locator(`a[href="/fleet/machines/${solo.machineId}"]`),
		).toBeVisible();
		await expect(tree.getByText("shared-box")).toBeVisible();
		await expect(tree.locator(`a[href="/fleet/machines/${shared.id}"]`)).toHaveCount(
			0,
		);
	});

	/// A box serving no environment yet is listed apart, after every
	/// environment, so an operator who has just had one report in sees it needs
	/// a rank. Its workloads' dots are drawn as on any other box.
	///
	/// spec: FLT#what-each-carries
	test("a pending box is listed under awaiting a rank, after the environments", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "pending-group" });
		await seedServer(sql, {
			name: "ranked-app",
			groupId: group.id,
			rank: "test",
		});
		const pending = await seedServer(sql, {
			name: "pending-app",
			groupId: group.id,
			rank: null,
		});

		await page.goto(`/fleet/groups/${group.id}`);

		const sections = page.getByTestId("group-tree").getByTestId("tree-environment");
		await expect(sections).toHaveCount(2);
		await expect(sections.nth(0)).toHaveAttribute("data-rank", "test");
		await expect(sections.nth(1)).toHaveAttribute("data-rank", "pending");
		await expect(sections.nth(1)).toContainText("awaiting a rank");
		await expect(
			sections.nth(1).locator(`a[href="/fleet/applications/${pending.id}"]`),
		).toBeVisible();
		await expect(sections.nth(1).getByTestId("status-dot").first()).toBeVisible();
	});

	/// A box with nothing on it has not reported, which is a different wait
	/// from a reported box waiting for a rank.
	///
	/// spec: FLT#what-each-carries
	test("a box with nothing on it reads as awaiting check-in, not awaiting a rank", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "silent-group" });
		await seedMachine(sql, { name: "silent-box", groupId: group.id });

		await page.goto(`/fleet/groups/${group.id}`);

		const section = page.getByTestId("group-tree").getByTestId("tree-environment");
		await expect(section).toHaveCount(1);
		await expect(section).toHaveAttribute("data-rank", "awaiting-check-in");
		await expect(section).toContainText("awaiting check-in");
		await expect(section).not.toContainText("awaiting a rank");
	});

	/// A box ranked before anything on it has reported is in its environment
	/// already, and still says it is waiting to hear from the box.
	///
	/// spec: FLT#navigating-the-two-grains
	test("a ranked box with nothing on it sits under its rank, awaiting check-in", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "ranked-silent-group" });
		await seedMachine(sql, {
			name: "ranked-silent-box",
			groupId: group.id,
			rank: "test",
		});

		await page.goto(`/fleet/groups/${group.id}`);

		const section = page.getByTestId("group-tree").getByTestId("tree-environment");
		await expect(section).toHaveCount(1);
		await expect(section).toHaveAttribute("data-rank", "test");
		await expect(section).toContainText("ranked-silent-box");
		await expect(section).toContainText("Awaiting check-in.");
	});

	/// The title says which thing the page is about. Whether that thing is well
	/// is the tree's and the checks' business, so no dot rides alongside the
	/// name.
	///
	/// spec: FLT
	test("neither detail page shows a status dot beside its title", async ({
		page,
		sql,
	}) => {
		const { shared, centralId } = await seedTree(sql);

		await page.goto(`/fleet/applications/${centralId}`);
		await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
		await expect(
			page
				.getByRole("heading", { level: 1 })
				.locator("xpath=..")
				.getByTestId("status-dot"),
		).toHaveCount(0);

		await page.goto(`/fleet/machines/${shared.id}`);
		await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
		await expect(
			page
				.getByRole("heading", { level: 1 })
				.locator("xpath=..")
				.getByTestId("status-dot"),
		).toHaveCount(0);
	});
});
