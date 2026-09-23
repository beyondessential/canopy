// The safety-mode indicator and the blocking it drives.
//
// spec: SAFE
//
// The stack's server runs as the development identity and so skips the mode
// check; what these cover is the interface half — that the mode is visible, that
// raising works the way the spec says, and that a control above the operator's
// mode is present and inert rather than missing.

import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedMachine,
	seedServer,
	seedServerGroup,
	seedStatus,
	seedVersion,
} from "./seed";
import { lower, modeControl, raiseTo } from "./safety";

test.describe("safety modes", () => {
	test.use({ safetyMode: "read-only" });

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
		await expect(modeControl(page)).toContainText(/(9|10):\d{2}/);
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

	test("a reloaded page comes back read-only", async ({ page }) => {
		await page.goto("/settings/admins");
		await raiseTo(page, "danger");

		// The session identifier is held in memory and never persisted, so a
		// reload asks for a fresh session rather than resuming the raise.
		await page.reload();
		await expect(modeControl(page)).toContainText(/read-only/i);
	});

	test("a control above the mode is present and does not act", async ({
		page,
	}) => {
		await page.goto("/settings/admins");

		// Present, not removed: the surface has the same shape in every mode.
		const add = page.getByRole("button", { name: "Add admin" });
		await expect(add).toBeVisible();

		// The wrapper names the mode the control wants, and is what the pointer
		// reaches — the control inside takes no pointer events at all.
		const blocked = page.getByLabel(/requires danger mode/i).first();
		await expect(blocked).toBeVisible();

		await page.getByLabel("Email").fill("blocked@example.invalid");
		// Clicking a blocked control does nothing at all — not even the form's
		// own validation runs.
		await blocked.click();
		await expect(page.getByText("Email cannot be empty")).toHaveCount(0);
		await expect(page.getByText("blocked@example.invalid")).toHaveCount(0);

		// Raised, the same control acts, and the entry appears.
		await raiseTo(page, "danger");
		await add.click();
		await expect(page.getByText("blocked@example.invalid")).toBeVisible();
	});
	test("a raise made before the page has its session is not undone when the session arrives", async ({
		page,
	}) => {
		// Hold the page's first request for a session until after the raise.
		let release: () => void = () => {};
		const held = new Promise<void>((resolve) => {
			release = resolve;
		});
		await page.route("**/api/safety/session", async (route) => {
			await held;
			await route.continue();
		});

		await page.goto("/settings/admins");
		await modeControl(page).click();
		await page.getByRole("menuitem", { name: /write/i }).click();
		await expect(modeControl(page)).toContainText(/write/i);

		// Let the late answer land, then check it changed nothing.
		const answered = page.waitForResponse("**/api/safety/session");
		release();
		await answered;
		await expect(modeControl(page)).toContainText(/write/i);
	});

	test("machine setup below danger offers its ticket blocked, and mints once raised", async ({
		page,
		sql,
	}) => {
		await resetSeededTables(sql);
		const group = await seedServerGroup(sql, { name: "enrol-below-danger" });
		const machine = await seedMachine(sql, {
			name: "waiting-box",
			groupId: group.id,
		});
		const tickets = async () =>
			Number(
				(
					await sql.query<{ count: string }>(
						"SELECT COUNT(*) AS count FROM machine_enrollment_tokens WHERE machine_id = $1",
						[machine.id],
					)
				)[0].count,
			);

		await page.goto(`/fleet/machines/${machine.id}`);

		// Minting is danger: read-only, the page offers it rather than asking.
		const issue = page.getByRole("button", { name: "Issue enrollment ticket" });
		await expect(issue).toBeDisabled();
		await expect(page.getByLabel(/requires danger mode/i).first()).toBeVisible();
		await expect(page.getByText(/bestool canopy register/)).toHaveCount(0);
		expect(await tickets()).toBe(0);

		// Raised, it mints on its own as it always has.
		await raiseTo(page, "danger");
		await expect(page.getByText(/bestool canopy register/)).toBeVisible();
		expect(await tickets()).toBeGreaterThan(0);
	});

	test("a blocked control carries its grade's stripe, full colour under the pointer", async ({
		page,
	}) => {
		// Danger: adding an allow-list entry.
		await page.goto("/settings/admins");
		const dangerous = page.getByRole("button", { name: "Add admin" });
		await expect(dangerous).toHaveCSS("background-image", /rgba\(239, 83, 80/);
		await expect(dangerous).toHaveCSS("filter", "grayscale(0.8)");
		await page.getByLabel(/requires danger mode/i).first().hover();
		await expect(dangerous).toHaveCSS("filter", "grayscale(0)");

		// Write: saving a snippet, in the write grade's colour.
		await page.goto("/bestool/snippets");
		const writing = page.getByRole("button", { name: "Add" });
		await expect(writing).toHaveCSS("background-image", /rgba\(255, 152, 0/);
		await expect(writing).toHaveCSS("filter", "grayscale(0.8)");
	});

	test("the mode control and its menu wear each mode's stripe", async ({
		page,
	}) => {
		await page.goto("/settings/admins");

		// Read-only is plain; each raised mode in the menu carries its stripe,
		// muted until the pointer is on it.
		await expect(modeControl(page)).toHaveCSS("background-image", "none");
		await modeControl(page).click();
		const write = page.getByRole("menuitem", { name: /write/i });
		const danger = page.getByRole("menuitem", { name: /danger/i });
		await expect(write).toHaveCSS("background-image", /rgba\(255, 152, 0/);
		await expect(danger).toHaveCSS("background-image", /rgba\(239, 83, 80/);
		await expect(danger).toHaveCSS("filter", "grayscale(0.8)");
		await danger.hover();
		await expect(danger).toHaveCSS("filter", "grayscale(0)");
		await page.keyboard.press("Escape");

		// Raised, the control itself wears the mode's stripe, and the menu shows
		// the current mode in full colour.
		await raiseTo(page, "danger");
		await expect(modeControl(page)).toHaveCSS(
			"background-image",
			/rgba\(239, 83, 80, 0\.5/,
		);
		await modeControl(page).click();
		const current = page.getByRole("menuitem", { name: /danger/i });
		await expect(current).toHaveAttribute("aria-current", "true");
		await page.mouse.move(0, 0);
		await expect(current).toHaveCSS("filter", "grayscale(0)");
	});

	test("a blocked control is out of reach of the keyboard too", async ({
		page,
	}) => {
		await page.goto("/settings/admins");
		const add = page.getByRole("button", { name: "Add admin" });
		await expect(add).toBeDisabled();

		// Enter in the field would otherwise submit the form without ever
		// touching the blocked button.
		await page.getByLabel("Email").fill("keyboard@example.invalid");
		await page.getByLabel("Email").press("Enter");
		await expect(page.getByText("Admin added")).toHaveCount(0);
		await expect(page.getByText("keyboard@example.invalid")).toHaveCount(0);
	});

	test("a control withheld from a non-administrator is absent, not blocked", async ({
		page,
		sql,
	}) => {
		await resetSeededTables(sql);
		const group = await seedServerGroup(sql, { name: "absent-not-blocked" });

		// As an administrator, archiving the empty group is there and blocked.
		await page.goto(`/fleet/groups/${group.id}`);
		await expect(page.getByRole("button", { name: "Archive" })).toBeVisible();
		await expect(page.getByLabel(/requires write mode/i).first()).toBeVisible();

		// As someone who is not, it is not there at all, and nothing on the page
		// is presented as waiting on a mode.
		await page.route("**/api/commons/is_current_user_admin", (route) =>
			route.fulfill({ json: false }),
		);
		await page.reload();
		await expect(page.getByText("absent-not-blocked").first()).toBeVisible();
		await expect(page.getByRole("button", { name: "Archive" })).toHaveCount(0);
		await expect(page.getByLabel(/requires (write|danger) mode/i)).toHaveCount(0);
	});
});

test.describe("safety modes, raised", () => {
	test("un-silencing stays reachable in write mode, where silencing is not", async ({
		page,
		sql,
	}) => {
		await resetSeededTables(sql);
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const server = await seedServer(sql, {
			name: "silence-by-grade",
			type: "tamanu-central",
		});
		await seedStatus(sql, {
			serverId: server.id,
			healthy: false,
			health: [{ check: "postgres", result: "failed" }],
		});

		// Silencing is danger, which this page starts in.
		await page.goto(`/fleet/applications/${server.id}`);
		await page.getByRole("button", { name: "Silence postgres" }).click();
		await page.getByRole("button", { name: "For this server" }).click();
		const manage = page.getByRole("button", { name: "Manage silence for postgres" });
		await expect(manage).toBeVisible();

		// Down to write: the popover still opens, because un-silencing is write
		// even though silencing is not.
		await raiseTo(page, "write");
		await expect(manage).toBeEnabled();
		await manage.click();
		const unsilence = page.getByRole("button", { name: "Un-silence" });
		await expect(unsilence).toBeEnabled();
		await unsilence.click();

		// With nothing left to un-silence, the popover only offers silencing, so
		// write no longer opens it.
		const silence = page.getByRole("button", { name: "Silence postgres" });
		await expect(silence).toBeDisabled();
		await expect(page.getByLabel(/requires danger mode/i).first()).toBeVisible();
	});

	test("a control disabled for a reason of its own carries no stripe", async ({
		page,
	}) => {
		// Hold the mint open, so the button sits disabled for a request in
		// flight rather than for its grade.
		let release: () => void = () => {};
		const held = new Promise<void>((resolve) => {
			release = resolve;
		});
		await page.route("**/api/mcp_tokens/mint", async (route) => {
			await held;
			await route.continue();
		});

		await page.goto("/settings/mcp-tokens");
		await page.getByLabel(/name/i).first().fill("in-flight");
		await page.getByRole("button", { name: "Mint token" }).click();

		const minting = page.getByRole("button", { name: "Minting…" });
		await expect(minting).toBeDisabled();
		await expect(minting).toHaveCSS("background-image", "none");
		await expect(minting).not.toHaveCSS("filter", "grayscale(0.8)");

		release();
	});
});
