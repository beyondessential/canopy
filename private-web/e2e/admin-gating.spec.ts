import { expect, test } from "./test-fixtures";
import { resetSeededTables, seedServerGroup } from "./seed";
import { sidebarLink } from "./nav";

const PROBE = "**/api/commons/is_current_user_admin";

// "New group" on the fleet listing is admin-only and unambiguous, so it makes a
// good stand-in for every admin-gated control in the app: they all read the
// same provider now.
const adminControl = (page: import("@playwright/test").Page) =>
	page.getByRole("link", { name: "New group" });

test.describe("admin gating", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("the admin probe runs once for the whole session, not per page", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "probe-count-group" });

		// Routes used to each run their own `is_current_user_admin` query. That
		// gave every page an independent answer — and an independent chance to
		// fail — which is how one half of a page ended up in admin mode while
		// the other half wasn't. Now the provider owns the probe, so visiting
		// more pages must not issue more of them. (Counting from a baseline
		// rather than asserting exactly 1: React's StrictMode double-mounts in
		// dev, and how many probes the initial mount costs isn't the point.)
		let probes = 0;
		await page.route(PROBE, async (route) => {
			probes += 1;
			await route.continue();
		});

		await page.goto("/fleet");
		await expect(adminControl(page)).toBeVisible();
		const onLoad = probes;

		// Client-side navigation, so the provider stays mounted throughout.
		await sidebarLink(page, "Archived").click();
		await expect(page.getByText("Nothing archived.")).toBeVisible();

		await sidebarLink(page, "Groups").click();
		await page.getByRole("link", { name: new RegExp(group.name) }).click();
		await expect(
			page.getByRole("heading", { name: group.name, level: 1 }),
		).toBeVisible();
		await expect(page.getByRole("link", { name: "Edit" })).toBeVisible();

		expect(probes).toBe(onLoad);
	});

	test("a failing probe explains itself instead of silently hiding controls", async ({
		page,
	}) => {
		await page.route(PROBE, (route) => route.fulfill({ status: 503, body: "" }));

		await page.goto("/fleet");

		await expect(
			page.getByText("Couldn't check your admin status"),
		).toBeVisible();
		await expect(adminControl(page)).toBeHidden();
	});

	test("the session self-heals once the probe recovers, without a reload", async ({
		page,
	}) => {
		let failing = true;
		await page.route(PROBE, async (route) => {
			if (failing) {
				await route.fulfill({ status: 503, body: "" });
			} else {
				await route.continue();
			}
		});

		await page.goto("/fleet");
		const banner = page.getByText("Couldn't check your admin status");
		await expect(banner).toBeVisible();

		// The provider retries on its own; the button just skips the wait.
		failing = false;
		await page.getByRole("button", { name: "Retry" }).click();

		await expect(banner).toBeHidden();
		await expect(adminControl(page)).toBeVisible();
	});
});
