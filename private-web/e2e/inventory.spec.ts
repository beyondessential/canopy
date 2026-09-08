import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedMaintenanceWindow,
	seedServer,
	seedServerGroup,
} from "./seed";

// The inventory a configuration run would receive, presented on the group
// page: one panel per environment, the variables set at each scope, and
// whether a run could take the environment's lease.
// spec: INV#presentation
test.describe("group inventory", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("shows one panel per environment, with the machines in it", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka", tags: {} });
		await seedServer(sql, {
			name: "kamaka-prod-central",
			host: "https://central.kamaka.e2e.invalid",
			type: "tamanu-central",
			rank: "production",
			groupId: group.id,
			tags: {},
		});
		await seedServer(sql, {
			name: "kamaka-demo-central",
			host: "https://central.demo.kamaka.e2e.invalid",
			type: "tamanu-central",
			rank: "demo",
			groupId: group.id,
			tags: {},
		});

		await page.goto(`/fleet/groups/${group.id}`);

		const inventory = page.getByTestId("group-inventory");
		const production = inventory.getByTestId("environment-production");
		const demo = inventory.getByTestId("environment-demo");
		await expect(production).toBeVisible();
		await expect(demo).toBeVisible();

		// Each environment carries only the machines its own applications run
		// on, though both sit in the same group.
		await expect(production.getByTestId("inventory-machine")).toHaveCount(1);
		await expect(
			production.getByText("kamaka-prod-central", { exact: true }),
		).toBeVisible();
		await expect(demo.getByTestId("inventory-machine")).toHaveCount(1);
		await expect(
			demo.getByText("kamaka-demo-central", { exact: true }),
		).toBeVisible();
	});

	test("sets a variable at each scope and marks the one that overrides", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka", tags: {} });
		await seedServer(sql, {
			name: "kamaka-prod-central",
			type: "tamanu-central",
			rank: "production",
			groupId: group.id,
			tags: {},
		});

		await page.goto(`/fleet/groups/${group.id}`);
		const production = page
			.getByTestId("group-inventory")
			.getByTestId("environment-production");

		const form = production.getByTestId("set-variable");
		await form.getByLabel("Scope").click();
		await page.getByRole("option", { name: "Whole group" }).click();
		await form.getByLabel("Name").fill("log_level");
		await form.getByLabel("Value").fill("info");
		await form.getByRole("button", { name: "Set" }).click();
		await expect(production.getByTestId("var")).toContainText(
			"log_level = info",
		);

		// The same name on the machine overrides it, which is the case an
		// operator chasing a value that isn't taking effect is looking for.
		await form.getByLabel("Scope").click();
		await page.getByRole("option", { name: "kamaka-prod-central" }).click();
		await form.getByLabel("Name").fill("log_level");
		await form.getByLabel("Value").fill("trace");
		await form.getByRole("button", { name: "Set" }).click();
		await expect(production.getByTestId("overriding-var")).toContainText(
			"log_level = trace",
		);
	});

	test("shows a secret variable by name and never by value", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka", tags: {} });
		await seedServer(sql, {
			name: "kamaka-prod-central",
			type: "tamanu-central",
			rank: "production",
			groupId: group.id,
			tags: {},
		});

		await page.goto(`/fleet/groups/${group.id}`);
		const production = page
			.getByTestId("group-inventory")
			.getByTestId("environment-production");

		const form = production.getByTestId("set-variable");
		await form.getByLabel("Name").fill("salt");
		await form.getByTestId("value-is-secret").getByRole("checkbox").check();
		await form.getByLabel("Value").fill("pepper");
		await form.getByRole("button", { name: "Set" }).click();

		await expect(production.getByTestId("secret-var")).toContainText(
			"salt = secret",
		);
		await expect(page.getByText("pepper")).toHaveCount(0);

		await production.getByTestId("remove-salt").click();
		await expect(production.getByTestId("secret-var")).toHaveCount(0);
	});

	test("says who holds the environment while someone else's maintenance does", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka", tags: {} });
		await seedServer(sql, {
			name: "kamaka-prod-central",
			type: "tamanu-central",
			rank: "production",
			groupId: group.id,
			tags: {},
		});
		await seedMaintenanceWindow(sql, {
			serverGroupId: group.id,
			declaredBy: "someone@else.invalid",
			note: "upgrading to v2.55",
		});

		await page.goto(`/fleet/groups/${group.id}`);
		const production = page
			.getByTestId("group-inventory")
			.getByTestId("environment-production");

		await expect(production.getByTestId("run-declared")).toContainText(
			"someone@else.invalid",
		);
		await expect(production.getByTestId("declare-work")).toHaveCount(0);
	});

	test("says so when the work is declared over a machine in the environment", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka", tags: {} });
		const central = await seedServer(sql, {
			name: "kamaka-prod-central",
			type: "tamanu-central",
			rank: "production",
			groupId: group.id,
			tags: {},
		});
		await seedMaintenanceWindow(sql, {
			machineId: central.machineId,
			declaredBy: "someone@else.invalid",
			note: "replacing a disk",
		});

		await page.goto(`/fleet/groups/${group.id}`);
		const production = page
			.getByTestId("group-inventory")
			.getByTestId("environment-production");

		await expect(production.getByTestId("run-declared")).toContainText(
			"someone@else.invalid",
		);
		await expect(production.getByTestId("declare-work")).toHaveCount(0);
	});

	test("gives the line a run on the environment is started with", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka south", tags: {} });
		await seedServer(sql, {
			name: "kamaka-prod-central",
			type: "tamanu-central",
			rank: "production",
			groupId: group.id,
			tags: {},
		});

		await page.goto(`/fleet/groups/${group.id}`);
		const production = page
			.getByTestId("group-inventory")
			.getByTestId("environment-production");

		// Canopy's address and the environment's identity are filled in; the
		// playbook is the operator's to pick. A group name a shell would split
		// is quoted, since the line is copied to be pasted into one.
		const line = `CANOPY_URL=${new URL(page.url()).origin} CANOPY_GROUP='kamaka south' CANOPY_RANK=production ansible-playbook -i inventory/canopy.yml <playbook>`;
		await expect(production.getByTestId("run-command")).toHaveText(line);

		// Copying it is the point of the panel, so the clipboard carries the
		// whole line and not the visible fragment of a scrolled block.
		await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
		await production.getByRole("button", { name: "Copy the command" }).click();
		expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(line);
	});

	test("declares the work from beside the line", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "drifting", tags: {} });
		await seedServer(sql, {
			name: "drifting-prod-central",
			type: "tamanu-central",
			rank: "production",
			groupId: group.id,
			tags: {},
		});

		await page.goto(`/fleet/groups/${group.id}`);
		const production = page
			.getByTestId("group-inventory")
			.getByTestId("environment-production");

		await expect(production.getByTestId("run-command")).toContainText(
			"CANOPY_GROUP=drifting CANOPY_RANK=production",
		);

		await production.getByTestId("declare-work").click();
		await expect(
			page.getByRole("heading", { name: "Declare maintenance — drifting" }),
		).toBeVisible();
		await page.getByRole("button", { name: "Declare", exact: true }).click();

		await expect(production.getByTestId("run-declared")).toBeVisible();
		await expect(production.getByTestId("declare-work")).toHaveCount(0);

		// Lifting it in the section below is the same state, so the panel offers
		// the declaration again rather than waiting for a reload.
		await page
			.getByTestId("maintenance-section")
			.getByRole("button", { name: "Lift" })
			.click();
		await expect(production.getByTestId("declare-work")).toHaveCount(1);
		await expect(production.getByTestId("run-declared")).toHaveCount(0);
	});
});
