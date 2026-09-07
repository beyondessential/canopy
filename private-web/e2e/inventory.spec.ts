import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedMaintenanceWindow,
	seedServer,
	seedServerGroup,
} from "./seed";

// The inventory a configuration run would receive, presented on the group
// page: one panel per environment, the group's variables once, and each
// application's own variables under it.
// spec: INV#presentation
test.describe("group inventory", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("shows one panel per environment, with each application's address", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, {
			name: "kamaka",
			tags: { timezone: "Pacific/Auckland", tamanu_version: "v2.54.8" },
		});
		await seedServer(sql, {
			name: "kamaka-prod-central",
			host: "https://central.kamaka.e2e.invalid",
			type: "tamanu-central",
			rank: "production",
			groupId: group.id,
			tags: { ansible_user: "ubuntu" },
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

		// The group's values are shown once per environment rather than repeated
		// under every application.
		await expect(
			production.getByText("timezone = Pacific/Auckland").first(),
		).toBeVisible();
		await expect(production.getByText("ansible_user = ubuntu")).toBeVisible();

		// Each environment carries only its own applications, and the address
		// falls back to the recorded host where no device is bound to the box.
		await expect(production.getByTestId("inventory-application")).toHaveCount(1);
		await expect(
			production.getByText("kamaka-prod-central", { exact: true }),
		).toBeVisible();
		await expect(
			production.getByText("central.kamaka.e2e.invalid", { exact: true }),
		).toBeVisible();
		await expect(demo.getByTestId("inventory-application")).toHaveCount(1);
		await expect(
			demo.getByText("kamaka-demo-central", { exact: true }),
		).toBeVisible();
	});

	test("marks an application that overrides a group value", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, {
			name: "drifting",
			tags: { elastic_agent_enabled: "false" },
		});
		await seedServer(sql, {
			name: "drifting-prod-central",
			type: "tamanu-central",
			rank: "production",
			groupId: group.id,
			tags: { elastic_agent_enabled: "true" },
		});
		await seedServer(sql, {
			name: "drifting-prod-facility",
			type: "tamanu-facility",
			rank: "production",
			groupId: group.id,
			tags: {},
		});

		await page.goto(`/fleet/groups/${group.id}`);

		const production = page
			.getByTestId("group-inventory")
			.getByTestId("environment-production");

		// The group's value and the application's override are both shown, and
		// only the override is marked as one.
		await expect(
			production.getByText("elastic_agent_enabled = false"),
		).toBeVisible();
		await expect(production.getByTestId("overriding-var")).toHaveText(
			"elastic_agent_enabled = true",
		);

		// An application setting nothing of its own says so, rather than showing
		// the group's values again as if it set them.
		await expect(production.getByText("Sets nothing of its own")).toBeVisible();
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

		const form = production.getByTestId("set-secret");
		await form.getByLabel("Name").fill("salt");
		await form.getByLabel("Value").fill("pepper");
		await form.getByRole("button", { name: "Set" }).click();

		await expect(production.getByTestId("secret-var")).toContainText(
			"salt = secret",
		);
		await expect(page.getByText("pepper")).toHaveCount(0);

		// And it is a variable, not a tag: the tag of that name is refused.
		await form.getByLabel("Name").fill("salt");
		await form.getByLabel("Value").fill("again");
		await form.getByRole("button", { name: "Set" }).click();
		await expect(production.getByTestId("secret-var")).toHaveCount(1);

		await production.getByTestId("remove-salt").click();
		await expect(production.getByTestId("secret-var")).toHaveCount(0);
	});

	test("says a run would be refused while someone else's maintenance holds", async ({
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
		await seedMaintenanceWindow(sql, {
			serverGroupId: group.id,
			declaredBy: "someone@else.invalid",
			note: "upgrading to v2.55",
		});

		await page.goto(`/groups/${group.id}`);
		const production = page
			.getByTestId("group-inventory")
			.getByTestId("environment-production");

		await expect(
			production.getByText(/under maintenance declared by someone@else\.invalid/),
		).toBeVisible();
		await expect(production.getByText(/upgrading to v2\.55/)).toBeVisible();
		await expect(production.getByTestId("inventory-application")).toHaveCount(0);
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
		await production
			.getByRole("button", { name: "Copy the command" })
			.click();
		expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(
			line,
		);
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

		// The window is the reader's own, so the inventory is still served and
		// the panel says the run is held for them.
		await expect(production.getByTestId("run-declared")).toContainText(
			"Your work is declared",
		);
		await expect(production.getByTestId("declare-work")).toHaveCount(0);
		await expect(production.getByTestId("inventory-application")).toHaveCount(1);

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
