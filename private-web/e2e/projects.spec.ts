// The fleet's projects view. spec: PRJ

import type { Locator, Page } from "@playwright/test";
import { sidebar, sidebarLink } from "./nav";
import {
	DEV_LOGIN,
	resetSeededTables,
	seedCluster,
	seedDevice,
	seedMachine,
	seedServer,
	seedServerGroup,
	seedTailscaleUser,
} from "./seed";
import { expect, test } from "./test-fixtures";

const section = (page: Page, name: string) =>
	page.getByRole("region", { name, exact: true });
const card = (within: Page | Locator, name: string) =>
	within.getByRole("article", { name, exact: true });
const sectionHeadings = (page: Page) =>
	page.locator("section").getByRole("heading", { level: 2 });
const tamanuRows = (page: Page, group: string) =>
	page.getByRole("table", { name: `${group} Tamanu` }).getByRole("row");
const search = (page: Page) =>
	page.getByRole("textbox", { name: "Search projects" });

/** A group with a production central on a box of its own. */
async function seedProject(
	sql: Parameters<typeof seedServerGroup>[0],
	name: string,
	details: Parameters<typeof seedServerGroup>[1] = {},
) {
	const group = await seedServerGroup(sql, { name, ...details });
	await seedServer(sql, { groupId: group.id, rank: "production", name: null });
	return group;
}

test.describe("projects view", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test('"Projects" sits under Fleet in the sidebar and opens /fleet/pm', async ({
		page,
		sql,
	}) => {
		await seedProject(sql, "Sidebar Isles");
		await page.goto("/fleet");
		await expect(
			sidebar(page).getByRole("group", { name: "Fleet pages" }),
		).toContainText("Projects");
		await sidebarLink(page, "Projects").click();
		await expect(page).toHaveURL(/\/fleet\/pm$/);
		await expect(card(page, "Sidebar Isles")).toBeVisible();
	});

	test("the viewer's groups come first in their own highlighted section, matched on login ignoring case", async ({
		page,
		sql,
	}) => {
		await seedProject(sql, "Alpha Atoll", { pmInCharge: "someone@example.com" });
		const mine = await seedProject(sql, "Zulu Province", {
			pmInCharge: DEV_LOGIN.toUpperCase(),
		});
		await page.goto("/fleet/pm");

		const yours = section(page, "Yours");
		const others = section(page, "Everyone else's");
		await expect(yours.getByRole("article")).toHaveCount(1);
		await expect(card(yours, "Zulu Province")).toBeVisible();
		await expect(others.getByRole("article")).toHaveCount(1);
		await expect(card(others, "Alpha Atoll")).toBeVisible();

		// Yours leads, ahead of a group whose name sorts before it.
		await expect(sectionHeadings(page)).toHaveText(["Yours", "Everyone else's"]);

		await expect(card(page, "Zulu Province")).toHaveCSS("border-left-width", "3px");
		await expect(card(page, "Alpha Atoll")).not.toHaveCSS("border-left-width", "3px");

		await card(page, "Zulu Province")
			.getByRole("link", { name: "Zulu Province" })
			.click();
		await expect(page).toHaveURL(new RegExp(`/fleet/groups/${mine.id}$`));
	});

	test("a viewer in charge of nothing sees one section, without an empty yours heading", async ({
		page,
		sql,
	}) => {
		await seedProject(sql, "Bravo Bay", { pmInCharge: "someone@example.com" });
		await seedProject(sql, "Charlie Coast");
		await page.goto("/fleet/pm");

		await expect(page.getByRole("article")).toHaveCount(2);
		await expect(page.getByRole("article").nth(0)).toHaveAccessibleName("Bravo Bay");
		await expect(page.getByRole("heading", { name: "Yours" })).toHaveCount(0);
		await expect(sectionHeadings(page)).toHaveCount(0);
	});

	test("each group shows its production central, then facilities by name, with links opening in a new tab", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, {
			name: "Delta District",
			project: "District hospital rollout",
			groundContact: "Ana, IT lead",
		});
		const centralBox = await seedDevice(sql, {
			tailscaleNodeName: "delta-central.tail0000.ts.net",
		});
		await seedServer(sql, {
			groupId: group.id,
			name: null,
			type: "tamanu-facility",
			host: null,
		});
		await seedServer(sql, {
			groupId: group.id,
			name: "Zebra Clinic",
			type: "tamanu-facility",
			rank: "production",
			host: "https://zebra.example.org",
		});
		await seedServer(sql, {
			groupId: group.id,
			name: null,
			type: "tamanu-central",
			rank: "production",
			host: "https://central.delta.example.org",
			deviceId: centralBox.id,
		});
		await seedServer(sql, {
			groupId: group.id,
			name: "Alpha Hospital",
			type: "tamanu-facility",
			rank: "production",
			host: "https://alpha.example.org",
		});
		await page.goto("/fleet/pm");

		const delta = card(page, "Delta District");
		await expect(delta).toContainText("District hospital rollout");
		await expect(delta).toContainText("Ana, IT lead");

		const rows = tamanuRows(page, "Delta District");
		// The header, then the central, then facilities by the name each presents.
		await expect(rows).toHaveCount(5);
		await expect(rows.nth(1)).toContainText("Tamanu central");
		await expect(rows.nth(2)).toContainText("Alpha Hospital");
		await expect(rows.nth(3)).toContainText("Tamanu facility");
		await expect(rows.nth(4)).toContainText("Zebra Clinic");

		const publicLink = rows.nth(1).getByRole("link", { name: /central\.delta\.example\.org/ });
		await expect(publicLink).toHaveAttribute("href", /^https:\/\/central\.delta\.example\.org\/?$/);
		await expect(publicLink).toHaveAttribute("target", "_blank");
		const tailnetLink = rows.nth(1).getByRole("link", { name: "delta-central.tail0000.ts.net" });
		await expect(tailnetLink).toHaveAttribute("href", "https://delta-central.tail0000.ts.net");
		await expect(tailnetLink).toHaveAttribute("target", "_blank");
	});

	test("two facilities sharing a name are told apart by their machine", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "Echo Islands" });
		for (const box of ["north-box", "south-box"]) {
			const machine = await seedMachine(sql, {
				name: box,
				groupId: group.id,
				rank: "production",
			});
			await seedServer(sql, {
				groupId: group.id,
				machineId: machine.id,
				name: "Island Clinic",
				type: "tamanu-facility",
			});
		}
		await page.goto("/fleet/pm");

		const rows = tamanuRows(page, "Echo Islands");
		await expect(rows).toHaveCount(3);
		await expect(rows.nth(1)).toContainText("north-box");
		await expect(rows.nth(1)).toContainText("Island Clinic");
		await expect(rows.nth(2)).toContainText("south-box");
		await expect(rows.nth(2)).toContainText("Island Clinic");
	});

	test("a group whose production environment holds no Tamanu shows its details and reads as having none", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, {
			name: "Foxtrot Valley",
			project: "Lab pilot",
		});
		await seedMachine(sql, { groupId: group.id, rank: "production" });
		await page.goto("/fleet/pm");

		const foxtrot = card(page, "Foxtrot Valley");
		await expect(foxtrot).toContainText("Lab pilot");
		await expect(foxtrot).toContainText("No Tamanu in production.");
		await expect(foxtrot.getByRole("table")).toHaveCount(0);
	});

	test("a missing public address or tailnet name, or a cluster-hosted application, shows none", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "Golf Gulf" });
		const cluster = await seedCluster(sql, { name: "ops-projects" });
		await sql.query(
			`INSERT INTO applications (type, name, rank, group_id, kubernetes_cluster_id, host)
			 VALUES ('tamanu-central', NULL, 'production', $1, $2, 'https://golf.example.org')`,
			[group.id, cluster.id],
		);
		await seedServer(sql, {
			groupId: group.id,
			name: "Quiet Post",
			type: "tamanu-facility",
			rank: "production",
			host: null,
		});
		await page.goto("/fleet/pm");

		const rows = tamanuRows(page, "Golf Gulf");
		await expect(rows).toHaveCount(3);
		// The cluster-hosted central: a public address, and no tailnet one.
		await expect(rows.nth(1).getByRole("cell").nth(1)).toContainText("golf.example.org");
		await expect(rows.nth(1).getByRole("cell").nth(2)).toHaveText("None");
		// A facility on a machine with no tailnet name, configured with no URL.
		await expect(rows.nth(2).getByRole("cell").nth(1)).toHaveText("None");
		await expect(rows.nth(2).getByRole("cell").nth(2)).toHaveText("None");
		await expect(rows.nth(2).getByRole("link", { name: /example|ts\.net/ })).toHaveCount(0);
	});

	test("search narrows by group name, project, contact, and PM by display name or login, ignoring case", async ({
		page,
		sql,
	}) => {
		await seedTailscaleUser(sql, { login: "sam@example.org", name: "Sam Lee" });
		await seedProject(sql, "Hotel Heights", { pmInCharge: "sam@example.org" });
		await seedProject(sql, "India Inlet", { project: "Vaccine rollout" });
		await seedProject(sql, "Juliet Junction", { groundContact: "Mele, ministry liaison" });
		await page.goto("/fleet/pm");
		await expect(page.getByRole("article")).toHaveCount(3);

		const narrowsTo = async (term: string, group: string) => {
			await search(page).fill(term);
			await expect(page.getByRole("article")).toHaveCount(1);
			await expect(card(page, group)).toBeVisible();
		};
		await narrowsTo("hotel HEIGHTS", "Hotel Heights");
		await narrowsTo("VACCINE", "India Inlet");
		await narrowsTo("ministry", "Juliet Junction");
		await narrowsTo("sam lee", "Hotel Heights");
		await narrowsTo("SAM@EXAMPLE", "Hotel Heights");

		await search(page).fill("nothing like it");
		await expect(page.getByRole("article")).toHaveCount(0);
		await search(page).fill("");
		await expect(page.getByRole("article")).toHaveCount(3);
	});
});
