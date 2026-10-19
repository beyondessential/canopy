import { resetSeededTables, seedCheckPolicy } from "./seed";
import { expect, test } from "./test-fixtures";
import { sidebarLink } from "./nav";

test.describe("Sources page", () => {
	// Source policy outlives the test that sets it — the stack (and its
	// database) is per worker, not per test.
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("the healthcheck catalog links out to the sources page", async ({
		page,
		sql,
	}) => {
		await seedCheckPolicy(sql, { source: "alertd", checkName: "db_connect" });

		// The catalog page no longer embeds the sources table — it offers a
		// link out to the dedicated page instead.
		await page.goto("/settings/healthchecks");
		await expect(
			page.getByRole("columnheader", { name: "Reachability" }),
		).toHaveCount(0);

		await page.getByRole("link", { name: /manage sources/i }).click();

		await expect(page).toHaveURL(/\/settings\/healthchecks\/sources$/);
		await expect(page.getByRole("heading", { name: "Sources" })).toBeVisible();
		await expect(page.getByRole("row", { name: /alertd/ }).first()).toBeVisible();

		// The sidebar keeps Healthchecks current on the sub-page.
		await expect(sidebarLink(page, "Healthchecks")).toHaveAttribute(
			"aria-current",
			"page",
		);

		// A back link returns to the catalog.
		await page.getByRole("link", { name: /all healthchecks/i }).click();
		await expect(page).toHaveURL(/\/settings\/healthchecks$/);
	});
});
