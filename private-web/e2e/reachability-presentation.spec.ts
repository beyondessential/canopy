import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedServer,
	seedServerGroup,
	seedServerSilencedRef,
	seedStatus,
	seedVersion,
} from "./seed";

// spec: CHK#reachability
test.describe("reachability presents before anything is wrong", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("a server that has never gone quiet shows a passing reachability check", async ({
		page,
		sql,
	}) => {
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const server = await seedServer(sql, { name: "never-quiet" });
		await seedStatus(sql, { serverId: server.id, healthy: true });

		await page.goto(`/fleet/applications/${server.id}`);

		// The check is there with nothing filed against it, so its silence
		// control is reachable before the server has ever gone away.
		await expect(
			page.getByRole("link", { name: "reachability", exact: true }),
		).toBeVisible();
		// Passing, so it doesn't drag the headline.
		await expect(page.getByText("Healthy", { exact: true })).toBeVisible();
	});

	test("a silenced reachability presents as silenced rather than green", async ({
		page,
		sql,
	}) => {
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const server = await seedServer(sql, { name: "hushed-reach" });
		await seedStatus(sql, { serverId: server.id, healthy: true });
		// Canopy's own checks are silenced at a bare ref, not under `health/`.
		await seedServerSilencedRef(sql, {
			serverId: server.id,
			source: "canopy",
			ref: "reachability",
		});

		await page.goto(`/fleet/applications/${server.id}`);

		await expect(
			page.getByRole("link", { name: "reachability", exact: true }),
		).toBeVisible();
		// The chip only matches if the UI builds canopy's ref the same way the
		// backend stores it.
		await expect(page.getByText("silenced (application)")).toBeVisible();
	});
});

// spec: CHK#monitoring-gate
test.describe("unmonitored servers are marked", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("an unmonitored server's health chip carries the silence marker", async ({
		page,
		sql,
	}) => {
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const server = await seedServer(sql, {
			name: "off-the-radar",
			isMonitored: false,
		});
		await seedStatus(sql, {
			serverId: server.id,
			healthy: false,
			health: [{ check: "database", result: "failed" }],
		});

		await page.goto(`/fleet/applications/${server.id}`);

		// The state is still true and still says Unhealthy — what's off is the
		// alerting, and the marker is what says so.
		await expect(page.getByText("Unhealthy", { exact: true })).toBeVisible();
		await expect(page.getByTestId("unmonitored-marker")).toBeVisible();
	});

	test("a monitored server's health chip carries no marker", async ({
		page,
		sql,
	}) => {
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const server = await seedServer(sql, {
			name: "on-the-radar",
			isMonitored: true,
		});
		await seedStatus(sql, {
			serverId: server.id,
			healthy: false,
			health: [{ check: "database", result: "failed" }],
		});

		await page.goto(`/fleet/applications/${server.id}`);

		await expect(page.getByText("Unhealthy", { exact: true })).toBeVisible();
		await expect(page.getByTestId("unmonitored-marker")).toHaveCount(0);
	});

	test("the status page cuts through an unmonitored server's dot", async ({
		page,
		sql,
	}) => {
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const group = await seedServerGroup(sql, { name: "mixed-monitoring" });
		const watched = await seedServer(sql, {
			name: "watched",
			type: "tamanu-central",
			rank: "production",
			groupId: group.id,
			isMonitored: true,
		});
		const ignored = await seedServer(sql, {
			name: "ignored",
			type: "tamanu-facility",
			rank: "production",
			groupId: group.id,
			isMonitored: false,
		});
		await seedStatus(sql, { serverId: watched.id, healthy: true });
		await seedStatus(sql, { serverId: ignored.id, healthy: true });

		await page.goto("/status");

		const strip = page
			.locator(`a[href="/fleet/groups/${group.id}"]`)
			.first()
			.getByTestId("dot-strip");
		await expect(strip).toBeVisible();

		// The cut is a mask on the dot itself, so assert on computed style
		// rather than a marker element: at a single dot's size there's no room
		// for anything else to carry the distinction. Each server was seeded on
		// its own box, so each dot sits in a cell in its own enclosure; both are
		// production, and centrals sort first within a rank, so the watched
		// central is dot 0 and the ignored facility is dot 1.
		const masks = await strip
			.locator("[data-testid='rank-row'] > span > span > span")
			.evaluateAll((els) => els.map((el) => getComputedStyle(el).maskImage));
		expect(masks).toHaveLength(2);
		expect(masks[0]).toBe("none");
		expect(masks[1]).not.toBe("none");

		// And the box says why, for anyone who hovers it: the enclosure names
		// what each of its dots stands for, so the dots carry no tooltip of
		// their own to open on top of it.
		await strip
			.locator("[data-testid='rank-row'] > span > span > span")
			.nth(1)
			.hover();
		await expect(
			page.getByRole("tooltip", { name: /unmonitored/ }),
		).toBeVisible();
	});
});
