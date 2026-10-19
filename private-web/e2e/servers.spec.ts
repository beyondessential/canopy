import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedApplicationReport,
	seedDevice,
	seedMachine,
	seedServer,
	seedServerGroup,
	seedVersion,
} from "./seed";
import { sidebar, sidebarLink } from "./nav";

test.describe("servers list page", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("renders the fleet tabs and the seeded group row", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "cluster-uno" });
		await seedServer(sql, {
			name: "in-group",
			type: "tamanu-central",
			groupId: group.id,
		});

		await page.goto("/fleet");

		await expect(sidebarLink(page, "Groups")).toHaveAttribute(
			"aria-current",
			"page",
		);
		await expect(sidebarLink(page, "Archived")).toBeVisible();
		await expect(sidebarLink(page, "Figures")).toBeVisible();

		// The fleet is browsed by group, and every machine is in one, so there
		// is no ungrouped listing to offer.
		// spec: FLT
		await expect(sidebarLink(page, "Ungrouped")).toHaveCount(0);
		await expect(
			sidebar(page).getByRole("group", { name: "Fleet pages" }).getByRole("link"),
		).toHaveCount(3);

		// The group's name shows as a link to its detail page.
		await expect(
			page.getByRole("link", { name: group.name }),
		).toHaveAttribute("href", `/fleet/groups/${group.id}`);
	});
});

test.describe("server detail page", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("renders the seeded server's name and host", async ({ page, sql }) => {
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const group = await seedServerGroup(sql, { name: "host-group" });
		const server = await seedServer(sql, {
			name: "detail-target",
			type: "tamanu-central",
			groupId: group.id,
		});

		await page.goto(`/fleet/applications/${server.id}`);

		// The page renders two h1s (app bar "Canopy" + page heading);
		// scope to the heading whose accessible name includes the server
		// name, which the group prefix becomes part of.
		await expect(
			page.getByRole("heading", {
				level: 1,
				name: new RegExp(server.name),
			}),
		).toBeVisible();
		const hostLink = page.getByRole("link", { name: new RegExp(server.host) });
		await expect(hostLink).toBeVisible();

		// The server's Group section shows the group's effective billing labels.
		await expect(page.getByText("Billing labels")).toBeVisible();
		await expect(page.getByText("billing.product=tamanu")).toBeVisible();
		await expect(page.getByText("billing.deployment=host-group")).toBeVisible();
	});

	test("nonexistent UUID surfaces an error alert", async ({ page }) => {
		await page.goto("/fleet/applications/00000000-0000-0000-0000-000000000000");
		await expect(page.getByRole("alert")).toBeVisible();
	});

	/// The page moved under `/fleet` because `servers` named the box and the
	/// workload at once, and the fleet's listings sat beside its records. A link
	/// into Canopy outlives a rename — a bookmark, a Slack message, an incident
	/// writeup — so the old address still lands.
	///
	/// spec: FLT#navigating-the-two-grains
	test("the old /servers address lands on the application", async ({
		page,
		sql,
	}) => {
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const server = await seedServer(sql, {
			name: "moved-target",
			type: "tamanu-central",
		});

		await page.goto(`/servers/${server.id}`);

		await expect(page).toHaveURL(new RegExp(`/fleet/applications/${server.id}$`));
		await expect(
			page.getByRole("heading", {
				level: 1,
				name: new RegExp(server.name),
			}),
		).toBeVisible();
	});

	/// Two hops: the old address names the application, and the application's
	/// form is its machine's. An operator following an old bookmark lands on
	/// the form that edits the thing they meant, wherever that form now lives.
	test("the old /servers edit address lands on the form that edits it", async ({
		page,
		sql,
	}) => {
		const server = await seedServer(sql, { name: "moved-edit-target" });

		await page.goto(`/servers/${server.id}/edit`);

		await expect(page).toHaveURL(
			new RegExp(`/fleet/machines/${server.machineId}/edit$`),
		);
		await expect(
			page.getByTestId("application-section").getByLabel(/^Name(\s*\*)?$/i),
		).toHaveValue(server.name);
	});

	/// The fleet's listings moved with its records, so the old addresses for
	/// those land too — and land on the listing rather than being read as an
	/// application id, which is what route ranking used to have to prevent.
	test("the old fleet listing addresses land on the listings", async ({
		page,
	}) => {
		await page.goto("/servers/figures");
		await expect(page).toHaveURL(/\/fleet\/figures$/);

		await page.goto("/servers/archived");
		await expect(page).toHaveURL(/\/fleet\/archived$/);

		await page.goto("/servers");
		await expect(page).toHaveURL(/\/fleet$/);
	});

	/// A group is the fleet's too, so its pages moved with the rest and their
	/// old addresses land the same way.
	test("the old group addresses land under the fleet", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "moved-group" });

		await page.goto(`/groups/${group.id}`);
		await expect(page).toHaveURL(new RegExp(`/fleet/groups/${group.id}$`));
		await expect(
			page.getByRole("heading", { name: group.name }),
		).toBeVisible();

		await page.goto(`/groups/${group.id}/edit`);
		await expect(page).toHaveURL(new RegExp(`/fleet/groups/${group.id}/edit$`));

		await page.goto(`/groups/${group.id}/machines/new`);
		await expect(page).toHaveURL(
			new RegExp(`/fleet/groups/${group.id}/machines/new$`),
		);

		await page.goto("/groups/new");
		await expect(page).toHaveURL(/\/fleet\/groups\/new$/);
	});
});

test.describe("the machine's edit form", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	/// One form per machine, holding the box's section and one section per
	/// application on it. A machine fact has one place to be edited, and a
	/// shared box is edited where everything sharing it is visible.
	///
	/// spec: FLT#groups
	test("editing an application hands over to its machine's form", async ({
		page,
		sql,
	}) => {
		const server = await seedServer(sql, {
			name: "edit-target",
			type: "tamanu-central",
		});

		await page.goto(`/fleet/applications/${server.id}/edit`);

		await expect(page).toHaveURL(
			new RegExp(`/fleet/machines/${server.machineId}/edit$`),
		);
		// The application is a section of that form, pre-filled.
		const section = page.getByTestId("application-section");
		await expect(section.getByLabel(/^Name(\s*\*)?$/i)).toHaveValue(
			server.name,
		);
	});

	test("saving a renamed application persists the change", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "edit-group" });
		const server = await seedServer(sql, {
			name: "edit-save-target",
			groupId: group.id,
		});

		await page.goto(`/fleet/machines/${server.machineId}/edit`);

		await page
			.getByTestId("application-section")
			.getByLabel(/^Name(\s*\*)?$/i)
			.fill("edit-save-renamed");
		await page.getByRole("button", { name: /^save$/i }).click();

		// Save navigates to the box, that being whose form it is.
		await page.waitForURL(`**/fleet/machines/${server.machineId}`);

		const rows = await sql.query<{ name: string }>(
			"SELECT name FROM applications WHERE id = $1",
			[server.id],
		);
		expect(rows[0]!.name).toBe("edit-save-renamed");
	});

	/// A group is the machine's, and the applications on it take it, so it is
	/// offered once — on the box — and not per workload.
	///
	/// spec: FLT#groups
	test("the group is offered on the machine and not on its applications", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "edit-group-once" });
		const server = await seedServer(sql, {
			name: "edit-group-target",
			groupId: group.id,
		});

		await page.goto(`/fleet/machines/${server.machineId}/edit`);

		await expect(page.getByLabel(/^Group/i)).toHaveCount(1);
		await expect(
			page.getByTestId("application-section").getByLabel(/^Group/i),
		).toHaveCount(0);
	});

	/// Renaming the box writes the box, not the workload that happens to share
	/// its name — the two Name fields are different fields.
	test("the machine's own name saves against the machine", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "edit-box-group" });
		const machine = await seedMachine(sql, {
			name: "box-before",
			groupId: group.id,
		});
		await seedServer(sql, {
			name: "on-the-box",
			groupId: group.id,
			machineId: machine.id,
		});

		await page.goto(`/fleet/machines/${machine.id}/edit`);

		// The box's Name is the one outside any application section.
		await page
			.getByTestId("machine-section")
			.getByLabel(/^Name(\s*\*)?$/i)
			.fill("box-after");
		await page.getByRole("button", { name: /^save$/i }).click();
		await page.waitForURL(`**/fleet/machines/${machine.id}`);

		const machines = await sql.query<{ name: string }>(
			"SELECT name FROM machines WHERE id = $1",
			[machine.id],
		);
		expect(machines[0]!.name).toBe("box-after");
		const apps = await sql.query<{ name: string }>(
			"SELECT name FROM applications WHERE machine_id = $1",
			[machine.id],
		);
		expect(apps[0]!.name).toBe("on-the-box");
	});

	// The identity belongs to the box and is bound by enrolment, not by
	// editing a form — so it is offered on neither section.
	// spec: FLT#identities
	test("offers no identity field", async ({ page, sql }) => {
		const device = await seedDevice(sql);
		const server = await seedServer(sql, {
			name: "edit-no-identity",
			deviceId: device.id,
		});

		await page.goto(`/fleet/machines/${server.machineId}/edit`);
		await expect(
			page.getByTestId("application-section").getByLabel(/^Name(\s*\*)?$/i),
		).toHaveValue(server.name);
		await expect(page.getByRole("main").getByLabel(/device/i)).toHaveCount(0);
	});

	/// A box carrying two workloads is the case the one-form rule exists for:
	/// both are edited in one place, so a change to the box is visibly a
	/// change to both.
	///
	/// spec: FLT#groups
	test("a shared box holds a section for each application on it", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "shared-edit-group" });
		const machine = await seedMachine(sql, {
			name: "shared-edit-box",
			groupId: group.id,
		});
		await seedServer(sql, {
			name: "shared-central",
			type: "tamanu-central",
			groupId: group.id,
			machineId: machine.id,
		});
		await seedServer(sql, {
			name: "shared-facility",
			type: "tamanu-facility",
			groupId: group.id,
			machineId: machine.id,
		});

		await page.goto(`/fleet/machines/${machine.id}/edit`);

		await expect(page.getByTestId("application-section")).toHaveCount(2);
		await expect(page.getByText("shared-central")).toBeVisible();
		await expect(page.getByText("shared-facility")).toBeVisible();
	});
});

test.describe("archived view", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("lists archived servers and groups and restores them", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "arch-group" });
		const server = await seedServer(sql, { name: "arch-server", type: "tamanu-central" });
		// Archive both directly (the UI paths are covered elsewhere).
		await sql.query("UPDATE server_groups SET deleted_at = now() WHERE id = $1", [
			group.id,
		]);
		await sql.query("UPDATE applications SET deleted_at = now() WHERE id = $1", [
			server.id,
		]);

		await page.goto("/fleet/archived");

		// Both archived items are discoverable here (and nowhere else).
		await expect(page.getByRole("link", { name: "arch-group" })).toBeVisible();
		await expect(page.getByText(/arch-server/)).toBeVisible();

		// Restore the group (its row renders first, so the first Restore is it).
		await page.getByRole("button", { name: "Restore" }).first().click();
		await expect(
			page.getByRole("link", { name: "arch-group" }),
		).not.toBeVisible();
		// The archived server is still listed.
		await expect(page.getByText(/arch-server/)).toBeVisible();
	});

	test("lists archived machines and restores them", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "arch-box-group" });
		const machine = await seedMachine(sql, {
			name: "arch-box",
			groupId: group.id,
		});
		await sql.query("UPDATE machines SET deleted_at = now() WHERE id = $1", [
			machine.id,
		]);

		await page.goto("/fleet/archived");
		await expect(
			page.getByRole("link", { name: "arch-box-group / arch-box" }),
		).toBeVisible();

		await page.getByRole("button", { name: "Restore" }).click();
		await expect(page.getByText("Nothing archived.")).toBeVisible();

		await page.goto(`/fleet/machines/${machine.id}`);
		await expect(
			page.getByRole("heading", { level: 1, name: /arch-box/ }),
		).toBeVisible();
		await expect(page.getByText("This machine is archived.")).toHaveCount(0);
	});

	test("a machine in an archived group is restored after its group", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "arch-held-group" });
		const machine = await seedMachine(sql, {
			name: "arch-held-box",
			groupId: group.id,
		});
		await sql.query("UPDATE machines SET deleted_at = now() WHERE id = $1", [
			machine.id,
		]);
		await sql.query("UPDATE server_groups SET deleted_at = now() WHERE id = $1", [
			group.id,
		]);

		await page.goto("/fleet/archived");
		const row = page
			.getByRole("link", { name: "arch-held-group / arch-held-box" })
			.locator("..");
		await row.getByRole("button", { name: "Restore" }).click();
		await expect(row.getByText(/restore it before its machines/)).toBeVisible();
	});

	test("an empty group can be archived from its detail page", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "empty-grp" });
		page.on("dialog", (d) => d.accept());

		await page.goto(`/fleet/groups/${group.id}`);
		await page.getByRole("button", { name: "Archive" }).click();
		// Redirects to the servers list, and the group now shows under Archived.
		await expect(page).toHaveURL(/\/fleet$/);
		await page.goto("/fleet/archived");
		await expect(page.getByRole("link", { name: "empty-grp" })).toBeVisible();
	});

	test("a group whose servers are all gone archives, cascading to them", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "gone-grp" });
		// No statuses seeded → both servers are "gone".
		await seedServer(sql, { name: "gone-1", type: "tamanu-central", groupId: group.id });
		await seedServer(sql, { name: "gone-2", type: "tamanu-facility", groupId: group.id });
		page.on("dialog", (d) => d.accept());

		await page.goto(`/fleet/groups/${group.id}`);
		// The Archive button is offered because every member is gone.
		await page.getByRole("button", { name: "Archive" }).click();
		await expect(page).toHaveURL(/\/fleet$/);

		// The group and both servers are now archived.
		await page.goto("/fleet/archived");
		await expect(page.getByRole("link", { name: "gone-grp" })).toBeVisible();
		await expect(page.getByText(/gone-1/)).toBeVisible();
		await expect(page.getByText(/gone-2/)).toBeVisible();
	});

	test("a group whose servers reported long ago still archives", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "quiet-grp" });
		const one = await seedServer(sql, {
			name: "quiet-1",
			type: "tamanu-central",
			groupId: group.id,
		});
		const two = await seedServer(sql, {
			name: "quiet-2",
			type: "tamanu-facility",
			groupId: group.id,
		});
		// Both reported months ago and nothing since, so both are unreachable
		// rather than never heard from. Archiving is still the right call: the
		// button asks whether every member has gone quiet, not what colour its
		// dot is.
		for (const s of [one, two]) {
			await seedApplicationReport(sql, {
				applicationId: s.id,
				reportedAt: "NOW() - INTERVAL '90 days'",
			});
		}
		page.on("dialog", (d) => d.accept());

		await page.goto(`/fleet/groups/${group.id}`);
		await page.getByRole("button", { name: "Archive" }).click();
		await expect(page).toHaveURL(/\/fleet$/);

		await page.goto("/fleet/archived");
		await expect(page.getByRole("link", { name: "quiet-grp" })).toBeVisible();
		await expect(page.getByText(/quiet-1/)).toBeVisible();
		await expect(page.getByText(/quiet-2/)).toBeVisible();
	});
});

test.describe("server create → setup → archive flow", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("creates a server in a group, surfaces its enrollment ticket, then archives it", async ({
		page,
		sql,
	}) => {
		// The detail page's get_detail resolves the latest version, so it needs
		// at least one published version present.
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const group = await seedServerGroup(sql, { name: "flow-group" });

		// Create the box — the in-group route pre-selects the group, so we only
		// set the (required) name. Nothing here names an application: what runs
		// on the box arrives by report.
		await page.goto(`/fleet/groups/${group.id}/machines/new`);
		await page.getByLabel(/^Name(\s*\*)?$/i).fill("flow-machine");
		await page.getByRole("button", { name: "Create machine" }).click();

		// Lands on the new box's own page.
		await expect(page).toHaveURL(/\/machines\/[0-9a-f-]{36}$/);

		// Setup — enrolment admits the box, so an unenrolled machine auto-mints
		// a ticket and shows the bestool register command for the operator to
		// run. No application is involved: nothing runs here until the enrolled
		// agent reports it.
		await expect(page.getByText(/hasn't checked in yet/i)).toBeVisible();
		await expect(page.getByText(/bestool canopy register/)).toBeVisible();
		await expect(
			page.getByRole("heading", { name: "Set up this machine" }),
		).toBeVisible();

		// The workload on it is reported, not entered, so seed one and carry on
		// through its lifecycle.
		const server = await seedServer(sql, {
			name: "flow-server",
			groupId: group.id,
		});
		await page.goto(`/fleet/applications/${server.id}`);
		await expect(
			page.getByRole("heading", { level: 1, name: /flow-server/ }),
		).toBeVisible();

		// An application never mints a ticket. It points at its box instead.
		await expect(page.getByText(/bestool canopy register/)).toHaveCount(0);
		await page.getByRole("link", { name: "Enrol its machine" }).click();
		await expect(page).toHaveURL(/\/machines\/[0-9a-f-]{36}$/);
		await page.goBack();

		// Archive — confirm the dialog; since the server is in a group, it
		// redirects to that group's page (not the servers list).
		await page.getByRole("button", { name: "Archive", exact: true }).click();
		const dialog = page.getByRole("dialog");
		await expect(dialog).toBeVisible();
		await dialog.getByRole("button", { name: "Archive", exact: true }).click();
		await expect(page).toHaveURL(new RegExp(`/fleet/groups/${group.id}$`));
	});
});
