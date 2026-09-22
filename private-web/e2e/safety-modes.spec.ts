// The safety-mode indicator and the blocking it drives.
//
// spec: SAFE
//
// The stack's server runs as the development identity and so skips the mode
// check; what these cover is the interface half — that the mode is visible, that
// raising works the way the spec says, and that a control above the operator's
// mode is present and inert rather than missing.

import { expect, test } from "./test-fixtures";
import { lower, modeControl, raiseTo } from "./safety";

test.describe("safety modes", () => {
	test("a page comes up read-only, and says so", async ({ page }) => {
		await page.goto("/settings/admins");
		await expect(modeControl(page)).toContainText(/read-only/i);
	});

	test("raising to write takes effect without confirmation", async ({
		page,
	}) => {
		await page.goto("/settings/admins");
		await modeControl(page).click();
		await page.getByRole("menuitem", { name: /write/i }).click();

		// No dialog: only danger asks.
		await expect(
			page.getByRole("button", { name: "Enter danger mode" }),
		).toHaveCount(0);
		await expect(modeControl(page)).toContainText(/write/i);
	});

	test("raising to danger asks first, and can be declined", async ({
		page,
	}) => {
		await page.goto("/settings/admins");
		await modeControl(page).click();
		await page.getByRole("menuitem", { name: /danger/i }).click();

		await expect(page.getByText("Enter danger mode?")).toBeVisible();
		await page.getByRole("button", { name: "Cancel" }).click();
		await expect(modeControl(page)).toContainText(/read-only/i);

		// And again, this time going through with it.
		await raiseTo(page, "danger");
		await expect(modeControl(page)).toContainText(/danger/i);
	});

	test("a raise shows the time it has left, counting down from ten", async ({
		page,
	}) => {
		await page.goto("/settings/admins");
		await raiseTo(page, "write");
		// Opens just under ten minutes and is always visible, not something to
		// go and check.
		await expect(modeControl(page)).toContainText(/\b(9|10):\d{2}\b/);
	});

	test("an operator lowers without waiting for the countdown", async ({
		page,
	}) => {
		await page.goto("/settings/admins");
		await raiseTo(page, "danger");
		await lower(page);
		await expect(modeControl(page)).toContainText(/read-only/i);
		await expect(modeControl(page)).not.toContainText(/\d:\d{2}/);
	});

	test("a control above the mode is present and does not act", async ({
		page,
	}) => {
		await page.goto("/settings/admins");

		// Present, not removed: the surface has the same shape in every mode.
		const add = page.getByRole("button", { name: "Add admin" });
		await expect(add).toBeVisible();

		// It names the mode it wants, on hover.
		await add.hover();
		await expect(page.getByText(/requires danger mode/i).first()).toBeVisible();

		await page.getByLabel("Email").fill("blocked@example.invalid");
		// Clicking a blocked control does nothing at all — not even the form's
		// own validation runs. `force` gets past Playwright's own actionability
		// check so the click is really attempted against the blocked control.
		await add.click({ force: true });
		await expect(page.getByText("Email cannot be empty")).toHaveCount(0);

		// Raised, the same control acts, and the entry appears.
		await raiseTo(page, "danger");
		await add.click();
		await expect(page.getByText("blocked@example.invalid")).toBeVisible();
	});
});
