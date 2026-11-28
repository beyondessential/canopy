// The root and the home toggle. spec: HOME

import type { Page } from "@playwright/test";
import { modeControl } from "./safety";
import { chosenHomepage, resetHomepages, seedHomepage } from "./seed";
import { expect, test } from "./test-fixtures";

const toggle = (page: Page) =>
	page.getByRole("button", {
		name: /my homepage|your default homepage/,
	});

/** Answer the homepage as the server would where the policy gives `home`,
 * which a debug stack with no tailnet cannot be made to read. */
async function withDefault(page: Page, home: string) {
	await page.route("**/api/homepage/get", async (route) => {
		const response = await route.fetch();
		const homepage = await response.json();
		await route.fulfill({ response, json: { ...homepage, default: home } });
	});
}

test.describe("homepage", () => {
	test.beforeEach(async ({ sql }) => {
		await resetHomepages(sql);
	});

	test("with nothing chosen and no default, the root opens the status page", async ({
		page,
	}) => {
		await page.goto("/");
		await expect(page).toHaveURL(/\/status$/);
	});

	test("a grant with home /fleet/pm makes the root open the projects view", async ({
		page,
	}) => {
		await withDefault(page, "/fleet/pm");
		await page.goto("/");
		await expect(page).toHaveURL(/\/fleet\/pm$/);
		await expect(page.getByRole("textbox", { name: "Search projects" })).toBeVisible();
	});

	test("a chosen homepage naming no page opens as a missing page, and the toggle there clears it", async ({
		page,
		sql,
	}) => {
		await seedHomepage(sql, "/no/such/page");
		await page.goto("/");
		await expect(page).toHaveURL(/\/no\/such\/page$/);
		await expect(page.getByText("There's no page here.")).toBeVisible();

		await expect(toggle(page)).toHaveAccessibleName("Clear my homepage");
		await toggle(page).click();
		await expect(toggle(page)).toHaveAccessibleName("Make this my homepage");
		expect(await chosenHomepage(sql)).toBeNull();
	});

	test("the toggle makes a page home, and the root opens it in a new session", async ({
		page,
		context,
		sql,
	}) => {
		await page.goto("/fleet");
		await expect(toggle(page)).toHaveAccessibleName("Make this my homepage");
		await expect(toggle(page)).toHaveAttribute("aria-pressed", "false");
		await toggle(page).click();
		await expect(toggle(page)).toHaveAccessibleName("Clear my homepage");
		await expect(toggle(page)).toHaveAttribute("aria-pressed", "true");
		expect(await chosenHomepage(sql)).toBe("/fleet");

		const fresh = await context.newPage();
		await fresh.goto("/");
		await expect(fresh).toHaveURL(/\/fleet$/);
	});

	test("the toggle on the chosen homepage clears it, and the root opens the default again", async ({
		page,
		sql,
	}) => {
		await seedHomepage(sql, "/incidents");
		await page.goto("/incidents");
		await expect(toggle(page)).toHaveAccessibleName("Clear my homepage");
		await toggle(page).click();
		await expect(toggle(page)).toHaveAccessibleName("Make this my homepage");
		expect(await chosenHomepage(sql)).toBeNull();

		// Client-side, so the cleared choice is what the root reads.
		await page.getByRole("link", { name: "Canopy", exact: true }).first().click();
		await expect(page).toHaveURL(/\/status$/);
	});

	test("on a page that is home only by default, the toggle shows on and does nothing", async ({
		page,
		sql,
	}) => {
		await withDefault(page, "/maintenance");
		await page.goto("/maintenance");
		await expect(toggle(page)).toHaveAccessibleName("This is your default homepage");
		await expect(toggle(page)).toHaveAttribute("aria-pressed", "true");

		let writes = 0;
		await page.route(/\/api\/homepage\/(set|clear)$/, async (route) => {
			writes += 1;
			await route.continue();
		});
		await toggle(page).click();
		await expect(toggle(page)).toHaveAccessibleName("This is your default homepage");
		expect(writes).toBe(0);
		expect(await chosenHomepage(sql)).toBeNull();
	});

	test("a query string on the page isn't kept in the chosen homepage", async ({
		page,
		sql,
	}) => {
		await page.goto("/incidents?state=resolved");
		await toggle(page).click();
		await expect(toggle(page)).toHaveAccessibleName("Clear my homepage");
		expect(await chosenHomepage(sql)).toBe("/incidents");
	});
});

test.describe("homepage, read-only", () => {
	test.use({ safetyMode: "read-only" });

	test.beforeEach(async ({ sql }) => {
		await resetHomepages(sql);
	});

	test("a homepage read that fails is retried, and the toggle works once it answers", async ({
		page,
	}) => {
		let failures = 1;
		await page.route("**/api/homepage/get", async (route) => {
			if (failures > 0) {
				failures--;
				await route.fulfill({ status: 503, body: "" });
				return;
			}
			await route.continue();
		});
		await page.goto("/versions");
		await expect(toggle(page)).toHaveAccessibleName("Make this my homepage");
		await expect(toggle(page)).toBeEnabled();
	});

	test("the toggle works in read-only mode", async ({ page, sql }) => {
		await page.goto("/versions");
		await expect(modeControl(page)).toContainText(/read-only/i);
		await toggle(page).click();
		await expect(toggle(page)).toHaveAccessibleName("Clear my homepage");
		await expect(page.getByRole("dialog")).toHaveCount(0);
		expect(await chosenHomepage(sql)).toBe("/versions");
		await expect(modeControl(page)).toContainText(/read-only/i);
	});
});
