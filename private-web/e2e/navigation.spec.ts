import { expect, test } from "./test-fixtures";
import { resetSeededTables, seedIncident, seedServerGroup } from "./seed";
import { sidebar, sidebarLink } from "./nav";
import { lower, modeControl, raiseTo } from "./safety";

// The mode control is part of the sidebar, so these start where an operator
// does.
test.use({ safetyMode: "read-only" });

test.describe("sidebar navigation", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("lists the sections, then the public links", async ({ page }) => {
		await page.goto("/status");

		const sections = sidebar(page).getByRole("link");
		await expect(sections.first()).toHaveText("Status");
		for (const name of [
			"Incidents",
			"Fleet",
			"Versions",
			"Upgrades",
			"Maintenance",
			"Devices",
			"Bestool",
			"Settings",
		]) {
			await expect(sidebarLink(page, name)).toBeVisible();
		}

		const publicLink = sidebar(page).getByRole("link", { name: /^Public/ });
		await expect(publicLink).toHaveAttribute("target", "_blank");
		await expect(sections.last()).toHaveAttribute("target", "_blank");
	});

	test("counts open incidents beside Incidents", async ({ page, sql }) => {
		await seedIncident(sql);
		await page.goto("/status");
		await expect(
			sidebar(page).getByRole("link", { name: /^Incidents/ }),
		).toHaveText("Incidents1");
	});

	test("expands the current section and toggles others in place", async ({
		page,
	}) => {
		await page.goto("/settings/admins");

		const settingsPages = sidebar(page).getByRole("group", {
			name: "Settings pages",
		});
		await expect(settingsPages).toBeVisible();
		await expect(sidebarLink(page, "Admins")).toHaveAttribute(
			"aria-current",
			"page",
		);
		const fleetPages = sidebar(page).getByRole("group", { name: "Fleet pages" });
		await expect(fleetPages).toHaveCount(0);

		await page.getByRole("button", { name: "Expand Fleet" }).click();
		await expect(fleetPages).toBeVisible();
		await expect(page).toHaveURL(/\/settings\/admins$/);

		await page.getByRole("button", { name: "Collapse Fleet" }).click();
		await expect(fleetPages).toHaveCount(0);
	});

	test("following Fleet lands on Groups, which stays current on a group", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "nav-group" });
		await page.goto("/status");

		await sidebarLink(page, "Fleet").click();
		await expect(page).toHaveURL(/\/fleet$/);
		await expect(sidebarLink(page, "Groups")).toHaveAttribute(
			"aria-current",
			"page",
		);

		await page.getByRole("link", { name: group.name }).click();
		await expect(page).toHaveURL(new RegExp(`/fleet/groups/${group.id}$`));
		await expect(sidebarLink(page, "Groups")).toHaveAttribute(
			"aria-current",
			"page",
		);
	});

	test("the mode band opens in place and closes on a click elsewhere", async ({
		page,
	}) => {
		await page.goto("/status");

		await modeControl(page).click();
		await expect(page.getByRole("menuitem")).toHaveCount(3);
		await page.getByRole("main").click({ position: { x: 5, y: 5 } });
		await expect(page.getByRole("menuitem")).toHaveCount(0);

		await raiseTo(page, "write");
		await lower(page);
	});

	test("the mode band hands focus back to itself when its list closes", async ({
		page,
	}) => {
		await page.goto("/status");

		await modeControl(page).click();
		await expect(page.getByRole("menuitem").first()).toBeFocused();
		await page.keyboard.press("Escape");
		await expect(page.getByRole("menuitem")).toHaveCount(0);
		await expect(modeControl(page)).toBeFocused();

		// Dismissing the danger confirmation returns to the band too.
		await modeControl(page).click();
		await page.getByRole("menuitem", { name: /danger/i }).click();
		await page.getByRole("button", { name: /cancel/i }).click();
		await expect(modeControl(page)).toBeFocused();
		await expect(modeControl(page)).toContainText(/read-only/i);
	});
});

test.describe("sidebar navigation on a narrow screen", () => {
	test.use({ viewport: { width: 600, height: 800 } });

	test("hides behind a menu button and closes on following a link", async ({
		page,
		sql,
	}) => {
		await resetSeededTables(sql);
		await seedIncident(sql);
		await page.goto("/status");

		await expect(sidebar(page)).toHaveCount(0);
		await expect(modeControl(page)).toContainText(/read-only/i);
		await expect(page.getByRole("link", { name: /^Incidents/ })).toHaveText(
			"Incidents1",
		);

		await page.getByRole("button", { name: "Open navigation" }).click();
		await sidebarLink(page, "Maintenance").click();

		await expect(page).toHaveURL(/\/maintenance$/);
		await expect(sidebar(page)).toHaveCount(0);
	});
});
