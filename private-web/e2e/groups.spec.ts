import { expect, test } from "./test-fixtures";
import { resetSeededTables, seedServer, seedServerGroup } from "./seed";

test.describe("group detail page", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("renders the group's name, notes, tags and member servers", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, {
			name: "watched-cluster",
			notes: "ops handover note",
			tags: { env: "prod", tier: "1" },
		});
		const memberA = await seedServer(sql, {
			name: "member-a",
			groupId: group.id,
		});
		const memberB = await seedServer(sql, {
			name: "member-b",
			groupId: group.id,
		});

		await page.goto(`/fleet/groups/${group.id}`);

		await expect(
			page.getByRole("heading", { name: group.name, level: 1 }),
		).toBeVisible();
		await expect(page.getByText("ops handover note")).toBeVisible();
		await expect(page.getByText("env=prod")).toBeVisible();
		await expect(page.getByText("tier=1")).toBeVisible();
		// Effective billing labels: product defaults to tamanu, the deployment label is the
		// lower-kebab group name (no explicit billing.* tags on this group).
		await expect(page.getByText("Billing labels")).toBeVisible();
		await expect(page.getByText("billing.product=tamanu")).toBeVisible();
		await expect(
			page.getByText("billing.deployment=watched-cluster"),
		).toBeVisible();
		// Each member sits under its own box in the tree. Matched by href:
		// `seedServer` names the box after the workload, so a name matches both
		// links.
		await expect(
			page.locator(`a[href="/fleet/applications/${memberA.id}"]`),
		).toBeVisible();
		await expect(
			page.locator(`a[href="/fleet/applications/${memberB.id}"]`),
		).toBeVisible();
		await expect(
			page.locator(`a[href="/fleet/machines/${memberA.machineId}"]`),
		).toBeVisible();
	});

	test("an empty group lists no servers", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "lonely-group" });
		await page.goto(`/fleet/groups/${group.id}`);
		await expect(
			page.getByRole("heading", { name: group.name, level: 1 }),
		).toBeVisible();
		await expect(page.getByText(/nothing in this group yet/i)).toBeVisible();
		await expect(page.getByText(/no environment to configure/i)).toBeVisible();
	});
});

test.describe("group edit page", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("pre-fills the form with the group's name, notes and tags", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, {
			name: "editable-group",
			notes: "be careful",
			tags: { region: "au" },
		});

		await page.goto(`/fleet/groups/${group.id}/edit`);

		// Name is marked required so MUI renders the label as "Name *".
		await expect(page.getByLabel(/^Name\b/i)).toHaveValue(group.name);
		await expect(page.getByLabel(/^Notes$/i)).toHaveValue("be careful");
		await expect(page.getByLabel(/^Key$/i)).toHaveValue("region");
		await expect(page.getByLabel(/^Value$/i)).toHaveValue("au");
	});

	test("the tags add-row button appends a blank editable row", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, {
			name: "tagless-group",
			tags: {},
		});

		await page.goto(`/fleet/groups/${group.id}/edit`);

		// Starts empty.
		await expect(page.getByText(/^no tags\.$/i)).toBeVisible();
		await expect(page.getByLabel(/^Key$/i)).toHaveCount(0);

		// One click → one row.
		await page.getByRole("button", { name: "add tag" }).click();
		await expect(page.getByLabel(/^Key$/i)).toHaveCount(1);
		await expect(page.getByLabel(/^Value$/i)).toHaveCount(1);

		// A second click → a second row (the regression: only the second
		// row would survive re-render because the first held an empty key).
		await page.getByRole("button", { name: "add tag" }).click();
		await expect(page.getByLabel(/^Key$/i)).toHaveCount(2);

		// The new rows are editable, and typing into the first doesn't
		// erase the second (the rowId identity is stable).
		await page.getByLabel(/^Key$/i).nth(0).fill("env");
		await page.getByLabel(/^Value$/i).nth(0).fill("prod");
		await expect(page.getByLabel(/^Key$/i)).toHaveCount(2);
	});

	test("editing a tag value and saving persists the new value", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, {
			name: "edit-tag-group",
			tags: { env: "staging" },
		});

		await page.goto(`/fleet/groups/${group.id}/edit`);

		const valueInput = page.getByLabel(/^Value$/i);
		await expect(valueInput).toHaveValue("staging");
		await valueInput.fill("prod");
		await page.getByRole("button", { name: /^save$/i }).click();

		// Save navigates to detail; wait for that.
		await page.waitForURL(`**/fleet/groups/${group.id}`);

		// DB has the new value.
		const rows = await sql.query<{ tags: Record<string, string> }>(
			"SELECT tags FROM server_groups WHERE id = $1",
			[group.id],
		);
		expect(rows[0]!.tags).toEqual({ env: "prod" });
	});

	test("removing a tag row and saving drops it from the DB", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, {
			name: "delete-tag-group",
			tags: { env: "prod", tier: "1" },
		});

		await page.goto(`/fleet/groups/${group.id}/edit`);

		await expect(page.getByLabel(/^Key$/i)).toHaveCount(2);
		// Rows render sorted by key — "env" is first, "tier" is second.
		await page.getByRole("button", { name: "remove tag" }).nth(1).click();
		await expect(page.getByLabel(/^Key$/i)).toHaveCount(1);
		await expect(page.getByLabel(/^Key$/i)).toHaveValue("env");

		await page.getByRole("button", { name: /^save$/i }).click();
		await page.waitForURL(`**/fleet/groups/${group.id}`);

		const rows = await sql.query<{ tags: Record<string, string> }>(
			"SELECT tags FROM server_groups WHERE id = $1",
			[group.id],
		);
		expect(rows[0]!.tags).toEqual({ env: "prod" });
	});

	test("adding a new row and saving writes the tag to the DB", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, {
			name: "add-tag-group",
			tags: { env: "prod" },
		});

		await page.goto(`/fleet/groups/${group.id}/edit`);

		await page.getByRole("button", { name: "add tag" }).click();
		// The new (empty-keyed) row sorts last under the existing "env" row.
		await page.getByLabel(/^Key$/i).nth(1).fill("tier");
		await page.getByLabel(/^Value$/i).nth(1).fill("1");

		await page.getByRole("button", { name: /^save$/i }).click();
		await page.waitForURL(`**/fleet/groups/${group.id}`);

		const rows = await sql.query<{ tags: Record<string, string> }>(
			"SELECT tags FROM server_groups WHERE id = $1",
			[group.id],
		);
		expect(rows[0]!.tags).toEqual({ env: "prod", tier: "1" });
	});

	test("editing the Slack cooldown and linger minutes persists as the right intervals", async ({
		page,
		sql,
	}) => {
		// Seeded at 3 minutes open / 4 minutes linger; UI works in minutes;
		// bumping to 5 and 7 should round-trip through the API as seconds
		// in the DB.
		const group = await seedServerGroup(sql, {
			name: "cooldown-group",
			slackOpenDelaySeconds: 180,
			slackCloseDelaySeconds: 240,
		});

		await page.goto(`/fleet/groups/${group.id}/edit`);

		// Two "minutes" fields: the open cooldown first, the linger second.
		const openInput = page.getByLabel(/^minutes$/i).first();
		const lingerInput = page.getByLabel(/^minutes$/i).nth(1);
		await expect(openInput).toHaveValue("3");
		await expect(lingerInput).toHaveValue("4");
		await openInput.fill("5");
		await lingerInput.fill("7");
		await page.getByRole("button", { name: /^save$/i }).click();
		await page.waitForURL(`**/fleet/groups/${group.id}`);

		const rows = await sql.query<{ open: string; close: string }>(
			"SELECT EXTRACT(EPOCH FROM slack_open_delay)::text AS open, \
			        EXTRACT(EPOCH FROM slack_close_delay)::text AS close \
			 FROM server_groups WHERE id = $1",
			[group.id],
		);
		expect(Number(rows[0]!.open)).toBe(300);
		expect(Number(rows[0]!.close)).toBe(420);
	});
});
