import { expect, test } from "./test-fixtures";
import { seedIssue, seedServer, seedServerGroup } from "./seed";

const daysAgo = (days: number) =>
	new Date(Date.now() - days * 86_400_000).toISOString();

test.describe("An application's check list", () => {
	/// A source gone quiet leaves its checks at their last result, muted and
	/// aged, while a source still reporting reads as it always has.
	///
	/// spec: CHK#presentation
	test("mutes a quiet source's checks and says when they last reported", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "quiet-source-group" });
		const server = await seedServer(sql, {
			name: "quiet-source",
			groupId: group.id,
		});
		await seedIssue(sql, {
			serverId: server.id,
			source: "tamanu",
			ref: "health/tasks",
			active: false,
			lastSeen: daysAgo(46),
		});
		await seedIssue(sql, {
			serverId: server.id,
			source: "alertd",
			ref: "health/db_connect",
			active: false,
		});

		await page.goto(`/fleet/applications/${server.id}`);

		const quiet = page
			.getByTestId("check-row")
			.filter({ hasText: "tamanu-central:tasks" });
		await expect(quiet).toHaveAttribute("data-quiet", "true");
		await expect(quiet).toContainText("last reported 46d ago");

		const live = page
			.getByTestId("check-row")
			.filter({ hasText: "tamanu-central:db_connect" });
		await expect(live).not.toHaveAttribute("data-quiet", "true");
		await expect(live).not.toContainText("last reported");
	});

	/// Each check presents once, as it currently stands: a resolved state and
	/// a check its source no longer reports are not listed.
	///
	/// spec: CHK#presentation
	test("lists only current checks", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "current-group" });
		const server = await seedServer(sql, {
			name: "current-only",
			groupId: group.id,
		});
		await seedIssue(sql, {
			serverId: server.id,
			source: "alertd",
			ref: "health/db_connect",
			active: false,
		});
		await seedIssue(sql, {
			serverId: server.id,
			source: "alertd",
			ref: "health/report_errors",
			resolved: true,
		});
		await seedIssue(sql, {
			serverId: server.id,
			source: "alertd",
			ref: "health/kopia_backup",
			active: false,
			lastSeen: daysAgo(30),
		});

		await page.goto(`/fleet/applications/${server.id}`);

		await expect(
			page.getByText("tamanu-central:db_connect", { exact: true }),
		).toBeVisible();
		await expect(
			page.getByText("tamanu-central:report_errors", { exact: true }),
		).toHaveCount(0);
		await expect(
			page.getByText("tamanu-central:kopia_backup", { exact: true }),
		).toHaveCount(0);
	});

	/// Most urgent first, then alphabetically by the name each check reads
	/// as, so a bare name and an application type's interleave on what an
	/// operator reads.
	///
	/// spec: CHK#presentation
	test("orders by result, then by presented name", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "order-group" });
		const server = await seedServer(sql, {
			name: "ordered",
			groupId: group.id,
		});
		await seedIssue(sql, {
			serverId: server.id,
			source: "alertd",
			ref: "health/caddy_certs",
			active: false,
		});
		await seedIssue(sql, {
			serverId: server.id,
			source: "alertd",
			ref: "health/caddy_version",
			active: false,
		});
		await seedIssue(sql, {
			serverId: server.id,
			source: "alertd",
			ref: "health/sync_facility_stale",
		});

		await page.goto(`/fleet/applications/${server.id}`);

		const rows = page.getByTestId("check-row");
		await expect(rows.first()).toContainText(
			"tamanu-central:sync_facility_stale",
		);
		const names = await rows.locator("a").allTextContents();
		const listed = names.filter((n) =>
			[
				"tamanu-central:sync_facility_stale",
				"caddy_version",
				"tamanu-central:caddy_certs",
			].includes(n),
		);
		expect(listed).toEqual([
			"tamanu-central:sync_facility_stale",
			"caddy_version",
			"tamanu-central:caddy_certs",
		]);
	});
});
