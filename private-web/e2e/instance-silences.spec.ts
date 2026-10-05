import {
	resetSeededTables,
	seedGroupSilencedRef,
	seedIncident,
	seedInstancedCheck,
	seedServer,
	seedServerGroup,
	seedServerSilencedRef,
	type Sql,
} from "./seed";
import { expect, test } from "./test-fixtures";
import type { Page } from "@playwright/test";

const CHECK = "sync_facility_stale";
const REF = `health/${CHECK}`;

/** A central in a group reporting its facilities' sync staleness: Northgate
 * failing, Ridge warning, Eastbay warning but silenced on the application,
 * Southpoint passing, and one facility skipped. */
async function centralWithFacilities(sql: Sql) {
	const group = await seedServerGroup(sql, { name: "instanced-group" });
	const central = await seedServer(sql, {
		name: "central",
		type: "tamanu-central",
		groupId: group.id,
	});
	const issue = await seedInstancedCheck(sql, {
		serverId: central.id,
		check: CHECK,
		detail: { warn_minutes: 10, fail_minutes: 30 },
		instances: {
			"6f1c2a9e-0d4b-4c1e-9a2f-5d8e3b6c7a10": {
				result: "failed",
				label: "Northgate Clinic",
				detail: { minutes_since_success: 2875.4 },
			},
			"0d9a7e3c-81f2-4b6a-a0c5-7e2d19b4f358": {
				result: "warning",
				label: "Ridge Health Centre",
				detail: { minutes_since_success: 17.9 },
			},
			"a41b9c02-5e6f-4d78-b3a1-c0f2e8d93e7d": {
				result: "warning",
				// Its silence graded it skipped when it was set.
				effective: "skipped",
				label: "Eastbay Clinic",
				detail: { minutes_since_success: 4120.0 },
			},
			south: { result: "passed", label: "Southpoint" },
			hillside: { result: "skipped", label: "Hillside" },
		},
	});
	await seedServerSilencedRef(sql, {
		serverId: central.id,
		ref: REF,
		instance: "a41b9c02-5e6f-4d78-b3a1-c0f2e8d93e7d",
		createdBy: "ops@example.org",
	});
	return { group, central, issue };
}

/** The scoped silences stored for the check, by instance key and scope. */
async function instanceSilences(sql: Sql) {
	return sql.query<{
		instance_key: string | null;
		application_id: string | null;
		server_group_id: string | null;
	}>(
		`SELECT instance_key, application_id, server_group_id FROM scoped_check_policies
		 WHERE check_name = $1 AND ceiling = 'skipped' ORDER BY instance_key`,
		[CHECK],
	);
}

function instanceRow(page: Page, name: string) {
	return page.getByTestId("check-instance").filter({ hasText: name });
}

test.describe("Silencing one instance", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	/// A target's check lists its degraded and silenced instances, each by
	/// label and result with its own silence control, and counts the rest.
	///
	/// spec: CHK#silencing-one-instance
	test("the application's checks list instances and count the rest", async ({
		page,
		sql,
	}) => {
		const { central } = await centralWithFacilities(sql);
		await page.goto(`/fleet/applications/${central.id}`);

		const rows = page.getByTestId("check-instance");
		await expect(rows).toHaveCount(3);
		// Most urgent first, the silenced one last.
		await expect(rows.nth(0)).toContainText("Northgate Clinic");
		await expect(rows.nth(1)).toContainText("Ridge Health Centre");
		await expect(rows.nth(2)).toContainText("Eastbay Clinic");

		// Named by label, with the key shortened after it, and its own facts.
		const north = instanceRow(page, "Northgate Clinic");
		await expect(north).toContainText("6f1c2a9e…7a10");
		await expect(north).toContainText("minutes_since_success 2875.4");
		await expect(
			page.getByRole("button", {
				name: `Silence Northgate Clinic in ${CHECK}`,
			}),
		).toBeVisible();

		// The silenced one says where it is silenced.
		await expect(instanceRow(page, "Eastbay Clinic")).toContainText(
			"silenced (application)",
		);
		await expect(
			page.getByRole("button", {
				name: `Manage silence for Eastbay Clinic in ${CHECK}`,
			}),
		).toBeVisible();

		// The passing and skipped ones are counted, not listed.
		await expect(page.getByText("1 passing, 1 skipped")).toBeVisible();
		await expect(rows.filter({ hasText: "Southpoint" })).toHaveCount(0);
		await expect(rows.filter({ hasText: "Hillside" })).toHaveCount(0);

		// The check's shared fields stay on the check.
		await expect(page.getByText("fail_minutes")).toBeVisible();
	});

	/// spec: CHK#silencing-one-instance
	test("an instance is silenced from the checks for the application", async ({
		page,
		sql,
	}) => {
		const { central } = await centralWithFacilities(sql);
		await page.goto(`/fleet/applications/${central.id}`);

		await page
			.getByRole("button", { name: `Silence Northgate Clinic in ${CHECK}` })
			.click();
		await expect(
			page.getByText("The instance still records", { exact: false }),
		).toBeVisible();
		await page.getByRole("button", { name: "For this server" }).click();

		await expect(instanceRow(page, "Northgate Clinic")).toContainText(
			"silenced (application)",
		);
		// The other instances are graded as before.
		await expect(instanceRow(page, "Ridge Health Centre")).not.toContainText(
			"silenced",
		);
		const stored = await instanceSilences(sql);
		expect(
			stored.find(
				(s) => s.instance_key === "6f1c2a9e-0d4b-4c1e-9a2f-5d8e3b6c7a10",
			),
		).toMatchObject({ application_id: central.id, server_group_id: null });
	});

	/// spec: CHK#silencing-one-instance
	test("an instance is silenced from the checks for the group", async ({
		page,
		sql,
	}) => {
		const { group, central } = await centralWithFacilities(sql);
		await page.goto(`/fleet/applications/${central.id}`);

		await page
			.getByRole("button", { name: `Silence Ridge Health Centre in ${CHECK}` })
			.click();
		await page.getByRole("button", { name: "For this group" }).click();

		await expect(instanceRow(page, "Ridge Health Centre")).toContainText(
			"silenced (group)",
		);
		const stored = await instanceSilences(sql);
		expect(
			stored.find(
				(s) => s.instance_key === "0d9a7e3c-81f2-4b6a-a0c5-7e2d19b4f358",
			),
		).toMatchObject({ application_id: null, server_group_id: group.id });
	});

	/// The issue's silence offers the whole check or one of its degraded
	/// instances, from within an incident.
	///
	/// spec: CHK#silencing-one-instance
	test("one instance is silenced from an issue in an incident", async ({
		page,
		sql,
	}) => {
		const { group, central, issue } = await centralWithFacilities(sql);
		const incident = await seedIncident(sql, {
			serverGroupId: group.id,
			issues: [{ issueId: issue.id }],
		});
		await page.goto(`/incidents/${incident.id}`);

		await page.getByRole("button", { name: "Silence ref…" }).click();
		const picker = page.getByRole("combobox", { name: "Silence" });
		await expect(picker).toHaveText("Whole check");
		await picker.click();
		// The degraded instances are offered, the silenced and passing ones not.
		const options = page.getByRole("option");
		await expect(options).toHaveCount(3);
		await expect(options.nth(0)).toHaveText("Whole check");
		await expect(options.nth(1)).toContainText("Northgate Clinic");
		await expect(options.nth(1)).toContainText("failed");
		await expect(options.nth(2)).toContainText("Ridge Health Centre");
		await options.nth(1).click();
		await expect(picker).toHaveText("Northgate Clinic");
		await page.getByRole("button", { name: "For this server" }).click();

		await expect
			.poll(async () =>
				(await instanceSilences(sql)).map((s) => s.instance_key),
			)
			.toContain("6f1c2a9e-0d4b-4c1e-9a2f-5d8e3b6c7a10");
		// Only the instance: the whole check is not silenced.
		expect(
			(await instanceSilences(sql)).filter((s) => s.instance_key === null),
		).toHaveLength(0);

		await page.goto(`/fleet/applications/${central.id}`);
		await expect(instanceRow(page, "Northgate Clinic")).toContainText(
			"silenced (application)",
		);
	});

	/// Each opening of the issue's silence starts from the whole check, however
	/// the panel was last closed, so a past instance choice never carries over.
	///
	/// spec: CHK#silencing-one-instance
	test("the issue's silence forgets its instance choice when closed", async ({
		page,
		sql,
	}) => {
		const { group, issue } = await centralWithFacilities(sql);
		const incident = await seedIncident(sql, {
			serverGroupId: group.id,
			issues: [{ issueId: issue.id }],
		});
		await page.goto(`/incidents/${incident.id}`);

		const open = page.getByRole("button", { name: "Silence ref…" });
		const picker = page.getByRole("combobox", { name: "Silence" });
		const choose = async (name: string) => {
			await picker.click();
			await page.getByRole("option").filter({ hasText: name }).click();
			await expect(picker).toHaveText(name);
		};

		// Cancelled.
		await open.click();
		await choose("Northgate Clinic");
		await page.getByRole("button", { name: "Cancel" }).click();
		await expect(picker).toHaveCount(0);
		await open.click();
		await expect(picker).toHaveText("Whole check");

		// Closed by its own button.
		await choose("Ridge Health Centre");
		await open.click();
		await expect(picker).toHaveCount(0);
		await open.click();
		await expect(picker).toHaveText("Whole check");

		// Closed by silencing.
		await choose("Northgate Clinic");
		await page.getByRole("button", { name: "For this server" }).click();
		await expect
			.poll(async () =>
				(await instanceSilences(sql)).map((s) => s.instance_key),
			)
			.toContain("6f1c2a9e-0d4b-4c1e-9a2f-5d8e3b6c7a10");
		await expect(picker).toHaveCount(0);
		await open.click();
		await expect(picker).toHaveText("Whole check");
	});

	/// An instance silence lists the instance it quiets, and one whose key the
	/// check no longer reports is marked so an operator can clear it.
	///
	/// spec: CHK#silencing-one-instance
	test("the silenced refs show the instance, and mark one not reported", async ({
		page,
		sql,
	}) => {
		const { group, central } = await centralWithFacilities(sql);
		await seedServerSilencedRef(sql, {
			serverId: central.id,
			ref: REF,
			instance: "5c2e81aa-westfield-09b4",
		});
		await seedGroupSilencedRef(sql, {
			groupId: group.id,
			ref: REF,
			instance: "south",
		});
		await page.goto(`/fleet/applications/${central.id}`);

		const silenced = page.getByTestId("silenced-instance");
		await expect(silenced).toHaveCount(2);
		const eastbay = silenced.filter({ hasText: "Eastbay Clinic" });
		await expect(eastbay).toContainText("a41b9c02…3e7d");
		await expect(eastbay).not.toContainText("not reported");
		const westfield = silenced.filter({ hasText: "5c2e81aa…09b4" });
		await expect(westfield).toContainText("not reported");

		// Clearing the outlived silence.
		const westfieldRow = page
			.locator("div")
			.filter({ has: westfield })
			.filter({ has: page.getByRole("button", { name: "Un-silence" }) })
			.last();
		await westfieldRow.getByRole("button", { name: "Un-silence" }).click();
		await expect(silenced).toHaveCount(1);
		expect(
			(await instanceSilences(sql)).map((s) => s.instance_key),
		).not.toContain("5c2e81aa-westfield-09b4");

		// The group's own silences list the instance too, by its label.
		await page.goto(`/fleet/groups/${group.id}`);
		await expect(
			page.getByTestId("silenced-instance").filter({ hasText: "Southpoint" }),
		).toBeVisible();
	});

	/// spec: CHK#silencing-one-instance
	test("an instance is un-silenced from the checks", async ({ page, sql }) => {
		const { central } = await centralWithFacilities(sql);
		await page.goto(`/fleet/applications/${central.id}`);

		await page
			.getByRole("button", {
				name: `Manage silence for Eastbay Clinic in ${CHECK}`,
			})
			.click();
		await expect(page.getByText("Silenced for this server")).toBeVisible();
		await page.getByRole("button", { name: "Un-silence" }).click();

		// Back in trouble, it is graded as any other instance.
		await expect(
			page.getByRole("button", { name: `Silence Eastbay Clinic in ${CHECK}` }),
		).toBeVisible();
		await expect(instanceRow(page, "Eastbay Clinic")).not.toContainText(
			"silenced",
		);
		expect(await instanceSilences(sql)).toHaveLength(0);
	});

	/// The fleet-wide check page presents the check across every target, so
	/// it offers the catalog policy and no instance silence.
	///
	/// spec: CHK#silencing-one-instance
	test("the fleet-wide check page offers no instance silence", async ({
		page,
		sql,
	}) => {
		await centralWithFacilities(sql);
		await page.goto(
			`/healthchecks/alertd/application.tamanu-central/${CHECK}`,
		);
		await expect(
			page.getByRole("heading", { name: new RegExp(CHECK) }).first(),
		).toBeVisible();
		await expect(page.getByTestId("check-instance")).toHaveCount(0);
		await expect(
			page.getByRole("button", { name: /Northgate Clinic/ }),
		).toHaveCount(0);
	});
});
