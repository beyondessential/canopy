import type { Locator } from "@playwright/test";
import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedAnsibleCatalogue,
	seedAnsibleVariable,
	seedAuditEntry,
	seedMachine,
	seedMaintenanceWindow,
	seedServer,
	seedServerGroup,
} from "./seed";

// What a configuration run would be served, presented on the group
// page: one environment at a time, the variables set at each scope, and
// whether a run could take the environment's lease.
// spec: ANSRUN#presentation
/** Type a name and a value into a scope's entry row and set it. */
async function setVariable(
	scope: Locator,
	name: string,
	value: string,
): Promise<void> {
	await scope.getByTestId("variable-name").fill(name);
	await scope.getByLabel("Value").fill(value);
	await scope.getByRole("button", { name: "Set", exact: true }).click();
}

test.describe("group Ansible", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("shows the selected environment, with the machines in it", async ({
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

		const ansible = page.getByTestId("group-ansible");
		const production = ansible.getByTestId("environment-production");
		await expect(production).toBeVisible();
		await expect(ansible.getByTestId("environment-demo")).toHaveCount(0);

		// Each environment lists only the machines its own applications run on,
		// though both sit in the same group.
		await expect(production.getByTestId("ansible-machine")).toHaveCount(1);
		await expect(production.getByTestId("ansible-machine")).toContainText(
			"kamaka-prod-central",
		);

		await ansible.getByLabel("Environment").click();
		await page.getByRole("option", { name: "demo" }).click();

		const demo = ansible.getByTestId("environment-demo");
		await expect(demo.getByTestId("ansible-machine")).toHaveCount(1);
		await expect(demo.getByTestId("ansible-machine")).toContainText(
			"kamaka-demo-central",
		);
		await expect(ansible.getByTestId("environment-production")).toHaveCount(0);
	});

	test("offers nothing to select where the group has one environment", async ({
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

		const ansible = page.getByTestId("group-ansible");
		await expect(ansible.getByTestId("environment-production")).toBeVisible();
		await expect(ansible.getByLabel("Environment")).toHaveCount(0);
	});

	test("says there is nothing to configure with no live applications", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka", tags: {} });

		await page.goto(`/fleet/groups/${group.id}`);

		const ansible = page.getByTestId("group-ansible");
		await expect(ansible).toContainText("no environment to configure");
		await expect(ansible.getByLabel("Environment")).toHaveCount(0);
	});

	// A machine is in the environment its rank names before anything on it has
	// reported, so a ranked box with nothing on it is one to configure.
	// spec: GRP#environments
	test("offers the environment of a ranked machine with nothing on it", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka", tags: {} });
		await seedMachine(sql, {
			name: "kamaka-new",
			groupId: group.id,
			rank: "production",
		});

		await page.goto(`/fleet/groups/${group.id}`);

		const ansible = page.getByTestId("group-ansible");
		await expect(ansible.getByTestId("environment-production")).toBeVisible();
		await expect(ansible).not.toContainText("no environment to configure");
	});

	// A pending application is in no environment, so a group with nothing
	// ranked has none to configure and the picker never offers a pending bucket.
	// spec: GRP#environments
	test("offers no environment while nothing in the group is ranked", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka", tags: {} });
		await seedServer(sql, {
			name: "kamaka-new",
			type: "tamanu-central",
			rank: null,
			groupId: group.id,
			tags: {},
		});

		await page.goto(`/fleet/groups/${group.id}`);

		const ansible = page.getByTestId("group-ansible");
		await expect(ansible).toContainText("no environment to configure");
		await expect(ansible.getByLabel("Environment")).toHaveCount(0);
	});

	test("sets a variable on the environment the selector names", async ({
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
		await seedServer(sql, {
			name: "kamaka-demo-central",
			type: "tamanu-central",
			rank: "demo",
			groupId: group.id,
			tags: {},
		});

		await page.goto(`/fleet/groups/${group.id}`);
		const ansible = page.getByTestId("group-ansible");

		await ansible.getByLabel("Environment").click();
		await page.getByRole("option", { name: "demo" }).click();

		const demo = ansible.getByTestId("environment-demo");
		await setVariable(demo.getByTestId("scope-environment"), "log_level", "trace");
		await expect(demo.getByTestId("var")).toContainText("log_level");
		await expect(demo.getByTestId("var")).toContainText('"trace"');

		// The variable is the demo environment's, so production does not carry
		// it, and the machines listed are the selected environment's.
		await ansible.getByLabel("Environment").click();
		await page.getByRole("option", { name: "production" }).click();
		const production = ansible.getByTestId("environment-production");
		await expect(production.getByTestId("var")).toHaveCount(0);
		await expect(production.getByTestId("ansible-machine")).toHaveCount(1);
		await expect(production.getByTestId("ansible-machine")).toContainText(
			"kamaka-prod-central",
		);
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
			.getByTestId("group-ansible")
			.getByTestId("environment-production");

		await setVariable(production.getByTestId("scope-group"), "log_level", "info");
		await expect(production.getByTestId("var")).toContainText("log_level");

		// The same name on the machine overrides it, which is the case an
		// operator chasing a value that isn't taking effect is looking for.
		await production.getByTestId("ansible-machine").getByTestId("scope-expand").click();
		await setVariable(production.getByTestId("ansible-machine"), "log_level", "trace");
		await expect(production.getByTestId("overriding-var")).toContainText(
			'"trace"',
		);
		await expect(production.getByTestId("overriding-var")).toContainText(
			"overrides the group",
		);
	});

	// Whether a machine is managed is a switch on it, not a listed variable; a
	// machine that is not managed is greyed and starts collapsed.
	// spec: INV#managed-machines
	test("switches a machine to managed by Ansible", async ({ page, sql }) => {
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
			.getByTestId("group-ansible")
			.getByTestId("environment-production");
		const machine = production.getByTestId("ansible-machine");
		const toggle = machine.getByTestId("scope-expand");
		await expect(toggle).toHaveAttribute("aria-expanded", "false");
		await expect(machine.getByTestId("variable-name")).toBeHidden();

		const managed = machine.getByTestId("managed-switch").locator("input");
		await managed.click();
		await expect(managed).toBeChecked();
		await expect(production.getByTestId("variables-count")).toHaveText("0");
		await expect(production.getByTestId("var")).toHaveCount(0);
		const [row] = await sql.query<{ value: unknown }>(
			"SELECT value FROM ansible_variables WHERE name = 'managed_by_ansible'",
		);
		expect(row.value).toBe(true);

		// Seen fresh, a managed machine starts expanded.
		await page.reload();
		await expect(
			page
				.getByTestId("group-ansible")
				.getByTestId("ansible-machine")
				.getByTestId("scope-expand"),
		).toHaveAttribute("aria-expanded", "true");
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
			.getByTestId("group-ansible")
			.getByTestId("environment-production");

		const scope = production.getByTestId("scope-environment");
		await scope.getByTestId("variable-name").fill("salt");
		await scope.getByTestId("value-is-secret").click();
		await scope.getByLabel("Value").fill("pepper");
		await scope.getByRole("button", { name: "Set", exact: true }).click();

		await expect(production.getByTestId("secret-var")).toContainText("salt");
		await expect(production.getByTestId("secret-var")).toContainText("secret");
		await expect(page.getByText("pepper")).toHaveCount(0);

		await production.getByTestId("remove-salt").click();
		await expect(production.getByTestId("secret-var")).toHaveCount(0);
	});

	// A variable's value is edited in its row, holding the value it has; a
	// secret's field opens empty, and leaving the row keeps the value.
	// spec: ANSVAR#presentation
	test("edits a variable's value in its row", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "kamaka", tags: {} });
		await seedServer(sql, {
			name: "kamaka-prod-central",
			type: "tamanu-central",
			rank: "production",
			groupId: group.id,
			tags: {},
		});
		await seedAnsibleVariable(sql, {
			serverGroupId: group.id,
			rank: "production",
			name: "log_level",
			value: "info",
		});

		await page.goto(`/fleet/groups/${group.id}`);
		const production = page
			.getByTestId("group-ansible")
			.getByTestId("environment-production");
		await production.getByTestId("variables-toggle").click();
		const scope = production.getByTestId("scope-environment");
		const logLevel = scope.getByTestId("var").filter({ hasText: "log_level" });

		await scope.getByTestId("edit-log_level").click();
		const field = scope.getByTestId("edit-variable").getByLabel("Value");
		// Undescribed by any catalogue, so its value is JSON.
		await expect(field).toHaveValue('"info"');
		await field.fill("debug");
		await field.press("Enter");
		await expect(scope.getByTestId("edit-variable")).toHaveCount(0);
		await expect(logLevel).toContainText('"debug"');

		// Leaving without saving keeps the value.
		await scope.getByTestId("edit-log_level").click();
		await scope.getByTestId("edit-variable").getByLabel("Value").fill("trace");
		await page.keyboard.press("Escape");
		await expect(scope.getByTestId("edit-variable")).toHaveCount(0);
		await expect(logLevel).toContainText('"debug"');

		// A secret opens empty, and needs a new value to save.
		await scope.getByTestId("variable-name").fill("salt");
		await scope.getByTestId("value-is-secret").click();
		await scope.getByTestId("set-variable").getByLabel("Value").fill("pepper");
		await scope.getByRole("button", { name: "Set", exact: true }).click();
		await scope.getByTestId("edit-salt").click();
		const secret = scope.getByTestId("edit-variable");
		await expect(secret.getByLabel("Value")).toHaveValue("");
		await expect(secret.getByRole("button", { name: "Save" })).toBeDisabled();
		await secret.getByLabel("Value").fill("paprika");
		await secret.getByRole("button", { name: "Save" }).click();
		await expect(scope.getByTestId("edit-variable")).toHaveCount(0);
		await expect(scope.getByTestId("secret-var")).toContainText("salt");
		await expect(page.getByText("paprika")).toHaveCount(0);
	});

	// Removing asks nothing and offers to undo; a variable's history puts back
	// an earlier value, and a removal is put back from its scope after the offer
	// has gone.
	// spec: ANSVAR#presentation
	test("undoes a removal and puts back an earlier value", async ({ page, sql }) => {
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
			.getByTestId("group-ansible")
			.getByTestId("environment-production");
		const scope = production.getByTestId("scope-environment");
		await setVariable(scope, "log_level", "info");
		await setVariable(scope, "log_level", "debug");
		const logLevel = scope.getByTestId("var").filter({ hasText: "log_level" });
		await expect(logLevel).toContainText('"debug"');

		// The history shows each change, and undoing the last puts back "info".
		await scope.getByTestId("history-log_level").click();
		const history = page.getByTestId("variable-history");
		await expect(history.getByTestId("history-entry")).toHaveCount(2);
		await expect(history.getByTestId("history-entry").first()).toContainText(
			'Set to "debug", was "info"',
		);
		await history.getByTestId("history-restore").click();
		await expect(page.getByTestId("variable-history")).toHaveCount(0);
		await expect(logLevel).toContainText('"info"');

		// Removing offers an undo, which puts it back.
		await scope.getByTestId("remove-log_level").click();
		await expect(logLevel).toHaveCount(0);
		await page.getByTestId("undo-removal").click();
		await expect(logLevel).toContainText('"info"');

		// Once the offer has gone, the scope lists the removal.
		await scope.getByTestId("remove-log_level").click();
		await expect(logLevel).toHaveCount(0);
		await page.keyboard.press("Escape");
		const removed = scope.getByTestId("removed-variables");
		await expect(removed).toContainText("log_level");
		await removed.getByText("log_level").click();
		await page.getByTestId("history-restore").first().click();
		await expect(logLevel).toContainText('"info"');
		await expect(scope.getByTestId("removed-variables")).toHaveCount(0);
	});

	// A scope's history names each variable changed there, a removal among them
	// even once the variable is set again and off the list of removals, and
	// puts it back. Another scope's changes are not in it.
	// spec: ANSVAR#presentation
	test("puts back a removed variable from its scope's history", async ({ page, sql }) => {
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
			.getByTestId("group-ansible")
			.getByTestId("environment-production");
		const scope = production.getByTestId("scope-environment");
		await setVariable(production.getByTestId("scope-group"), "timezone", "Pacific/Fiji");
		await setVariable(scope, "log_level", "info");
		await setVariable(scope, "region", "nz");
		const logLevel = scope.getByTestId("var").filter({ hasText: "log_level" });
		await expect(scope.getByTestId("var").filter({ hasText: "region" })).toHaveCount(1);
		await scope.getByTestId("remove-log_level").click();
		await expect(logLevel).toHaveCount(0);
		await page.keyboard.press("Escape");
		await setVariable(scope, "log_level", "debug");
		await expect(logLevel).toContainText('"debug"');
		await expect(scope.getByTestId("removed-variables")).toHaveCount(0);
		// A removal whose entry holds no value, as one written before entries
		// held them, names no value and offers nothing to put back.
		await seedAuditEntry(sql, {
			action: "inventory_variable_removed",
			minutesAgo: 60,
			records: [
				{
					kind: "variable",
					id: `inv-vars-production-${group.id}:proxy`,
					name: "proxy",
				},
				{ kind: "group", id: group.id, name: "kamaka" },
			],
			detail: { rank: "production", secret: false },
		});

		await page.getByRole("button", { name: "History of production", exact: true }).click();
		const history = page.getByTestId("variable-history");
		const entries = history.getByTestId("history-entry");
		await expect(entries).toHaveCount(5);
		await expect(entries.nth(0)).toContainText('log_level Set to "debug"');
		await expect(entries.nth(1)).toContainText('log_level Removed "info"');
		await expect(entries.nth(2)).toContainText('region Set to "nz"');
		await expect(entries.nth(3)).toContainText('log_level Set to "info"');
		await expect(entries.nth(4)).toContainText("proxy Removed");
		await expect(entries.nth(4)).not.toContainText("null");
		await expect(entries.nth(4).getByTestId("history-restore")).toHaveCount(0);
		await expect(history).not.toContainText("timezone");

		await entries.nth(1).getByTestId("history-restore").click();
		await expect(page.getByTestId("variable-history")).toHaveCount(0);
		await expect(logLevel).toContainText('"info"');
	});

	// The name completes from the catalogue, showing each variable's type, and
	// the value is entered in a form shaped by it. A variable limited to other
	// scopes is not offered.
	// spec: ANSVAR#presentation
	test("completes names from the catalogue and shapes the value by its type", async ({
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
		await seedAnsibleCatalogue(sql);

		await page.goto(`/fleet/groups/${group.id}`);
		const production = page
			.getByTestId("group-ansible")
			.getByTestId("environment-production");
		const environment = production.getByTestId("scope-environment");

		await environment.getByTestId("variable-name").fill("elastic");
		const option = page.getByRole("option", { name: /elastic_agent_enabled/ });
		await expect(option).toContainText("bool");
		await expect(option).toContainText("Whether the endpoint security agent");
		await option.click();
		await environment.getByLabel("Value").click();
		await page.getByRole("option", { name: "true", exact: true }).click();
		await environment.getByRole("button", { name: "Set", exact: true }).click();
		await expect(environment.getByTestId("var")).toContainText("elastic_agent_enabled");
		await expect(environment.getByTestId("var")).toContainText("true");

		// A machine-only variable is not offered at the environment, though
		// another name matching the same text is.
		await environment.getByTestId("variable-name").fill("host");
		await expect(
			page.getByRole("option", { name: /tamanu_caddy_extra_hostnames/ }),
		).toBeVisible();
		await expect(page.getByRole("option", { name: /ansible_host/ })).toHaveCount(0);
		await environment.getByTestId("variable-name").fill("");

		// An object is a field for each property, required ones marked, and a
		// variable the catalogue calls secret is entered as one.
		const machine = production.getByTestId("ansible-machine");
		await machine.getByTestId("scope-expand").click();
		await machine.getByTestId("variable-name").fill("tamanu_caddy");
		await page.getByRole("option", { name: /tamanu_caddy_tls_dns_iam/ }).click();
		const form = machine.getByTestId("object-field");
		await expect(form.getByLabel("access_key_id")).toBeVisible();
		await expect(form.getByLabel("secret_access_key")).toBeVisible();
		await expect(machine.getByTestId("value-is-secret")).toBeDisabled();
		await expect(machine.getByTestId("value-is-secret")).toHaveAttribute(
			"aria-pressed",
			"true",
		);
		await expect(machine.getByRole("button", { name: "Set", exact: true })).toBeDisabled();

		await form.getByLabel("access_key_id").fill("AKIAEXAMPLE");
		await form.getByLabel("secret_access_key").fill("short");
		await expect(form).toContainText("5 of 8 characters");
		await form.getByLabel("secret_access_key").fill("long-enough-secret");
		await machine.getByRole("button", { name: "Set", exact: true }).click();
		await expect(machine.getByTestId("secret-var")).toContainText(
			"tamanu_caddy_tls_dns_iam",
		);
	});

	// A list is entered one item at a time, and a list of objects takes a row
	// of fields for each item.
	// spec: ANSVAR#presentation
	test("enters a list an item at a time", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "kamaka", tags: {} });
		await seedServer(sql, {
			name: "kamaka-prod-central",
			type: "tamanu-central",
			rank: "production",
			groupId: group.id,
			tags: {},
		});
		await seedAnsibleCatalogue(sql);

		await page.goto(`/fleet/groups/${group.id}`);
		const environment = page
			.getByTestId("group-ansible")
			.getByTestId("environment-production")
			.getByTestId("scope-environment");

		await environment.getByTestId("variable-name").fill("tamanu_caddy_extra");
		await page.getByRole("option", { name: /tamanu_caddy_extra_hostnames/ }).click();
		const hostnames = environment.getByTestId("list-field");
		await hostnames.getByLabel("Item").fill("clinic.example.org");
		await hostnames.getByLabel("Item").press("Enter");
		await hostnames.getByLabel("Item").fill("www.clinic.example.org");
		await hostnames.getByLabel("Item").press("Enter");
		await expect(hostnames.getByText("clinic.example.org", { exact: true })).toBeVisible();
		await expect(hostnames.getByText("www.clinic.example.org")).toBeVisible();
		await environment.getByRole("button", { name: "Set", exact: true }).click();
		await expect(environment.getByTestId("var")).toContainText(
			'["clinic.example.org","www.clinic.example.org"]',
		);

		await environment.getByTestId("variable-name").fill("podman_network");
		await page.getByRole("option", { name: /podman_network_subnets_v4/ }).click();
		const subnets = environment.getByTestId("list-field");
		await subnets.getByRole("button", { name: "Add" }).click();
		await subnets.getByLabel("subnet").fill("10.100.0.0/16");
		await subnets.getByLabel("gateway").fill("10.100.0.1");
		await environment.getByRole("button", { name: "Set", exact: true }).click();
		await expect(
			environment.getByTestId("var").filter({ hasText: "podman_network_subnets_v4" }),
		).toContainText('"subnet":"10.100.0.0/16"');
	});

	// The scope's variables as text: one `name: value` a line, saved as the
	// difference.
	// spec: ANSVAR#presentation
	test("edits a scope as text and saves the difference", async ({ page, sql }) => {
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
			.getByTestId("group-ansible")
			.getByTestId("environment-production");
		const scope = production.getByTestId("scope-environment");

		await scope.getByTestId("scope-edit").click();
		const editor = scope.getByTestId("scope-text-editor");
		await editor.locator(".cm-content").fill("log_level: info\nretries: 3\nenabled: yes");
		await editor.getByTestId("scope-save").click();

		await expect(scope.getByTestId("var")).toHaveCount(3);
		await expect(scope.getByTestId("var").filter({ hasText: "retries" })).toContainText("3");
		await expect(scope.getByTestId("var").filter({ hasText: "enabled" })).toContainText("true");
		const stored = () =>
			sql.query<{ name: string; id: string; updated_at: string }>(
				"SELECT name, id::text, updated_at::text FROM ansible_variables ORDER BY name",
			);
		const before = await stored();
		expect(before.map((row) => row.name)).toEqual(["enabled", "log_level", "retries"]);

		// Removing a line removes the variable, and an unchanged one is left
		// as it was: the same row, not set again.
		await scope.getByTestId("scope-edit").click();
		await scope
			.getByTestId("scope-text-editor")
			.locator(".cm-content")
			.fill("enabled: yes\nretries: 3");
		await scope.getByTestId("scope-save").click();
		await expect(scope.getByTestId("var")).toHaveCount(2);
		await expect(scope.getByTestId("var").filter({ hasText: "log_level" })).toHaveCount(0);
		const after = await stored();
		expect(after).toEqual(before.filter((row) => row.name !== "log_level"));
	});

	// An import is offered where nothing carries a variable, and sets what the
	// file says on the machine each host is matched to.
	// spec: ANSIMP#preview
	test("imports an inventory file into an environment with no variables", async ({
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
		await seedServer(sql, {
			name: "kamaka-prod-office",
			type: "tamanu-facility",
			rank: "production",
			groupId: group.id,
			tags: {},
		});

		await page.goto(`/fleet/groups/${group.id}`);
		const production = page
			.getByTestId("group-ansible")
			.getByTestId("environment-production");

		await production.getByTestId("import-inventory").click();
		const dialog = page.getByTestId("import-dialog");
		await dialog.getByTestId("import-file").setInputFiles({
			name: "kamaka.yml",
			mimeType: "text/yaml",
			buffer: Buffer.from(
				[
					"# Connect to the site VPN first",
					"all:",
					"  vars:",
					"    timezone: Pacific/Auckland",
					"    salt: pepper",
					"  hosts:",
					"    kamaka-prod-central:",
					"      public_hostname: central.example.org",
					"    10.20.30.40:223:",
					"      # Behind the office firewall",
					"      tamanu_facility_id: office",
					"",
				].join("\n"),
			),
		});

		// One host has no machine, so nothing can be imported yet.
		const office = dialog.getByTestId("import-host-10.20.30.40:223");
		await expect(office.getByRole("combobox")).toHaveText("Assign or skip");
		await expect(
			dialog.getByTestId("import-host-kamaka-prod-central").getByRole("combobox"),
		).toHaveText("kamaka-prod-central");
		await expect(page.getByTestId("import-apply")).toBeDisabled();
		await expect(dialog.getByTestId("import-var-salt")).toContainText("••••••••");
		await expect(dialog.getByTestId("import-var-salt")).not.toContainText("pepper");
		await expect(dialog.getByTestId("import-notes")).toHaveValue(
			"## production\nConnect to the site VPN first",
		);

		await office.getByLabel("Machine").click();
		await page.getByRole("option", { name: "kamaka-prod-office" }).click();
		await expect(office.getByRole("combobox")).toHaveText("kamaka-prod-office");
		// The comment within the host's entry goes on its machine's notes.
		const officeNotes = dialog.getByTestId("import-host-notes-10.20.30.40:223");
		await expect(officeNotes).toHaveValue("Behind the office firewall");
		await expect(dialog.getByLabel("Added to the notes of kamaka-prod-office")).toBeVisible();
		await officeNotes.fill("Behind the office firewall, ask the site lead");
		await expect(page.getByTestId("import-apply")).toBeEnabled();
		await page.getByTestId("import-apply").click();

		await expect(page.getByTestId("import-dialog")).toHaveCount(0);
		const [noted] = await sql.query<{ notes: string }>(
			"SELECT notes FROM machines WHERE name = 'kamaka-prod-office'",
		);
		expect(noted.notes).toBe("Behind the office firewall, ask the site lead");
		const scopes = production.getByTestId("ansible-variables");
		await expect(scopes.getByTestId("var").filter({ hasText: "timezone" })).toHaveCount(1);
		await expect(scopes.getByTestId("secret-var").filter({ hasText: "salt" })).toHaveCount(1);
		await expect(
			scopes.getByTestId("var").filter({ hasText: "tamanu_facility_id" }),
		).toContainText('"office"');
		// It is no longer offered once the environment carries variables.
		await expect(production.getByTestId("import-inventory")).toHaveCount(0);
	});

	// Someone else's maintenance holds the environment, so a run cannot start
	// here, and the section says whose it is.
	// spec: ANSRUN#presentation
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
		await seedAnsibleCatalogue(sql);
		await seedMaintenanceWindow(sql, {
			serverGroupId: group.id,
			declaredBy: "someone@else.invalid",
			note: "upgrading to v2.55",
		});

		await page.goto(`/fleet/groups/${group.id}`);
		const production = page
			.getByTestId("group-ansible")
			.getByTestId("environment-production");

		await expect(production.getByTestId("run-leased")).toContainText(
			"someone@else.invalid",
		);
		await expect(production.getByTestId("run-any")).toBeDisabled();
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
			reported: true,
		});
		await seedAnsibleCatalogue(sql);
		await seedMaintenanceWindow(sql, {
			machineId: central.machineId,
			declaredBy: "someone@else.invalid",
			note: "replacing a disk",
		});

		await page.goto(`/fleet/groups/${group.id}`);
		const production = page
			.getByTestId("group-ansible")
			.getByTestId("environment-production");

		await expect(production.getByTestId("run-leased")).toContainText(
			"someone@else.invalid",
		);
		await expect(production.getByTestId("run-any")).toBeDisabled();
	});

	// A run needs the catalogue, which names the playbooks.
	// spec: ANSRUN#presentation
	test("cannot start a run until a catalogue has been read", async ({ page, sql }) => {
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
			.getByTestId("group-ansible")
			.getByTestId("environment-production");
		await expect(production.getByTestId("run-needs-catalogue")).toBeVisible();
		await expect(production.getByTestId("run-any")).toBeDisabled();
	});

	// Install and upgrade are Tamanu's, so an environment with no Tamanu
	// application is offered Run… alone.
	// spec: ANSRUN#presentation
	test("offers install and upgrade only where a Tamanu application is held", async ({
		page,
		sql,
	}) => {
		const tamanu = await seedServerGroup(sql, { name: "kamaka", tags: {} });
		await seedServer(sql, {
			name: "kamaka-prod-central",
			type: "tamanu-central",
			rank: "production",
			groupId: tamanu.id,
			tags: {},
		});
		const other = await seedServerGroup(sql, { name: "other", tags: {} });
		await seedServer(sql, {
			name: "other-prod-canopy",
			type: "canopy",
			rank: "production",
			groupId: other.id,
			tags: {},
		});
		await seedAnsibleCatalogue(sql);

		await page.goto(`/fleet/groups/${tamanu.id}`);
		const production = page
			.getByTestId("group-ansible")
			.getByTestId("environment-production");
		await expect(production.getByTestId("run-install")).toBeEnabled();
		await expect(production.getByTestId("run-upgrade")).toBeEnabled();

		await page.goto(`/fleet/groups/${other.id}`);
		const otherProduction = page
			.getByTestId("group-ansible")
			.getByTestId("environment-production");
		await expect(otherProduction.getByTestId("run-any")).toBeEnabled();
		await expect(otherProduction.getByTestId("run-install")).toHaveCount(0);
		await expect(otherProduction.getByTestId("run-upgrade")).toHaveCount(0);
	});

	// Starting work takes the lease and shows the command that carries it, with
	// the checkout left out of what is copied, then waits for the run.
	// spec: ANSRUN#starting-a-run
	test("starts work, shows the command, and gives the lease up", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka", tags: {} });
		const central = await seedServer(sql, {
			name: "kamaka-prod-central",
			host: "https://central.kamaka.e2e.invalid",
			type: "tamanu-central",
			rank: "production",
			groupId: group.id,
			tags: {},
			reported: true,
		});
		await seedAnsibleCatalogue(sql);
		await seedAnsibleVariable(sql, {
			machineId: central.machineId,
			name: "managed_by_ansible",
			value: true,
		});

		await page.goto(`/fleet/groups/${group.id}`);
		const production = page
			.getByTestId("group-ansible")
			.getByTestId("environment-production");

		await production.getByTestId("run-install").click();
		const dialog = page.getByTestId("run-dialog");
		await expect(dialog.getByTestId("run-limit-matches")).toContainText(
			"Matches central.kamaka.e2e.invalid",
		);
		// The command is not shown until the lease is taken, since it carries it.
		await expect(dialog.getByTestId("run-command")).toHaveCount(0);
		await page.getByTestId("run-start").click();

		const copied = dialog.getByTestId("run-command-copied");
		await expect(copied).toContainText(/CANOPY_LEASE=[0-9a-f-]{36}/);
		await expect(dialog.getByTestId("run-command-checkout")).toBeVisible();
		await expect(dialog.getByTestId("run-waiting")).toBeVisible();
		await expect(dialog.getByTestId("run-clock")).toContainText(/[0-5]:[0-5][0-9]/);

		// Copying carries the command as shown and not the checkout line.
		await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
		await dialog.getByRole("button", { name: "Copy the command" }).click();
		const text = await page.evaluate(() => navigator.clipboard.readText());
		expect(text).toBe(await copied.textContent());

		const open = await sql.query<{ playbook: string; started_at: string | null }>(
			"SELECT playbook, started_at FROM ansible_leases WHERE released_at IS NULL",
		);
		expect(open).toHaveLength(1);
		expect(open[0]?.playbook).toBe("tamanu-install.yml");
		expect(open[0]?.started_at).toBeNull();

		await page.getByTestId("run-give-up").click();
		await expect(page.getByTestId("run-dialog")).toHaveCount(0);
		await expect
			.poll(async () => {
				const rows = await sql.query<{ n: string }>(
					"SELECT COUNT(*)::text AS n FROM ansible_leases WHERE released_at IS NULL",
				);
				return rows[0]?.n;
			})
			.toBe("0");
	});

	// The run may read the lease between the last poll and the click, in which
	// case it is under way and giving up would end the maintenance it declared.
	// spec: ANSRUN#starting-a-run
	test("does not give up a lease whose run has started", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "kamaka", tags: {} });
		const central = await seedServer(sql, {
			name: "kamaka-prod-central",
			type: "tamanu-central",
			rank: "production",
			groupId: group.id,
			tags: {},
			reported: true,
		});
		await seedAnsibleCatalogue(sql);
		await seedAnsibleVariable(sql, {
			machineId: central.machineId,
			name: "managed_by_ansible",
			value: true,
		});

		// Run-state reads are held back from the moment the dialog is waiting, so
		// the dialog learns the run has started only after Give up is clicked.
		let holding = false;
		let heldBack!: () => void;
		const polled = new Promise<void>((resolve) => {
			heldBack = resolve;
		});
		let release!: () => void;
		const gate = new Promise<void>((resolve) => {
			release = resolve;
		});
		await page.route("**/api/ansible/run_state", async (route) => {
			if (holding) {
				heldBack();
				await gate;
			}
			await route.continue();
		});

		await page.goto(`/fleet/groups/${group.id}`);
		await page
			.getByTestId("group-ansible")
			.getByTestId("environment-production")
			.getByTestId("run-install")
			.click();
		await page.getByTestId("run-start").click();
		await expect(page.getByTestId("run-waiting")).toBeVisible();
		holding = true;
		await polled;

		const [lease] = await sql.query<{ id: string }>(
			"SELECT id FROM ansible_leases WHERE released_at IS NULL",
		);
		const read = await page.request.post("/api/ansible/for_group", {
			data: { lease_id: lease?.id },
		});
		expect(read.ok()).toBe(true);
		await page.getByTestId("run-give-up").click();
		release();

		await expect(page.getByTestId("run-started")).toBeVisible();
		const open = await sql.query<{ n: string }>(
			"SELECT COUNT(*)::text AS n FROM ansible_leases WHERE released_at IS NULL",
		);
		expect(open[0]?.n).toBe("1");
	});

	// The run reading its environment starts it: the dialog says so, and
	// maintenance is declared for as long as the lease holds.
	// spec: ANSRUN#work-under-way
	test("shows the run started once it has read the environment", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka", tags: {} });
		const central = await seedServer(sql, {
			name: "kamaka-prod-central",
			host: "https://central.kamaka.e2e.invalid",
			type: "tamanu-central",
			rank: "production",
			groupId: group.id,
			tags: {},
			reported: true,
		});
		await seedAnsibleCatalogue(sql);
		await seedAnsibleVariable(sql, {
			machineId: central.machineId,
			name: "managed_by_ansible",
			value: true,
		});

		await page.goto(`/fleet/groups/${group.id}`);
		const production = page
			.getByTestId("group-ansible")
			.getByTestId("environment-production");
		await production.getByTestId("run-install").click();
		await page.getByTestId("run-start").click();
		await expect(page.getByTestId("run-waiting")).toBeVisible();

		// What the plugin does with the lease it was handed.
		const [lease] = await sql.query<{ id: string }>(
			"SELECT id FROM ansible_leases WHERE released_at IS NULL",
		);
		const read = await page.request.post("/api/ansible/for_group", {
			data: { lease_id: lease?.id },
		});
		expect(read.ok()).toBe(true);

		const dialog = page.getByTestId("run-dialog");
		await expect(dialog.getByTestId("run-started")).toBeVisible({ timeout: 15_000 });
		await expect(dialog.getByTestId("run-started")).toContainText("Maintenance");

		// The hold control is offered only for the window the run declared, which
		// the run state names; holding it gives the maintenance an end of its own.
		await page.getByTestId("run-hold").click();
		await expect(dialog.getByTestId("run-hold-confirm")).toBeEnabled();
		await dialog.getByTestId("run-hold-confirm").click();
		await expect(dialog.getByTestId("run-held")).toBeVisible();
	});
});
