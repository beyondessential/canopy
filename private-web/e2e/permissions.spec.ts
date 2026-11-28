// The permissions page. spec: ADM "The permissions page"

import type { Page } from "@playwright/test";
import { sidebar, sidebarLink } from "./nav";
import { resetAccessLists, seedAccessListEntry } from "./seed";
import { expect, test } from "./test-fixtures";

const levelsTable = (page: Page) =>
	page.getByRole("table", { name: "Access levels" });
const levelRow = (page: Page, name: string) =>
	levelsTable(page).getByRole("row", { name, exact: true });

/** Answer the overview as the server would with the policy read as `policy`
 * says, so the page can be shown sources a debug stack has no tailnet for. */
async function withPolicy(
	page: Page,
	granted: Record<"admin" | "pm", Array<{ source: string; covers: unknown }>>,
	readAt: string | null,
) {
	await page.route("**/api/access/list", async (route) => {
		const response = await route.fetch();
		const overview = await response.json();
		for (const holders of overview.levels) {
			holders.granted_to = granted[holders.level as "admin" | "pm"];
		}
		overview.policy.configured = true;
		overview.policy.read_at = readAt;
		await route.fulfill({ response, json: overview });
	});
}

test.describe("permissions page", () => {
	test.beforeEach(async ({ sql }) => {
		await resetAccessLists(sql);
	});

	test("shows admin, PM and all, highest first, each with what it may do", async ({
		page,
	}) => {
		await page.goto("/settings/permissions");
		const headers = levelsTable(page).getByRole("rowheader");
		await expect(headers).toHaveCount(3);
		await expect(headers.nth(0)).toContainText("Admin");
		await expect(headers.nth(0)).toContainText("Everything, including Canopy's settings.");
		await expect(headers.nth(1)).toContainText("PM");
		await expect(headers.nth(1)).toContainText("Edit groups' project details.");
		await expect(headers.nth(2)).toContainText("All");
		await expect(headers.nth(2)).toContainText("Read everything not kept to admins.");
	});

	test("admin and PM name their key and what each policy source resolves to", async ({
		page,
	}) => {
		await withPolicy(
			page,
			{
				admin: [
					{ source: "group:oncall", covers: { kind: "group", members: 6 } },
					{ source: "ada@example.com", covers: { kind: "person" } },
				],
				pm: [
					{ source: "autogroup:member", covers: { kind: "everyone" } },
					{ source: "*", covers: { kind: "everyone" } },
					{ source: "group:undefined", covers: { kind: "no_one" } },
					{ source: "autogroup:shared", covers: { kind: "no_one" } },
				],
			},
			new Date(Date.now() - 3 * 60_000).toISOString(),
		);
		await page.goto("/settings/permissions");

		const admin = levelRow(page, "Admin");
		await expect(admin).toContainText("admin: true");
		await expect(admin).toContainText("group:oncall (6 people)");
		await expect(admin).toContainText("ada@example.com (one person)");

		const pm = levelRow(page, "PM");
		await expect(pm).toContainText("pm: true");
		await expect(pm).toContainText("autogroup:member (everyone on the tailnet)");
		await expect(pm).toContainText("* (everyone on the tailnet)");
		await expect(pm).toContainText("group:undefined (no one)");
		await expect(pm).toContainText("autogroup:shared (no one)");

		await expect(page.getByText("Policy read 3m ago.")).toBeVisible();
	});

	test("all shows no key and no list, only who holds it", async ({ page }) => {
		await page.goto("/settings/permissions");
		const all = levelRow(page, "All");
		await expect(all).toContainText("Everyone on the tailnet who can reach Canopy");
		await expect(all).not.toContainText(": true");
		await expect(all.getByRole("textbox")).toHaveCount(0);
		await expect(all.getByRole("button")).toHaveCount(0);
	});

	test("says when Canopy isn't configured to read the policy", async ({ page }) => {
		await page.goto("/settings/permissions");
		await expect(
			page.getByText("Canopy isn't configured to read the tailnet policy."),
		).toBeVisible();
		await expect(levelRow(page, "PM")).toContainText("Policy not read");
	});

	test("a policy configured but not yet read grants to no one known", async ({
		page,
	}) => {
		await withPolicy(page, { admin: [], pm: [] }, null);
		await page.goto("/settings/permissions");
		await expect(page.getByText("The policy hasn't been read yet.")).toBeVisible();
		await expect(levelRow(page, "PM")).toContainText("Policy not read");
		await expect(levelRow(page, "PM")).not.toContainText("Granted to no one");
	});

	test("explains how to grant a level on the tailnet", async ({ page }) => {
		await page.goto("/settings/permissions");
		const panel = page
			.getByRole("heading", { name: "Granting a level on the tailnet" })
			.locator("..");
		await expect(panel).toContainText("bes.au/cap/canopy");
		await expect(panel).toContainText("tag:server-canopy");
		await expect(panel).toContainText("every level whose key is true");
		await expect(panel).toContainText("sets the default homepage");
	});

	test("lists each level's logins, and adds and removes them", async ({
		page,
		sql,
	}) => {
		await seedAccessListEntry(sql, "admin", "listed-admin@example.invalid");
		await page.goto("/settings/permissions");

		await expect(levelRow(page, "Admin")).toContainText("listed-admin@example.invalid");
		await expect(levelRow(page, "PM")).not.toContainText("listed-admin@example.invalid");

		await page.getByLabel("Add email to PM").fill("New-PM@Example.invalid");
		await page.getByRole("button", { name: "Add PM" }).click();
		await expect(levelRow(page, "PM")).toContainText("new-pm@example.invalid");

		await page
			.getByRole("button", { name: "Remove new-pm@example.invalid from PM" })
			.click();
		await expect(levelRow(page, "PM")).not.toContainText("new-pm@example.invalid");
		await expect(levelRow(page, "Admin")).toContainText("listed-admin@example.invalid");
	});

	test("rejects an empty email with an inline error", async ({ page }) => {
		await page.goto("/settings/permissions");
		// Browser email validation blocks an empty submit; a space-only value
		// reaches our own check.
		await page.getByLabel("Add email to Admin").fill("   ");
		await page.getByRole("button", { name: "Add admin" }).click();
		await expect(page.getByText("Email cannot be empty")).toBeVisible();
	});

	test("/settings/admins leads to the permissions page", async ({ page }) => {
		await page.goto("/settings/admins");
		await expect(page).toHaveURL(/\/settings\/permissions$/);
		await expect(levelsTable(page)).toBeVisible();
	});

	for (const level of ["pm", "all"] as const) {
		test(`a caller at ${level} has no Permissions entry and no page`, async ({
			page,
		}) => {
			await page.route("**/api/commons/current_user_level", (route) =>
				route.fulfill({ json: level }),
			);
			await page.goto("/settings/mcp-tokens");
			await expect(
				sidebar(page).getByRole("group", { name: "Settings pages" }),
			).toBeVisible();
			await expect(sidebarLink(page, "MCP access")).toBeVisible();
			await expect(sidebarLink(page, "Permissions")).toHaveCount(0);

			await page.goto("/settings/permissions");
			await expect(page.getByText("There's no page here.")).toBeVisible();
			await expect(levelsTable(page)).toHaveCount(0);

			// Settings itself lands on the first page they are shown.
			await page.goto("/settings");
			await expect(page).toHaveURL(/\/settings\/mcp-tokens$/);
		});
	}
});
