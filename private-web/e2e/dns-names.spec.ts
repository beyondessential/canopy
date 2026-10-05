import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedDeniedDnsName,
	seedServer,
	seedServerGroup,
	seedServerGroupDomain,
	seedUndeclaredDnsName,
} from "./seed";

// Declaring DNS names, the requests that resolved to no application on a box,
// denying them, and the notices that say a declaration is wanted (CRT).

test.describe("DNS names", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	/** A group claiming fiji.tamanu.app, and a box in it running a Tamanu central
	 * and a SENAITE lab, both allowed certificates. */
	async function sharedBox(sql: Parameters<typeof seedServer>[0]) {
		const group = await seedServerGroup(sql, { name: "fiji" });
		await seedServerGroupDomain(sql, {
			groupId: group.id,
			domain: "fiji.tamanu.app",
		});
		const central = await seedServer(sql, {
			name: "central",
			groupId: group.id,
			mayManageTls: true,
		});
		const lab = await seedServer(sql, {
			name: "lab",
			type: "senaite",
			groupId: group.id,
			mayManageTls: true,
			machineId: central.machineId,
		});
		return { group, central, lab, machineId: central.machineId };
	}

	test("an operator declares and releases a DNS name on an application", async ({
		page,
		sql,
	}) => {
		const { central } = await sharedBox(sql);
		await page.goto(`/fleet/applications/${central.id}`);

		const field = page.getByLabel("DNS name to declare");
		await field.fill("site.fiji.tamanu.app");
		await page.getByRole("button", { name: "Declare", exact: true }).click();

		const row = page
			.getByTestId("dns-name-row")
			.filter({ hasText: "site.fiji.tamanu.app" });
		await expect(row.getByText("declared", { exact: true })).toBeVisible();
		await expect(row.getByText("no addresses registered")).toBeVisible();
		await expect(row.getByText("outside the group's domains")).toHaveCount(0);

		// Outside every domain the group controls: allowed, and flagged.
		await field.fill("site.samoa.tamanu.app");
		await page.getByRole("button", { name: "Declare", exact: true }).click();
		await expect(
			page
				.getByTestId("dns-name-row")
				.filter({ hasText: "site.samoa.tamanu.app" })
				.getByText("outside the group's domains"),
		).toBeVisible();

		page.once("dialog", (dialog) => dialog.accept());
		await row.getByRole("button", { name: "Release" }).click();
		await expect(
			page.getByTestId("dns-name-row").filter({ hasText: "site.fiji.tamanu.app" }),
		).toHaveCount(0);
	});

	test("an undeclared request is declared on one of the box's applications", async ({
		page,
		sql,
	}) => {
		const { machineId } = await sharedBox(sql);
		await seedUndeclaredDnsName(sql, {
			machineId,
			name: "lab.fiji.tamanu.app",
		});

		await page.goto(`/fleet/machines/${machineId}`);
		const section = page.getByTestId("machine-dns-names");
		const row = section.getByTestId("undeclared-row");
		await expect(row.getByText("lab.fiji.tamanu.app")).toBeVisible();
		await expect(row.getByText(/certificate/i)).toBeVisible();

		await row.getByLabel("Application to declare it on").click();
		await page.getByRole("option", { name: "lab" }).click();
		await row.getByRole("button", { name: "Declare" }).click();

		await expect(section.getByTestId("undeclared-row")).toHaveCount(0);
		const declared = section.getByRole("row", { name: /lab\.fiji\.tamanu\.app/ });
		await expect(declared.getByRole("cell", { name: "lab", exact: true })).toBeVisible();
	});

	test("an undeclared request is denied with a note, and the denial lifted", async ({
		page,
		sql,
	}) => {
		const { machineId } = await sharedBox(sql);
		await seedUndeclaredDnsName(sql, {
			machineId,
			name: "old.fiji.tamanu.app",
		});

		await page.goto(`/fleet/machines/${machineId}`);
		const section = page.getByTestId("machine-dns-names");
		await section
			.getByTestId("undeclared-row")
			.getByRole("button", { name: "Deny" })
			.click();
		await page.getByLabel("Note").fill("site retired");
		await page.getByRole("dialog").getByRole("button", { name: "Deny" }).click();

		await expect(section.getByTestId("undeclared-row")).toHaveCount(0);
		const denied = section.getByTestId("denied-row");
		await expect(denied.getByText("old.fiji.tamanu.app")).toBeVisible();
		await expect(denied.getByText(/site retired/)).toBeVisible();

		await denied.getByRole("button", { name: "Lift" }).click();
		// Nothing left to show, so the section goes altogether.
		await expect(page.getByTestId("machine-dns-names")).toHaveCount(0);
	});

	test("a box that never asked about a DNS name shows no section", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "quiet" });
		const only = await seedServer(sql, { name: "solo", groupId: group.id });
		await page.goto(`/fleet/machines/${only.machineId}`);
		await expect(page.getByTestId("applications-on-box")).toBeVisible();
		await expect(page.getByTestId("machine-dns-names")).toHaveCount(0);
	});

	test("undeclared requests raise a notice on the group and on Status", async ({
		page,
		sql,
	}) => {
		const { group, machineId } = await sharedBox(sql);
		await seedUndeclaredDnsName(sql, { machineId, name: "a.fiji.tamanu.app" });
		await seedUndeclaredDnsName(sql, { machineId, name: "b.fiji.tamanu.app" });
		// Not asked about for over a day, so it no longer counts.
		await seedUndeclaredDnsName(sql, {
			machineId,
			name: "stale.fiji.tamanu.app",
			askedMinutesAgo: 25 * 60,
		});
		// A denial raises nothing.
		await seedDeniedDnsName(sql, { machineId, name: "c.fiji.tamanu.app" });

		await page.goto(`/fleet/groups/${group.id}`);
		const groupNotice = page.getByTestId("undeclared-dns-names-notice");
		await expect(groupNotice).toContainText("2 DNS names waiting on a declaration");
		await groupNotice.getByRole("link", { name: "central" }).click();
		await expect(page).toHaveURL(new RegExp(`/fleet/machines/${machineId}$`));

		await page.goto("/status");
		const statusNotice = page.getByTestId("undeclared-dns-names-notice");
		await expect(statusNotice).toContainText("2 DNS names waiting on a declaration");
		await expect(statusNotice.getByRole("link", { name: "fiji" })).toBeVisible();
	});

	test("an operator who is not an admin sees the requests but no controls", async ({
		page,
		sql,
	}) => {
		const { machineId } = await sharedBox(sql);
		await seedUndeclaredDnsName(sql, { machineId, name: "lab.fiji.tamanu.app" });
		await seedDeniedDnsName(sql, { machineId, name: "old.fiji.tamanu.app" });
		await page.route("**/api/commons/is_current_user_admin", (route) =>
			route.fulfill({ json: false }),
		);

		await page.goto(`/fleet/machines/${machineId}`);
		const section = page.getByTestId("machine-dns-names");
		await expect(section.getByText("lab.fiji.tamanu.app")).toBeVisible();
		await expect(section.getByText("old.fiji.tamanu.app")).toBeVisible();
		await expect(section.getByRole("button")).toHaveCount(0);
	});

	test("with nothing waiting there is no notice", async ({ page, sql }) => {
		const { group, machineId } = await sharedBox(sql);
		await seedDeniedDnsName(sql, { machineId, name: "c.fiji.tamanu.app" });

		await page.goto(`/fleet/groups/${group.id}`);
		await expect(page.getByRole("heading", { level: 1, name: "fiji" })).toBeVisible();
		await expect(page.getByTestId("undeclared-dns-names-notice")).toHaveCount(0);
	});
});
