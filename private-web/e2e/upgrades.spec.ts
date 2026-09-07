import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedDevice,
	seedRestoreConsumerCapability,
	seedRestoreReplica,
	seedServer,
	seedServerGroup,
	seedStatus,
	seedUpgradePlan,
	seedVersion,
	seedVersionKnownIssue,
	type ServerRank,
} from "./seed";

/** A server of the group at `rank` reporting `version`, which is what that
 * environment reads as running. */
async function runningAt(
	sql: Parameters<typeof seedServer>[0],
	groupId: string,
	version: string,
	rank: ServerRank = "production",
): Promise<void> {
	const server = await seedServer(sql, { groupId, rank });
	await seedStatus(sql, { serverId: server.id, version });
}

/** Declare a replica on an intent that migrates, which is what makes a plan
 * something the pipeline will act on. */
async function declareUpgradeReplica(
	sql: Parameters<typeof seedRestoreReplica>[0],
	groupId: string,
): Promise<void> {
	const consumer = await seedDevice(sql, { role: "backup-restore" });
	await seedRestoreConsumerCapability(sql, {
		deviceId: consumer.id,
		intents: [{ intent: "upgrade", semantics: ["check", "once", "migrate"] }],
	});
	await seedRestoreReplica(sql, {
		consumerDeviceId: consumer.id,
		groupId,
		intent: "upgrade",
		name: "upgrade",
	});
}

test.describe("upgrades dashboard", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("separates planned groups from ones with nothing recorded", async ({
		page,
		sql,
	}) => {
		const planned = await seedServerGroup(sql, { name: "kamaka" });
		const unplanned = await seedServerGroup(sql, { name: "drifting" });
		const current = await seedServerGroup(sql, { name: "settled" });
		await runningAt(sql, planned.id, "2.60.0");
		await runningAt(sql, unplanned.id, "2.60.0");
		await runningAt(sql, current.id, "2.61.0");
		const target = await seedVersion(sql, {
			major: 2,
			minor: 61,
			patch: 0,
		});

		// Long past, so `late` holds however far in the future this runs.
		await seedUpgradePlan(sql, {
			groupId: planned.id,
			targetVersionId: target.id,
			plannedFor: "2020-01-01",
			note: "site can absorb 2.61 only",
		});
		await declareUpgradeReplica(sql, planned.id);

		await page.goto("/upgrades");

		const plannedRow = page
			.getByTestId("planned-upgrade-row")
			.filter({ hasText: "kamaka" });
		await expect(plannedRow).toContainText("2.60.0");
		await expect(plannedRow).toContainText("2.61.0");
		await expect(page.getByTestId("planned-upgrade-note")).toContainText(
			"site can absorb 2.61 only",
		);
		await expect(plannedRow).toContainText("late");
		// The plan says where it is going; the verdict says whether the data
		// survives getting there. Untested until a consumer reports.
		await expect(plannedRow).toContainText("not yet tested");

		// The group with nothing recorded is the one this view exists to
		// surface, so it is listed rather than omitted, behind a disclosure.
		await page
			.getByRole("button", { name: "Show groups with no plan" })
			.click();
		const drifting = page
			.getByTestId("unplanned-upgrade-row")
			.filter({ hasText: "drifting" });
		await expect(drifting).toBeVisible();
		await expect(drifting).toContainText("1 minor");
		await expect(page.getByTestId("unplanned-upgrades")).not.toContainText(
			"kamaka",
		);
		// A group already on the newest version has nothing to plan.
		await expect(page.getByTestId("unplanned-upgrades")).not.toContainText(
			"settled",
		);
	});

	test("says a plan with nothing declared to test it is not set up", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka" });
		await runningAt(sql, group.id, "2.60.0");
		const target = await seedVersion(sql, { major: 2, minor: 61, patch: 0 });
		await seedUpgradePlan(sql, {
			groupId: group.id,
			targetVersionId: target.id,
			plannedFor: "2020-01-01",
		});

		await page.goto("/upgrades");

		// Nothing is declared to migrate this group's data, so no test will ever
		// run: saying "not yet tested" would leave a reader waiting on a result
		// that cannot arrive.
		const row = page
			.getByTestId("planned-upgrade-row")
			.filter({ hasText: "kamaka" });
		await expect(row).toContainText("not set up");
		await expect(row).not.toContainText("not yet tested");
	});

	test("says so when nothing is planned", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "quiet" });
		await runningAt(sql, group.id, "2.60.0");

		await page.goto("/upgrades");

		await expect(page.getByTestId("planned-upgrades")).toContainText(
			"No group has a recorded plan",
		);
	});

	test("withdrawing a plan puts the group back to unplanned", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka" });
		await runningAt(sql, group.id, "2.60.0");
		const target = await seedVersion(sql, {
			major: 2,
			minor: 61,
			patch: 0,
		});
		await seedUpgradePlan(sql, {
			groupId: group.id,
			targetVersionId: target.id,
		});

		await page.goto("/upgrades");
		await expect(
			page
				.getByTestId("planned-upgrade-row")
				.filter({ hasText: "kamaka" }),
		).toBeVisible();

		page.once("dialog", (dialog) => dialog.accept());
		await page
			.getByRole("button", { name: "Withdraw kamaka's plan" })
			.click();

		// Withdrawn, so it moves to the unplanned list and stops being tested.
		await expect(page.getByTestId("planned-upgrades")).toContainText(
			"No group has a recorded plan",
		);
		await page
			.getByRole("button", { name: "Show groups with no plan" })
			.click();
		await expect(
			page
				.getByTestId("unplanned-upgrade-row")
				.filter({ hasText: "kamaka" }),
		).toBeVisible();

		// The plan is kept, so where kamaka was going stays readable.
		await page.getByRole("button", { name: "Show past plans" }).click();
		const past = page
			.getByTestId("past-plan-row")
			.filter({ hasText: "kamaka" });
		await expect(past).toContainText("2.61.0");
		await expect(past).toContainText("withdrawn");
	});

	test("a plan replaced by a later one reads as replaced, not withdrawn", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka" });
		await runningAt(sql, group.id, "2.60.0");
		const first = await seedVersion(sql, { major: 2, minor: 61, patch: 0 });
		const second = await seedVersion(sql, { major: 2, minor: 63, patch: 0 });
		await seedUpgradePlan(sql, {
			groupId: group.id,
			targetVersionId: first.id,
			note: "site can absorb 2.61 only",
		});
		await seedUpgradePlan(sql, {
			groupId: group.id,
			targetVersionId: second.id,
			supersedes: true,
		});

		await page.goto("/upgrades");
		await page.getByRole("button", { name: "Show past plans" }).click();

		const past = page.getByTestId("past-plan-row").filter({ hasText: "kamaka" });
		await expect(past).toHaveCount(1);
		await expect(past).toContainText("2.61.0");
		await expect(past).toContainText("replaced");
		await expect(page.getByTestId("past-plan-note")).toContainText(
			"site can absorb 2.61 only",
		);
		// The plan that replaced it is where the group is going, so it stays
		// out of the history.
		await expect(
			page.getByTestId("planned-upgrade-row").filter({ hasText: "kamaka" }),
		).toContainText("2.63.0");
	});

	test("amending a plan changes its date and note but not where it is going", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka" });
		await runningAt(sql, group.id, "2.60.0");
		const target = await seedVersion(sql, {
			major: 2,
			minor: 61,
			patch: 0,
		});
		await seedUpgradePlan(sql, {
			groupId: group.id,
			targetVersionId: target.id,
			plannedFor: "2020-01-01",
			note: "waiting on the site",
		});

		await page.goto("/upgrades");
		const row = page
			.getByTestId("planned-upgrade-row")
			.filter({ hasText: "kamaka" });
		const note = page.getByTestId("planned-upgrade-note");
		await expect(note).toContainText("waiting on the site");

		await page.getByRole("button", { name: "Edit kamaka's plan" }).click();
		const dialog = page.getByTestId("edit-plan");
		// Prefilled from the plan, so an operator amends rather than retypes.
		await expect(dialog.getByLabel("Planned for")).toHaveValue(
			"2020-01-01",
		);
		await expect(dialog.getByLabel("Note")).toHaveValue(
			"waiting on the site",
		);

		await dialog.getByLabel("Planned for").fill("2020-03-03");
		await dialog.getByLabel("Note").fill("site confirmed the window");
		await dialog.getByRole("button", { name: "Save" }).click();

		await expect(note).toContainText("site confirmed the window");
		await expect(row).toContainText("2020-03-03");
		// Same plan, same destination: amending is not a replacement.
		await expect(row).toContainText("2.61.0");
		await expect(page.getByTestId("planned-upgrade-row")).toHaveCount(1);
	});

	test("a date no longer expected can be cleared", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "kamaka" });
		await runningAt(sql, group.id, "2.60.0");
		const target = await seedVersion(sql, {
			major: 2,
			minor: 61,
			patch: 0,
		});
		await seedUpgradePlan(sql, {
			groupId: group.id,
			targetVersionId: target.id,
			plannedFor: "2020-01-01",
			plannedTime: "02:00",
			plannedZone: "Pacific/Fiji",
		});

		await page.goto("/upgrades");
		const row = page
			.getByTestId("planned-upgrade-row")
			.filter({ hasText: "kamaka" });
		await expect(row).toContainText("late");

		await page.getByRole("button", { name: "Edit kamaka's plan" }).click();
		const dialog = page.getByTestId("edit-plan");
		await dialog.getByLabel("Planned for").fill("");
		// The hour was an hour of that day, so it goes with it.
		await expect(dialog.getByLabel("Starts")).toHaveValue("");
		await dialog.getByRole("button", { name: "Save" }).click();

		// No date means nothing to be late against.
		await expect(row).not.toContainText("late");
		await expect(row).not.toContainText("FJT");
	});

	test("a clone is planned apart from its production", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka" });
		await runningAt(sql, group.id, "2.60.0");
		await runningAt(sql, group.id, "2.60.0", "clone");
		await seedVersion(sql, { major: 2, minor: 61, patch: 0 });

		await page.goto("/upgrades");
		await page.getByRole("button", { name: "Record a plan" }).click();
		const form = page.getByTestId("record-plan");
		await form.getByLabel("Group").click();
		await page.getByRole("option", { name: "kamaka" }).click();
		// Two environments, so the form does not guess which one is moving.
		await expect(form.getByLabel("Going to")).toBeDisabled();
		await form.getByLabel("Environment").click();
		await page.getByRole("option", { name: /clone/i }).click();
		await form.getByLabel("Going to").click();
		await page.getByRole("option", { name: "2.61.0" }).click();
		await form.getByRole("button", { name: "Record" }).click();

		// The clone's row names its rank; production, still unplanned, stays
		// in the list of gaps.
		const row = page
			.getByTestId("planned-upgrade-row")
			.filter({ hasText: "kamaka" });
		await expect(row).toHaveCount(1);
		await expect(row).toContainText("clone");
		await expect(row).toContainText("2.61.0");
		await expect(
			page.getByRole("button", { name: "Withdraw kamaka clone's plan" }),
		).toBeVisible();
		await page
			.getByRole("button", { name: "Show groups with no plan" })
			.click();
		await expect(
			page
				.getByTestId("unplanned-upgrade-row")
				.filter({ hasText: "kamaka" }),
		).toBeVisible();
	});

	test("a version far behind the newest can still be planned", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka" });
		await runningAt(sql, group.id, "2.53.0");
		// Ten minors of patch releases sit between the group and the newest, so the
		// version it is going to is a long way down the list.
		await seedVersion(sql, { major: 2, minor: 54, patch: 0 });
		for (let minor = 56; minor <= 65; minor++) {
			for (let patch = 0; patch <= 5; patch++) {
				await seedVersion(sql, { major: 2, minor, patch });
			}
		}

		await page.goto("/upgrades");
		await page.getByRole("button", { name: "Record a plan" }).click();
		const form = page.getByTestId("record-plan");
		await form.getByLabel("Group").click();
		await page.getByRole("option", { name: "kamaka" }).click();

		// Unfiltered, the list is the newest patch of each of the last ten minors:
		// short enough to scroll, so the hundreds of plannable patches don't have
		// to be.
		await form.getByLabel("Going to").click();
		await expect(page.getByRole("option")).toHaveCount(10);
		await expect(page.getByRole("option").first()).toHaveText("2.65.5");
		await expect(page.getByRole("option", { name: "2.54.0" })).toHaveCount(0);

		await form.getByLabel("Going to").fill("2.54");
		await page.getByRole("option", { name: "2.54.0" }).click();
		await form.getByRole("button", { name: "Record" }).click();

		await expect(
			page.getByTestId("planned-upgrade-row").filter({ hasText: "kamaka" }),
		).toContainText("2.54.0");
	});

	test("a minor is suggested at its newest patch clear of known issues", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka" });
		await runningAt(sql, group.id, "2.60.0");
		await seedVersion(sql, { major: 2, minor: 61, patch: 0 });
		await seedVersion(sql, { major: 2, minor: 61, patch: 1 });
		await seedVersion(sql, { major: 2, minor: 61, patch: 2 });
		await seedVersionKnownIssue(sql, { major: 2, minor: 61, patch: 2 });

		await page.goto("/upgrades");
		await page.getByRole("button", { name: "Record a plan" }).click();
		const form = page.getByTestId("record-plan");
		await form.getByLabel("Group").click();
		await page.getByRole("option", { name: "kamaka" }).click();
		await form.getByLabel("Going to").click();

		// The newest patch carries the issue, so the minor is suggested one back.
		await expect(page.getByRole("option")).toHaveCount(1);
		await expect(page.getByRole("option")).toHaveText("2.61.1");

		// Still reachable by typing, and marked: an issue may be resolved well
		// before the planned date.
		await form.getByLabel("Going to").fill("2.61.2");
		await expect(
			page.getByRole("option").filter({ hasText: "2.61.2" }),
		).toContainText("known issue");
	});

	test("the hour an upgrade starts reads beside the day", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka" });
		await runningAt(sql, group.id, "2.60.0");
		const target = await seedVersion(sql, {
			major: 2,
			minor: 61,
			patch: 0,
		});
		await seedUpgradePlan(sql, {
			groupId: group.id,
			targetVersionId: target.id,
			plannedFor: "2020-01-01",
			plannedTime: "00:00",
			plannedZone: "Pacific/Fiji",
		});

		await page.goto("/upgrades");
		const row = page
			.getByTestId("planned-upgrade-row")
			.filter({ hasText: "kamaka" });
		// Whose midnight it is, without the reader having to know the group.
		await expect(row).toContainText("12am FJT");

		await page.getByRole("button", { name: "Edit kamaka's plan" }).click();
		const dialog = page.getByTestId("edit-plan");
		await expect(dialog.getByLabel("Starts")).toHaveValue("00:00");
		await expect(dialog.getByLabel("Timezone")).toHaveValue("Pacific/Fiji");

		await dialog.getByLabel("Starts").fill("19:30");
		await dialog.getByLabel("Timezone").fill("Pacific/Nauru");
		await page.getByRole("option", { name: "Pacific/Nauru" }).click();
		await dialog.getByRole("button", { name: "Save" }).click();

		await expect(row).toContainText("7:30pm NRT");
	});

	test("a plan can be recorded with an hour, and the hour taken back off", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka" });
		await runningAt(sql, group.id, "2.60.0");
		await seedVersion(sql, { major: 2, minor: 61, patch: 0 });

		await page.goto("/upgrades");
		await page.getByRole("button", { name: "Record a plan" }).click();
		const form = page.getByTestId("record-plan");
		// Nothing to say about a group until one is named.
		await expect(form.getByLabel("Planned for")).toBeDisabled();
		await expect(form.getByLabel("Note")).toBeDisabled();

		await form.getByLabel("Group").click();
		await page.getByRole("option", { name: "kamaka" }).click();
		await form.getByLabel("Going to").click();
		await page.getByRole("option", { name: "2.61.0" }).click();
		await form.getByLabel("Planned for").fill("2030-04-05");
		await form.getByLabel("Starts").fill("23:00");
		// An hour on its own says nothing about how long the site is down, so
		// the form offers the hour the calendar feed would have assumed.
		await expect(form.getByLabel("Ends")).toHaveValue("00:00");
		// Fiji is where most of the fleet is, so it stands unless changed.
		await expect(form.getByLabel("Timezone")).toHaveValue("Pacific/Fiji");
		await form.getByRole("button", { name: "Record" }).click();

		const row = page
			.getByTestId("planned-upgrade-row")
			.filter({ hasText: "kamaka" });
		await expect(row).toContainText("11pm-12am FJT");

		// An hour that is no longer settled comes off without losing the day.
		await page.getByRole("button", { name: "Edit kamaka's plan" }).click();
		const dialog = page.getByTestId("edit-plan");
		await dialog.getByLabel("Starts").fill("");
		await expect(dialog.getByLabel("Ends")).toHaveValue("");
		await dialog.getByRole("button", { name: "Save" }).click();

		await expect(row).toContainText("2030-04-05");
		await expect(row).not.toContainText("FJT");
	});

	/// An environment that has reported no version cannot be placed against the
	/// newest one, so it has no distance and nothing to plan towards. The list
	/// exists to name what is behind, and a row that cannot say how far behind
	/// it is would sit there permanently with nothing anyone can do about it.
	///
	/// spec: UPG#the-dashboard
	test("an environment that has reported no version is not listed as behind", async ({
		page,
		sql,
	}) => {
		const behind = await seedServerGroup(sql, { name: "drifting" });
		const silent = await seedServerGroup(sql, { name: "silent" });
		await runningAt(sql, behind.id, "2.60.0");
		// A server with no status row has reported nothing.
		await seedServer(sql, { groupId: silent.id, rank: "production" });
		await seedVersion(sql, { major: 2, minor: 61, patch: 0 });

		await page.goto("/upgrades");
		await page
			.getByRole("button", { name: "Show groups with no plan" })
			.click();

		await expect(
			page.getByTestId("unplanned-upgrade-row").filter({ hasText: "drifting" }),
		).toBeVisible();
		await expect(page.getByTestId("unplanned-upgrades")).not.toContainText(
			"silent",
		);
	});

	/// The tables carry seven columns and a note, so at a narrow width they have
	/// to scroll inside their own card. Before this they ran out past the card's
	/// border and under the edge of the window, with the last columns
	/// unreachable.
	///
	/// spec: UPG#the-dashboard
	test("the plan tables scroll inside their card at a narrow width", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka" });
		await runningAt(sql, group.id, "2.58.0");
		await runningAt(sql, group.id, "2.60.0", "clone");
		const target = await seedVersion(sql, { major: 2, minor: 61, patch: 0 });
		// Every column filled, so the table is as wide as it gets in practice.
		await seedUpgradePlan(sql, {
			groupId: group.id,
			targetVersionId: target.id,
			plannedFor: "2020-01-01",
			plannedTime: "22:00",
			plannedEndTime: "02:00",
			plannedZone: "Pacific/Fiji",
			note: "site can absorb 2.61 only, and the window is tight",
		});
		await seedUpgradePlan(sql, {
			groupId: group.id,
			rank: "clone",
			targetVersionId: target.id,
			plannedFor: "2020-02-01",
			note: "clone goes first",
		});
		await declareUpgradeReplica(sql, group.id);

		await page.setViewportSize({ width: 900, height: 900 });
		await page.goto("/upgrades");
		await expect(page.getByTestId("planned-upgrade-row")).toHaveCount(2);

		const card = page.getByTestId("planned-upgrades");
		const scroller = card.locator(".MuiTableContainer-root").first();
		const overflow = await scroller.evaluate((el) => ({
			scrollWidth: el.scrollWidth,
			clientWidth: el.clientWidth,
		}));
		expect(
			overflow.scrollWidth,
			`the table is wider than its scroller, so it scrolls: ${JSON.stringify(overflow)}`,
		).toBeGreaterThan(overflow.clientWidth);

		// And the card itself stays inside the window: the overflow is absorbed
		// by the scroller rather than pushing the layout wide.
		const box = await card.boundingBox();
		expect(box).not.toBeNull();
		expect(box!.x + box!.width).toBeLessThanOrEqual(900);

		// Scrolling the container reaches the far side rather than clipping it.
		await scroller.evaluate((el) => {
			el.scrollLeft = el.scrollWidth;
		});
		expect(await scroller.evaluate((el) => el.scrollLeft)).toBeGreaterThan(0);
	});
});

test.describe("upgrade calendar", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("shows a dated plan on the day it lands", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "kamaka" });
		await runningAt(sql, group.id, "2.60.0");
		const target = await seedVersion(sql, { major: 2, minor: 61, patch: 0 });

		const now = new Date();
		const day = new Date(now.getFullYear(), now.getMonth(), 15);
		const iso = [
			day.getFullYear(),
			String(day.getMonth() + 1).padStart(2, "0"),
			"15",
		].join("-");
		await seedUpgradePlan(sql, {
			groupId: group.id,
			targetVersionId: target.id,
			plannedFor: iso,
		});

		await page.goto("/upgrades");

		const cell = page
			.getByTestId("upgrade-calendar")
			.getByTestId("calendar-day")
			.filter({ has: page.locator(`[data-testid="calendar-entry"]`) });
		await expect(cell).toHaveAttribute("data-date", iso);
		await expect(cell).toContainText("kamaka 2.61.0");

		// The month the reader is looking at is named, and moves.
		await expect(page.getByTestId("upgrade-calendar")).toContainText(
			now.toLocaleDateString(undefined, { month: "long", year: "numeric" }),
		);
		await page.getByRole("button", { name: "next month" }).click();
		await expect(page.getByTestId("calendar-entry")).toHaveCount(0);
		await page.getByRole("button", { name: "Today" }).click();
		await expect(page.getByTestId("calendar-entry")).toHaveCount(1);
	});
});

test.describe("upgrade calendar hours", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("an entry leads with the hour, and names the zone on hover", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka" });
		await runningAt(sql, group.id, "2.60.0");
		const target = await seedVersion(sql, { major: 2, minor: 61, patch: 0 });

		const now = new Date();
		const iso = [
			now.getFullYear(),
			String(now.getMonth() + 1).padStart(2, "0"),
			"15",
		].join("-");
		await seedUpgradePlan(sql, {
			groupId: group.id,
			targetVersionId: target.id,
			plannedFor: iso,
			plannedTime: "22:00",
			plannedZone: "Pacific/Fiji",
			note: "after the ward round",
		});

		await page.goto("/upgrades");

		const entry = page.getByTestId("calendar-entry");
		await expect(entry).toHaveText("10pm kamaka 2.61.0");

		// The zone is one hover away rather than in the cell, where it would
		// cost the version the room to render.
		await entry.hover();
		const tip = page.getByRole("tooltip");
		await expect(tip).toContainText("10pm FJT (Pacific/Fiji)");
		await expect(tip).toContainText("after the ward round");
	});
});

test.describe("upgrade calendar editing", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("clicking an entry amends the plan in place", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "kamaka" });
		await runningAt(sql, group.id, "2.60.0");
		const target = await seedVersion(sql, { major: 2, minor: 61, patch: 0 });

		const now = new Date();
		const month = [
			now.getFullYear(),
			String(now.getMonth() + 1).padStart(2, "0"),
		].join("-");
		await seedUpgradePlan(sql, {
			groupId: group.id,
			targetVersionId: target.id,
			plannedFor: `${month}-15`,
		});

		await page.goto("/upgrades");
		await page.getByTestId("calendar-entry").click();

		const dialog = page.getByTestId("edit-plan");
		await expect(dialog).toContainText("kamaka");
		await dialog.getByLabel("Planned for").fill(`${month}-17`);
		await dialog.getByRole("button", { name: "Save" }).click();

		await expect(
			page.getByTestId("calendar-day").filter({ hasText: "kamaka" }),
		).toHaveAttribute("data-date", `${month}-17`);
	});
});

test.describe("upgrade windows", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("a window recorded by hand reads as a range", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "kamaka" });
		await runningAt(sql, group.id, "2.60.0");
		await seedVersion(sql, { major: 2, minor: 61, patch: 0 });

		await page.goto("/upgrades");
		await page.getByRole("button", { name: "Record a plan" }).click();
		const form = page.getByTestId("record-plan");
		await form.getByLabel("Group").click();
		await page.getByRole("option", { name: "kamaka" }).click();
		await form.getByLabel("Going to").click();
		await page.getByRole("option", { name: "2.61.0" }).click();
		await form.getByLabel("Planned for").fill("2030-04-05");
		await form.getByLabel("Starts").fill("22:00");
		await form.getByLabel("Ends").fill("02:00");
		await form.getByRole("button", { name: "Record" }).click();

		// A window closing before it opened is the next morning, not a plan
		// that runs backwards.
		await expect(
			page.getByTestId("planned-upgrade-row").filter({ hasText: "kamaka" }),
		).toContainText("10pm-2am FJT");
	});

	test("the week view draws the window, and carries it past midnight", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "kamaka" });
		await runningAt(sql, group.id, "2.60.0");
		const target = await seedVersion(sql, { major: 2, minor: 61, patch: 0 });

		// The Monday of the week on screen, so the morning the window runs into
		// is a column of the same week whatever day the suite runs on.
		const now = new Date();
		const monday = new Date(
			now.getFullYear(),
			now.getMonth(),
			now.getDate() - ((now.getDay() + 6) % 7),
		);
		const tuesday = new Date(monday);
		tuesday.setDate(monday.getDate() + 1);

		await seedUpgradePlan(sql, {
			groupId: group.id,
			targetVersionId: target.id,
			plannedFor: localIso(monday),
			plannedTime: "22:00",
			plannedEndTime: "02:00",
			plannedZone: "Pacific/Fiji",
		});

		await page.goto("/upgrades");
		await page.getByRole("button", { name: "week", exact: true }).click();

		await expect(page.getByTestId("calendar-day")).toHaveCount(7);
		await expect(
			page.locator(
				`[data-testid="calendar-day"][data-date="${localIso(monday)}"] [data-testid="calendar-entry"]`,
			),
		).toContainText("10pm-2am");

		await expect(
			page.locator(
				`[data-testid="calendar-day"][data-date="${localIso(tuesday)}"] [data-continues]`,
			),
		).toContainText("until 2am");
	});

	test("a day number opens that day on its own", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "kamaka" });
		await runningAt(sql, group.id, "2.60.0");
		const target = await seedVersion(sql, { major: 2, minor: 61, patch: 0 });

		const now = new Date();
		const fifteenth = new Date(now.getFullYear(), now.getMonth(), 15);
		await seedUpgradePlan(sql, {
			groupId: group.id,
			targetVersionId: target.id,
			plannedFor: localIso(fifteenth),
			plannedTime: "09:00",
			plannedEndTime: "11:30",
			plannedZone: "Pacific/Fiji",
		});

		await page.goto("/upgrades");
		await page.getByRole("button", { name: "15", exact: true }).click();

		const calendar = page.getByTestId("upgrade-calendar");
		const only = calendar.getByTestId("calendar-day");
		await expect(only).toHaveCount(1);
		await expect(only).toHaveAttribute("data-date", localIso(fifteenth));
		await expect(calendar.getByTestId("calendar-entry")).toContainText(
			"9am-11:30am",
		);
	});

});

/** The local calendar day, as the API and the grid both write it. */
function localIso(at: Date): string {
	return [
		at.getFullYear(),
		String(at.getMonth() + 1).padStart(2, "0"),
		String(at.getDate()).padStart(2, "0"),
	].join("-");
}
