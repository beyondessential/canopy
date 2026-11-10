import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedIncident,
	seedMemoryEntry,
	seedServerGroup,
} from "./seed";

test.describe("incident memory page", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("lists entries with their links resolved, newest first", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "pacific" });
		await seedMemoryEntry(sql, { title: "Fleet quirk" });
		await seedMemoryEntry(sql, { title: "Pacific needs VPN", groupId: group.id });

		await page.goto("/incidents/memory");
		await expect(page.getByRole("heading", { name: "Incident memory" })).toBeVisible();
		await expect(page.getByText("Fleet quirk")).toBeVisible();
		await expect(page.getByText("Pacific needs VPN")).toBeVisible();
		await expect(page.getByRole("link", { name: "Group: pacific" })).toBeVisible();
		await expect(page.getByText("Fleet-wide")).toBeVisible();
	});

	test("search and filters narrow the list and are held in the address", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "pacific" });
		await seedMemoryEntry(sql, { title: "Fleet quirk" });
		await seedMemoryEntry(sql, { title: "Pacific needs VPN", groupId: group.id });
		await seedMemoryEntry(sql, {
			title: "Disk thresholds",
			check: { source: "alertd", name: "disk-space" },
		});

		await page.goto("/incidents/memory");
		await page.getByLabel("Search").fill("vp");
		await expect(page.getByText("Pacific needs VPN")).toBeVisible();
		await expect(page.getByText("Fleet quirk")).toHaveCount(0);
		await expect(page).toHaveURL(/q=vp/);

		// A filtered list opens from its address.
		await page.goto("/incidents/memory?kind=check");
		await expect(page.getByText("Disk thresholds")).toBeVisible();
		await expect(page.getByText("Fleet quirk")).toHaveCount(0);
		await expect(page.getByText("Pacific needs VPN")).toHaveCount(0);
	});

	test("retired entries appear only when asked for", async ({ page, sql }) => {
		await seedMemoryEntry(sql, { title: "Current one" });
		await seedMemoryEntry(sql, { title: "Old one", retiredReason: "fixed" });

		await page.goto("/incidents/memory");
		await expect(page.getByText("Current one")).toBeVisible();
		await expect(page.getByText("Old one")).toHaveCount(0);

		await page.getByLabel("Include retired").click();
		await expect(page.getByLabel("Include retired")).toBeChecked();
		await expect(page.getByText("Old one")).toBeVisible();
		await expect(page).toHaveURL(/retired=1/);
	});

	test("entries relevant to an incident are reached from its page", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "pacific" });
		const incident = await seedIncident(sql, { serverGroupId: group.id });
		await seedMemoryEntry(sql, { title: "Pacific needs VPN", groupId: group.id });
		await seedMemoryEntry(sql, { title: "Unrelated" });

		await page.goto(`/incidents/${incident.id}`);
		await page.getByRole("link", { name: /1 memory entry relevant/ }).click();
		await expect(page).toHaveURL(new RegExp(`relevant=${incident.id}`));
		await expect(page.getByText("Pacific needs VPN")).toBeVisible();
		await expect(page.getByText("Unrelated")).toHaveCount(0);
	});

	test("an administrator records, amends, retires and restores an entry", async ({
		page,
	}) => {
		await page.goto("/incidents/memory");
		await page.getByRole("button", { name: "Record entry" }).click();
		const dialog = page.getByRole("dialog");
		await dialog.getByLabel("Title").fill("Exports fill the disk");
		await dialog.getByLabel("Body").fill("The weekly export is why.");
		await dialog.getByRole("button", { name: "Save" }).click();
		await expect(page.getByText("Exports fill the disk")).toBeVisible();

		await page.getByText("Exports fill the disk").click();
		await page.getByRole("button", { name: "Amend" }).click();
		await page.getByRole("dialog").getByLabel("Body").fill("The nightly export is why.");
		await page.getByRole("dialog").getByRole("button", { name: "Save" }).click();
		await expect(page.getByText("The nightly export is why.")).toBeVisible();

		await page.getByRole("button", { name: "Retire", exact: true }).first().click();
		await page.getByLabel("Why it no longer holds").fill("fixed upstream");
		await page.getByRole("button", { name: "Retire", exact: true }).last().click();
		await expect(page.getByText("Exports fill the disk")).toHaveCount(0);

		await page.getByLabel("Include retired").click();
		await expect(page.getByLabel("Include retired")).toBeChecked();
		await page.getByText("Exports fill the disk").click();
		await expect(page.getByText(/retired by .*fixed upstream/).first()).toBeVisible();
		// Restoring the first version brings it back, current.
		await page.getByRole("button", { name: "Restore" }).last().click();
		await page.getByLabel("Include retired").click();
		await expect(page.getByLabel("Include retired")).not.toBeChecked();
		await expect(page.getByText("Exports fill the disk")).toBeVisible();
	});
});
