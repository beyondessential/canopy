// The safety-mode indicator and the blocking it drives.
//
// spec: SAFE
//
// The stack's server runs as the development identity and so skips the mode
// check; what these cover is the interface half — that the mode is visible, that
// raising works the way the spec says, and that a control above the operator's
// mode is present, and offers the raise when it is reached for, rather than
// missing or silently doing nothing.

import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedCheckPolicy,
	seedMachine,
	seedServer,
	seedServerCertificate,
	seedServerGroup,
	seedServerGroupDomain,
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

	test("a blocked control offers the raise, and carries the action out once confirmed", async ({
		page,
	}) => {
		await page.goto("/settings/admins");

		// Present, not removed: the surface has the same shape in every mode.
		const add = page.getByRole("button", { name: "Add admin" });
		await expect(add).toBeVisible();
		await expect(page.getByLabel(/requires danger mode/i).first()).toBeVisible();

		await page.getByLabel("Email").fill("blocked@example.invalid");
		await add.click();

		// Nothing was done yet, not even the form's own validation: the operator
		// is asked, with the action's name and why it needs danger.
		const dialog = page.getByRole("dialog");
		await expect(dialog.getByRole("heading", { name: "Add admin" })).toBeVisible();
		await expect(dialog).toContainText(
			"This action needs danger mode: it issues credentials.",
		);
		await expect(
			dialog.getByRole("button", { name: "Continue in danger mode" }),
		).toBeVisible();
		await expect(page.getByText("blocked@example.invalid")).toHaveCount(0);

		// Cancelling leaves the mode, the control and what was typed as they were.
		await dialog.getByRole("button", { name: "Cancel" }).click();
		await expect(dialog).toHaveCount(0);
		await expect(modeControl(page)).toContainText(/read-only/i);
		await expect(page.getByLabel("Email")).toHaveValue("blocked@example.invalid");
		await expect(page.getByLabel(/requires danger mode/i).first()).toBeVisible();

		// Confirming raises, and the same activation carries on and acts.
		await add.click();
		await dialog.getByRole("button", { name: "Continue in danger mode" }).click();
		await expect(page.getByText("blocked@example.invalid")).toBeVisible();
		await expect(modeControl(page)).toContainText(/danger/i);
		await expect(modeControl(page)).toContainText(/(9|10):\d{2}/);
	});

	test("a blocked danger control raises to danger from write too", async ({
		page,
	}) => {
		await page.goto("/settings/admins");
		await raiseTo(page, "write");

		await page.getByLabel("Email").fill("from-write@example.invalid");
		await page.getByRole("button", { name: "Add admin" }).click();
		const dialog = page.getByRole("dialog");
		await expect(dialog).toContainText("This action needs danger mode");
		await dialog.getByRole("button", { name: "Continue in danger mode" }).click();
		await expect(page.getByText("from-write@example.invalid")).toBeVisible();
		await expect(modeControl(page)).toContainText(/danger/i);
	});

	test("a blocked write control asks for write, and opens its form once confirmed", async ({
		page,
		sql,
	}) => {
		await resetSeededTables(sql);
		const group = await seedServerGroup(sql, { name: "ask-for-write" });
		await page.goto(`/fleet/groups/${group.id}`);

		const edit = page.getByRole("link", { name: "Edit", exact: true });
		await edit.click();
		const dialog = page.getByRole("dialog");
		await expect(
			dialog.getByRole("heading", { name: "Edit group ask-for-write" }),
		).toBeVisible();
		await expect(dialog).toContainText("This action needs write mode");
		await expect(dialog).not.toContainText(":");
		await expect(page).toHaveURL(new RegExp(`/fleet/groups/${group.id}$`));
		await dialog.getByRole("button", { name: "Continue in write mode" }).click();

		await expect(page).toHaveURL(new RegExp(`/fleet/groups/${group.id}/edit$`));
		await expect(modeControl(page)).toContainText(/write/i);
	});

	test("every reason a danger control has is given, and an action with its own confirmation still shows it", async ({
		page,
		sql,
	}) => {
		await resetSeededTables(sql);
		const group = await seedServerGroup(sql, { name: "reasons" });
		await seedServerGroupDomain(sql, {
			groupId: group.id,
			domain: "fiji.tamanu.app",
		});
		const server = await seedServer(sql, {
			name: "central",
			groupId: group.id,
			mayManageTls: true,
		});
		await seedServerCertificate(sql, {
			serverId: server.id,
			name: "leaked.fiji.tamanu.app",
		});

		await page.goto(`/fleet/applications/${server.id}`);
		await page.getByRole("button", { name: "Revoke" }).click();
		const dialog = page.getByRole("dialog");
		await expect(dialog).toContainText(
			"it acts directly on servers and invalidates credentials",
		);
		await dialog.getByRole("button", { name: "Continue in danger mode" }).click();

		// One dialog confirms the mode, the action's own confirms the action.
		await expect(page.getByText(/This cannot be undone/)).toBeVisible();
		await page.getByRole("button", { name: "Cancel" }).click();
		await expect(modeControl(page)).toContainText(/danger/i);
		await expect(page.getByText("revoked", { exact: true })).toHaveCount(0);
	});

	test("a blocked opener raises, then opens its dialog with focus inside it", async ({
		page,
		sql,
	}) => {
		await resetSeededTables(sql);
		const group = await seedServerGroup(sql, { name: "opener" });
		await seedServerGroupDomain(sql, {
			groupId: group.id,
			domain: "fiji.tamanu.app",
		});
		const server = await seedServer(sql, {
			name: "central",
			groupId: group.id,
			mayManageTls: true,
		});
		await seedServerCertificate(sql, {
			serverId: server.id,
			name: "a.fiji.tamanu.app",
		});

		await page.goto(`/fleet/applications/${server.id}`);
		await page.getByRole("button", { name: "Pause" }).click();
		await page
			.getByRole("dialog")
			.getByRole("button", { name: "Continue in danger mode" })
			.click();

		// Its own dialog, with nothing paused until that is submitted.
		const pause = page.getByRole("dialog", { name: /pause/i });
		await expect(pause).toBeVisible();
		await expect(pause.and(page.locator(":focus-within"))).toHaveCount(1);
		await expect(page.getByText("Paused")).toHaveCount(0);
	});

	test("a blocked control that passes nothing on to the page still acts once the raise is confirmed", async ({
		page,
		sql,
	}) => {
		await resetSeededTables(sql);
		const machine = await seedMachine(sql, { name: "plain-box" });

		await page.goto(`/fleet/machines/${machine.id}`);
		await page.getByRole("link", { name: "Edit", exact: true }).click();
		await page
			.getByRole("dialog")
			.getByRole("button", { name: "Continue in write mode" })
			.click();

		await expect(page).toHaveURL(new RegExp(`/fleet/machines/${machine.id}/edit$`));
	});

	test("a middle click on a blocked link asks for the raise rather than opening it", async ({
		page,
		sql,
		context,
	}) => {
		await resetSeededTables(sql);
		const machine = await seedMachine(sql, { name: "middle-box" });

		await page.goto(`/fleet/machines/${machine.id}`);
		const pages = context.pages().length;
		await page.getByRole("link", { name: "Edit", exact: true }).click({ button: "middle" });

		await expect(page.getByRole("dialog")).toContainText("This action needs write mode");
		expect(context.pages()).toHaveLength(pages);
		await page
			.getByRole("dialog")
			.getByRole("button", { name: "Continue in write mode" })
			.click();
		await expect(page).toHaveURL(new RegExp(`/fleet/machines/${machine.id}/edit$`));
	});

	test("Enter in a field beside a blocked declare offers the raise, then declares", async ({
		page,
		sql,
	}) => {
		await resetSeededTables(sql);
		const group = await seedServerGroup(sql, { name: "declare-enter" });
		await seedServerGroupDomain(sql, {
			groupId: group.id,
			domain: "fiji.tamanu.app",
		});
		const server = await seedServer(sql, {
			name: "central",
			groupId: group.id,
			mayManageTls: true,
		});

		await page.goto(`/fleet/applications/${server.id}`);
		const field = page.getByLabel("DNS name to declare");
		await field.fill("extra.fiji.tamanu.app");
		await field.press("Enter");

		const dialog = page.getByRole("dialog");
		await expect(dialog).toContainText("This action needs");
		await expect(field).toHaveValue("extra.fiji.tamanu.app");
		await dialog.getByRole("button", { name: /Continue in/ }).click();
		await expect(page.getByText("extra.fiji.tamanu.app").first()).toBeVisible();
		await expect(field).toHaveValue("");
	});

	test("a blocked toggle group carries out the very option chosen", async ({
		page,
		sql,
	}) => {
		await resetSeededTables(sql);
		await seedCheckPolicy(sql, { source: "alertd", checkName: "db_connect" });

		await page.goto("/settings/healthchecks/sources");
		const row = page.getByRole("row", { name: /alertd/ }).first();
		await row.getByRole("button", { name: "quiet", exact: true }).click();
		await page
			.getByRole("dialog")
			.getByRole("button", { name: "Continue in write mode" })
			.click();

		// The option chosen goes on to its own confirmation, not the group's first.
		const confirm = page
			.getByRole("dialog")
			.filter({ hasText: /set alertd reachability to .quiet./i });
		await confirm.getByRole("button", { name: /confirm/i }).click();
		await expect
			.poll(async () => {
				const rows = await sql.query<{ reachability: string }>(
					`SELECT reachability FROM source_policies WHERE source = 'alertd'`,
				);
				return rows[0]?.reachability ?? null;
			})
			.toBe("quiet");
	});

	test("a blocked select offers the raise on the press that opens it, then opens", async ({
		page,
		sql,
	}) => {
		await resetSeededTables(sql);
		const group = await seedServerGroup(sql, { name: "picker" });
		await seedServerGroupDomain(sql, {
			groupId: group.id,
			domain: "fiji.tamanu.app",
		});
		const server = await seedServer(sql, {
			name: "central",
			groupId: group.id,
			mayManageTls: true,
		});

		await page.goto(`/fleet/applications/${server.id}`);
		await page.getByLabel("Certificate lifetime").click();

		// Nothing is offered to choose until the raise is made.
		await expect(page.getByRole("option", { name: "shortlived" })).toHaveCount(0);
		await page
			.getByRole("dialog")
			.getByRole("button", { name: "Continue in write mode" })
			.click();

		await page.getByRole("option", { name: "shortlived" }).click();
		await expect
			.poll(async () => {
				const [row] = await sql.query<{ certificate_profile: string | null }>(
					"SELECT certificate_profile FROM applications WHERE id = $1",
					[server.id],
				);
				return row.certificate_profile;
			})
			.toBe("shortlived");
	});

	test("a blocked select offers the raise from the keyboard too", async ({
		page,
		sql,
	}) => {
		await resetSeededTables(sql);
		const group = await seedServerGroup(sql, { name: "picker-keys" });
		await seedServerGroupDomain(sql, {
			groupId: group.id,
			domain: "fiji.tamanu.app",
		});
		const server = await seedServer(sql, {
			name: "central",
			groupId: group.id,
			mayManageTls: true,
		});

		await page.goto(`/fleet/applications/${server.id}`);
		await page.getByLabel("Certificate lifetime").focus();
		await page.keyboard.press("ArrowDown");
		const dialog = page.getByRole("dialog");
		await expect(dialog).toContainText("This action needs write mode");
		await expect(page.getByRole("option", { name: "shortlived" })).toHaveCount(0);
		await dialog.getByRole("button", { name: "Continue in write mode" }).click();

		await expect(page.getByRole("option", { name: "shortlived" })).toBeVisible();
	});

	test("a failed raise says the mode is unchanged, and does not carry the action out", async ({
		page,
	}) => {
		await page.route("**/api/safety/raise", (route) =>
			route.fulfill({ status: 500, json: { title: "no" } }),
		);
		await page.goto("/settings/admins");
		await page.getByLabel("Email").fill("refused@example.invalid");
		await page.getByRole("button", { name: "Add admin" }).click();
		await page
			.getByRole("dialog")
			.getByRole("button", { name: "Continue in danger mode" })
			.click();

		await expect(page.getByText("Mode unchanged")).toBeVisible();
		await page.getByRole("button", { name: "Close" }).click();
		await expect(modeControl(page)).toContainText(/read-only/i);
		await expect(page.getByText("refused@example.invalid")).toHaveCount(0);
		await expect(page.getByLabel("Email")).toHaveValue("refused@example.invalid");
	});

	test("a double click on a blocked control carries the action out once", async ({
		page,
	}) => {
		await page.goto("/settings/admins");
		await page.getByLabel("Email").fill("twice@example.invalid");
		await page.getByRole("button", { name: "Add admin" }).dblclick();

		await expect(page.getByRole("dialog")).toHaveCount(1);
		await page
			.getByRole("dialog")
			.getByRole("button", { name: "Continue in danger mode" })
			.click();
		await expect(page.getByText("twice@example.invalid")).toHaveCount(1);
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

	test("machine setup below danger offers its ticket blocked, and mints once the raise is confirmed", async ({
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

		// Minting is danger: read-only, the page does not mint on load, and
		// offers it blocked instead.
		const issue = page.getByRole("button", { name: "Issue enrollment ticket" });
		await expect(issue).toBeVisible();
		await expect(page.getByLabel(/requires danger mode/i).first()).toBeVisible();
		await expect(page.getByText(/bestool canopy register/)).toHaveCount(0);
		expect(await tickets()).toBe(0);

		// Reaching for it asks for the raise, and confirming mints, once: the
		// activation does, not the raise.
		let mints = 0;
		page.on("request", (request) => {
			if (request.url().endsWith("/api/fleet/machines/mint_enrollment")) mints++;
		});
		await issue.click();
		await page
			.getByRole("dialog")
			.getByRole("button", { name: "Continue in danger mode" })
			.click();
		await expect(page.getByText(/bestool canopy register/)).toBeVisible();
		expect(await tickets()).toBeGreaterThan(0);
		expect(mints).toBe(1);
	});

	test("a control that opens a form carries the grade of what the form saves", async ({
		page,
		sql,
	}) => {
		await resetSeededTables(sql);
		const group = await seedServerGroup(sql, { name: "opener-group" });

		// Editing a group is write, so the link into its form is blocked
		// read-only rather than leading to a form that cannot be saved.
		await page.goto(`/fleet/groups/${group.id}`);
		const edit = page.getByRole("link", { name: "Edit", exact: true });
		await expect(page.getByLabel(/requires write mode/i).first()).toBeVisible();
		await expect(edit).toHaveCSS("background-image", /rgba\(255, 152, 0/);
		await edit.click();
		await expect(page).toHaveURL(new RegExp(`/fleet/groups/${group.id}$`));
		await page.getByRole("button", { name: "Cancel" }).click();
		await expect(page.getByRole("dialog")).toHaveCount(0);

		await raiseTo(page, "write");
		await edit.click();
		await expect(page).toHaveURL(new RegExp(`/fleet/groups/${group.id}/edit$`));
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

	test("a blocked control is reachable from the keyboard, and Enter in its form offers the raise", async ({
		page,
	}) => {
		await page.goto("/settings/admins");
		const add = page.getByRole("button", { name: "Add admin" });
		const email = page.getByLabel("Email");
		const dialog = page.getByRole("dialog");
		const confirm = dialog.getByRole("button", { name: "Continue in danger mode" });

		await email.fill("keyboard@example.invalid");
		await email.press("Tab");
		await expect(add).toBeFocused();

		// Enter does what a click does, and cannot also confirm what it opened.
		await page.keyboard.press("Enter");
		await expect(dialog).toBeVisible();
		await expect(confirm).not.toBeFocused();
		await expect(page.locator(".MuiDialog-root:focus-within")).toHaveCount(1);
		await page.keyboard.press("Escape");
		await expect(dialog).toHaveCount(0);
		await expect(add).toBeFocused();

		// So does Space, whose key-up is what activates.
		await page.keyboard.press("Space");
		await expect(dialog).toBeVisible();
		await expect(confirm).not.toBeFocused();
		await page.keyboard.press("Escape");
		await expect(dialog).toHaveCount(0);

		// Enter in a field of a form whose save is blocked is the same as the save.
		await email.press("Enter");
		await expect(dialog).toBeVisible();
		await expect(page.getByText("keyboard@example.invalid")).toHaveCount(0);
		await confirm.click();
		await expect(page.getByText("keyboard@example.invalid")).toBeVisible();
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

test.describe("safety modes, below a control that is disabled for its own reason", () => {
	test.use({ safetyMode: "read-only" });

	test("it carries no stripe and offers no raise, in any mode", async ({
		page,
	}) => {
		// Hold the mint open, so the button sits disabled for a request in flight.
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
		await page
			.getByRole("dialog")
			.getByRole("button", { name: "Continue in danger mode" })
			.click();
		const minting = page.getByRole("button", { name: "Minting…" });
		await expect(minting).toBeDisabled();

		// Back below its grade, it is still disabled rather than blocked.
		await lower(page);
		await expect(minting).toBeDisabled();
		await expect(minting).toHaveCSS("background-image", "none");
		await expect(page.getByLabel(/requires danger mode/i)).toHaveCount(0);
		await minting.click({ force: true });
		await expect(page.getByRole("dialog")).toHaveCount(0);

		release();
	});
});

test.describe("safety modes, raised", () => {
	test("a usable control is drawn in its grade's colour", async ({ page }) => {
		// Danger: adding an allow-list entry, in the danger colour.
		await page.goto("/settings/admins");
		await expect(page.getByRole("button", { name: "Add admin" })).toHaveCSS(
			"background-color",
			"rgb(211, 47, 47)",
		);

		// Write: saving a snippet, in the write colour.
		await page.goto("/bestool/snippets");
		await expect(page.getByRole("button", { name: "Add" })).toHaveCSS(
			"background-color",
			"rgb(237, 108, 2)",
		);
	});

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
		// write no longer opens it: reaching for it asks for danger instead.
		const silence = page.getByRole("button", { name: "Silence postgres" });
		await expect(page.getByLabel(/requires danger mode/i).first()).toBeVisible();
		await silence.click();
		await expect(page.getByRole("dialog")).toContainText(
			"This action needs danger mode",
		);
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
