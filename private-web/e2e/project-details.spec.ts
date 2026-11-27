// A group's project details on its page. spec: GRP "Project details"
//
// The stack's server answers as the development administrator, so a PM or an
// all-level caller is the interface's answer to the level probe; the server's
// refusals at each level are covered by the private server's own tests.

import type { Page } from "@playwright/test";
import { raiseTo } from "./safety";
import { resetSeededTables, seedServerGroup, seedTailscaleUser } from "./seed";
import { expect, test } from "./test-fixtures";

const panel = (page: Page) => page.getByRole("region", { name: "Project" });
const detail = (page: Page, label: string) =>
	panel(page).getByRole("row", { name: new RegExp(`^${label}`) }).getByRole("cell");
const editProject = (page: Page) =>
	panel(page).getByRole("button", { name: "Edit project" });

function asLevel(page: Page, level: "admin" | "pm" | "all") {
	return page.route("**/api/commons/current_user_level", (route) =>
		route.fulfill({ json: level }),
	);
}

test.describe("project details, in write mode", () => {
	test.use({ safetyMode: "read-only" });

	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("a PM edits a group's project, PM in charge and contact from its page", async ({
		page,
		sql,
	}) => {
		await seedTailscaleUser(sql, { login: "sam@example.org", name: "Sam Lee" });
		const group = await seedServerGroup(sql, {
			name: "Northgate",
			pmInCharge: "jo@example.org",
		});
		await asLevel(page, "pm");
		await page.goto(`/fleet/groups/${group.id}`);
		await raiseTo(page, "write");

		await editProject(page).click();
		const dialog = page.getByRole("dialog", { name: "Edit project" });
		// The form holds only the three details.
		await expect(dialog.getByRole("textbox")).toHaveCount(3);
		await dialog.getByLabel("Project").fill("District rollout, phase 2");
		await dialog.getByLabel("PM in charge").fill("Sam@Example.org");
		await dialog.getByLabel("Contact on the ground").fill("Ana, IT lead");
		await dialog.getByRole("button", { name: "Save" }).click();
		await expect(dialog).toHaveCount(0);

		await expect(detail(page, "Project")).toHaveText("District rollout, phase 2");
		// A login Canopy knows reads by its display name, whatever its case.
		await expect(detail(page, "PM in charge")).toContainText("Sam Lee");
		await expect(detail(page, "Contact on the ground")).toHaveText("Ana, IT lead");
	});

	test("the server's refusal of a PM in charge with no @ is shown in the form", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "Refusal Reef" });
		await asLevel(page, "pm");
		await page.goto(`/fleet/groups/${group.id}`);
		await raiseTo(page, "write");

		await editProject(page).click();
		const dialog = page.getByRole("dialog", { name: "Edit project" });
		await dialog.getByLabel("PM in charge").fill("Sam Lee");
		await dialog.getByRole("button", { name: "Save" }).click();
		await expect(dialog.getByRole("alert")).toContainText(
			"must be a tailnet login or an email address",
		);
		await dialog.getByRole("button", { name: "Cancel" }).click();
		await expect(detail(page, "PM in charge")).toHaveText("None");
	});

	test("a PM sees no Add machine, Edit or Archive on the group page", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "Admin Only" });
		await asLevel(page, "pm");
		await page.goto(`/fleet/groups/${group.id}`);
		await raiseTo(page, "write");

		await expect(editProject(page)).toBeVisible();
		await expect(page.getByRole("link", { name: "Add machine" })).toHaveCount(0);
		await expect(page.getByRole("link", { name: "Edit", exact: true })).toHaveCount(0);
		await expect(page.getByRole("button", { name: "Archive" })).toHaveCount(0);
	});
});

test.describe("project details, read-only", () => {
	test.use({ safetyMode: "read-only" });

	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("a PM in read-only mode sees the edit control blocked, and raising from it opens the form", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "Blocked Bay" });
		await asLevel(page, "pm");
		await page.goto(`/fleet/groups/${group.id}`);

		await expect(panel(page).getByLabel(/requires write mode/i)).toBeVisible();
		await editProject(page).click();
		await page
			.getByRole("dialog")
			.getByRole("button", { name: "Continue in write mode" })
			.click();
		await expect(page.getByRole("dialog", { name: "Edit project" })).toBeVisible();
	});

	test("an all-level caller gets no edit control, nor any PM or admin control", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, {
			name: "Plain View",
			project: "Island clinics",
		});
		await asLevel(page, "all");
		await page.goto(`/fleet/groups/${group.id}`);

		await expect(detail(page, "Project")).toHaveText("Island clinics");
		await expect(editProject(page)).toHaveCount(0);
		await expect(page.getByRole("link", { name: "Add machine" })).toHaveCount(0);
		await expect(page.getByRole("link", { name: "Edit", exact: true })).toHaveCount(0);
		await expect(page.getByRole("button", { name: "Archive" })).toHaveCount(0);
		await expect(page.getByLabel(/requires (write|danger) mode/i)).toHaveCount(0);
	});

	test("the PM in charge reads as entered where Canopy knows no name, and empty details read as none", async ({
		page,
		sql,
	}) => {
		const named = await seedServerGroup(sql, { pmInCharge: "Stranger@Example.org" });
		const empty = await seedServerGroup(sql, { name: "Empty Plains" });

		await page.goto(`/fleet/groups/${named.id}`);
		await expect(detail(page, "PM in charge")).toContainText("Stranger@Example.org");

		await page.goto(`/fleet/groups/${empty.id}`);
		await expect(detail(page, "Project")).toHaveText("None");
		await expect(detail(page, "PM in charge")).toHaveText("None");
		await expect(detail(page, "Contact on the ground")).toHaveText("None");
	});
});
