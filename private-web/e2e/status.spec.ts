import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedDevice,
	seedIncident,
	seedMachine,
	seedMaintenanceWindow,
	seedServer,
	seedServerGroup,
	seedStatus,
	seedVersion,
} from "./seed";

test.describe("status page", () => {
	test.beforeEach(async ({ sql }) => {
		// Each test inside this file makes assertions about either
		// emptiness or specific seeded rows, so wipe state on entry.
		// Worker-scoped fixtures otherwise leak between tests/specs.
		await resetSeededTables(sql);
	});

	test("renders the nav and redirects /", async ({ page }) => {
		await page.goto("/");
		await expect(page).toHaveURL(/\/status$/);
		await expect(page).toHaveTitle("Status · Canopy");
		await expect(page.getByRole("link", { name: "Status" })).toBeVisible();
	});

	test("empty database surfaces an info banner", async ({ page }) => {
		await page.goto("/status");
		await expect(page.getByText(/no server groups configured/i)).toBeVisible();
	});

	/// One grid, alphabetical. Each card carries its own ranks in its dot strip,
	/// so ordering the page by rank as well sorts it by something already
	/// written on every card.
	///
	/// spec: CHK#presentation
	test("orders cards by name rather than by rank, and links each to its group", async ({
		page,
		sql,
	}) => {
		// group_details computes version-distance against the latest
		// published version; without one the card 404s.
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const prodGroup = await seedServerGroup(sql, { name: "prod-cluster" });
		const demoGroup = await seedServerGroup(sql, { name: "demo-cluster" });
		await seedServer(sql, {
			name: "prod-alpha",
			type: "tamanu-central",
			rank: "production",
			groupId: prodGroup.id,
		});
		await seedServer(sql, {
			name: "demo-beta",
			type: "tamanu-central",
			rank: "demo",
			groupId: demoGroup.id,
		});

		await page.goto("/status");

		// No rank sections: the rank reads off each card's own rows.
		await expect(
			page.getByRole("heading", { name: "production", exact: true }),
		).toHaveCount(0);
		await expect(
			page.getByRole("heading", { name: "demo", exact: true }),
		).toHaveCount(0);

		// Group names render as h3 inside each card.
		await expect(
			page.getByRole("heading", { name: prodGroup.name }),
		).toBeVisible();
		await expect(
			page.getByRole("heading", { name: demoGroup.name }),
		).toBeVisible();

		// Each card is wrapped in a router link to /fleet/groups/<id>.
		await expect(
			page.locator(`a[href="/fleet/groups/${prodGroup.id}"]`),
		).toBeVisible();

		// Alphabetical, so demo-cluster leads prod-cluster despite ranking
		// below it.
		const cards = page.locator('a[href^="/fleet/groups/"]');
		await expect(cards.first()).toHaveAttribute(
			"href",
			`/fleet/groups/${demoGroup.id}`,
		);
		await expect(cards.last()).toHaveAttribute(
			"href",
			`/fleet/groups/${prodGroup.id}`,
		);
	});

	test("dot strips wrap and align across a spread of group sizes", async ({
		page,
		sql,
	}) => {
		test.setTimeout(120_000);
		await seedVersion(sql, { major: 1, minor: 0, patch: 0 });
		const device = await seedDevice(sql);

		// Groups of increasing size so the strip renders unwrapped, wrapped
		// once, and wrapped many times (the screenshots artifact shows the
		// grids). Each group spans three ranks, so two triangle separators
		// appear per strip; health states cycle so plain, failed-ringed,
		// warning-ringed, and never-reported dots all show up.
		const splits: Record<number, [number, number, number]> = {
			5: [3, 1, 1],
			10: [6, 2, 2],
			20: [12, 5, 3],
			30: [18, 8, 4],
			50: [30, 13, 7],
		};
		const groups: Array<{ id: string; size: number }> = [];
		for (const [size, split] of Object.entries(splits)) {
			const n = Number(size);
			const group = await seedServerGroup(sql, { name: `fleet-of-${n}` });
			groups.push({ id: group.id, size: n });
			const ranks = (["production", "clone", "dev"] as const).flatMap(
				(rank, ri) => Array(split[ri]).fill(rank) as ("production" | "clone" | "dev")[],
			);
			for (const [i, rank] of ranks.entries()) {
				const server = await seedServer(sql, {
					name: `f${n}-${i}`,
					type: i === 0 ? "tamanu-central" : "tamanu-facility",
					rank,
					groupId: group.id,
				});
				if (i % 7 === 6) continue; // never reported: grey dot
				const health =
					i % 4 === 3
						? [{ check: "postgres", result: "failed" }]
						: i % 5 === 4
							? [{ check: "disk_space", result: "warning" }]
							: [];
				await seedStatus(sql, {
					serverId: server.id,
					deviceId: device.id,
					healthy: i % 4 !== 3,
					health,
				});
			}
		}

		await page.goto("/status");

		for (const { id, size } of groups) {
			// A group may surface under more than one rank bucket; every card
			// shows the full strip, so scope to the first.
			const card = page.locator(`a[href="/fleet/groups/${id}"]`).first();
			const strip = card.getByTestId("dot-strip");
			await expect(strip).toBeVisible();
			// Three rank rows (production, clone, dev), each a row of machine
			// enclosures rather than a run of dots with a separator between.
			await expect(strip.getByTestId("rank-row")).toHaveCount(3);
			// Every application still has its dot; `seedServer` gives each its
			// own box, so there is one enclosure per dot here.
			await expect(strip.locator("[data-testid='rank-row'] > span")).toHaveCount(
				size,
			);
		}
	});

	/// Two dots in one pill is the whole point: a box carrying two workloads
	/// is one enclosure, and a box carrying one is still an enclosure, so the
	/// presence of a pill never means anything on its own.
	///
	/// spec: FLT
	test("a box carrying two workloads is one enclosure", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "shared-box-status" });
		const shared = await seedMachine(sql, {
			name: "shared",
			groupId: group.id,
		});
		await sql.query(
			`INSERT INTO applications (id, name, host, type, rank, group_id, machine_id)
			 VALUES (gen_random_uuid(), 'pair-central', 'https://pc.e2e.invalid',
			         'tamanu-central', 'production', $1, $2),
			        (gen_random_uuid(), 'pair-facility', 'https://pf.e2e.invalid',
			         'tamanu-facility', 'production', $1, $2)`,
			[group.id, shared.id],
		);
		// A third workload, on a box of its own and at another rank.
		await seedServer(sql, {
			name: "solo",
			rank: "dev",
			groupId: group.id,
		});

		await page.goto("/status");

		const strip = page
			.locator(`a[href="/fleet/groups/${group.id}"]`)
			.first()
			.getByTestId("dot-strip");
		await expect(strip).toBeVisible();

		// Two rank rows: the shared production box, then the dev box.
		const rows = strip.getByTestId("rank-row");
		await expect(rows).toHaveCount(2);
		// The production row is one enclosure holding two dots.
		await expect(rows.first().locator("> span")).toHaveCount(1);
		await expect(rows.first().locator("> span > span")).toHaveCount(2);
		// The dev row is one enclosure holding one — still an enclosure.
		await expect(rows.nth(1).locator("> span")).toHaveCount(1);
		await expect(rows.nth(1).locator("> span > span")).toHaveCount(1);
	});

	/// A window is declared over a box, so the box is what carries it. Before
	/// this the status page had no maintenance signal at all: a box being
	/// worked on looked exactly like one that was not.
	///
	/// spec: MNT#presentation
	test("a box under a maintenance window is marked on the status page", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "window-group" });
		const working = await seedServer(sql, {
			name: "being-worked-on",
			rank: "production",
			groupId: group.id,
		});
		const untouched = await seedServer(sql, {
			name: "left-alone",
			rank: "production",
			groupId: group.id,
		});
		await seedMaintenanceWindow(sql, {
			machineId: working.machineId,
			endsInHours: 2,
		});

		await page.goto("/status");

		const strip = page
			.locator(`a[href="/fleet/groups/${group.id}"]`)
			.first()
			.getByTestId("dot-strip");
		await expect(strip).toBeVisible();

		// A window over the box itself fades and stripes the icon. Both boxes
		// are production and sort by their applications' names, so the
		// worked-on box is first.
		const pills = await strip
			.locator("[data-testid='rank-row'] > span")
			.evaluateAll((els) =>
				els.map((el) => {
					const style = getComputedStyle(el);
					return { stripes: style.backgroundImage, opacity: style.opacity };
				}),
			);
		expect(pills).toHaveLength(2);
		expect(pills[0]!.stripes).not.toBe("none");
		expect(Number(pills[0]!.opacity)).toBeLessThan(1);
		expect(pills[1]!.stripes).toBe("none");
		expect(pills[1]!.opacity).toBe("1");
		// The row is not marked: the window is the box's, not its
		// environment's.
		await expect(
			strip.locator("[data-testid='rank-row']"),
		).not.toHaveAttribute("data-maintenance");
		void untouched;

		// And the pill says why.
		await strip.locator("[data-testid='rank-row'] > span").first().hover();
		await expect(
			page.getByRole("tooltip", { name: /under maintenance/ }),
		).toBeVisible();
	});

	/// A box serving two products is worked on one product at a time, so the
	/// mark belongs on the dot rather than on the box that carries both.
	///
	/// spec: MNT#presentation
	test("an application under its own window is marked on its dot", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "one-product-window" });
		const worked = await seedServer(sql, {
			name: "aaa-being-upgraded",
			rank: "production",
			groupId: group.id,
		});
		await seedServer(sql, {
			name: "bbb-left-serving",
			rank: "production",
			groupId: group.id,
			machineId: worked.machineId,
		});
		await seedMaintenanceWindow(sql, {
			applicationId: worked.id,
			endsInHours: 2,
		});

		await page.goto("/status");

		const pill = page
			.locator(`a[href="/fleet/groups/${group.id}"]`)
			.first()
			.getByTestId("dot-strip")
			.locator("[data-testid='rank-row'] > span")
			.first();
		// One box, two workloads, one of them declared over.
		const dots = pill.getByTestId("status-dot");
		await expect(dots).toHaveCount(2);
		await expect(dots.nth(0)).toHaveAttribute("data-maintenance", "holding");
		await expect(dots.nth(1)).not.toHaveAttribute("data-maintenance");
		// The mark is the dot's own, so the box carrying both stays plain: the
		// machine is not being taken down.
		await expect(pill).not.toHaveAttribute("data-maintenance");

		// The window hollows the dot, leaving a ring in its health colour, so
		// the workload reads as out of play with its health still on it.
		const marks = await dots.evaluateAll((els) =>
			els.map((el) => {
				const style = getComputedStyle(el);
				return { fill: style.backgroundColor, ring: style.borderTopWidth };
			}),
		);
		expect(marks[0]!.fill).toBe("rgba(0, 0, 0, 0)");
		expect(marks[0]!.ring).not.toBe("0px");
		expect(marks[1]!.fill).not.toBe("rgba(0, 0, 0, 0)");
		expect(marks[1]!.ring).toBe("0px");

		// The enclosure names what its dots stand for, so hovering opens one
		// tooltip rather than the dot's and the box's on top of each other.
		await pill.hover();
		await expect(page.getByRole("tooltip")).toHaveCount(1);
		await expect(page.getByRole("tooltip")).toContainText("under maintenance");
	});

	/// Suspension outlasts the window by the settle period, so a box stays
	/// marked after its window is lifted. The mark says which of the two it is,
	/// or a lift that has taken reads as one that has not.
	///
	/// spec: MNT#presentation
	test("a box settling is marked apart from one still being worked on", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "settle-group" });
		const holding = await seedServer(sql, {
			name: "aaa-still-working",
			rank: "production",
			groupId: group.id,
		});
		const settling = await seedServer(sql, {
			name: "bbb-just-lifted",
			rank: "production",
			groupId: group.id,
		});
		await seedMaintenanceWindow(sql, {
			machineId: holding.machineId,
			endsInHours: 2,
		});
		await seedMaintenanceWindow(sql, {
			machineId: settling.machineId,
			endedMinutesAgo: 2,
		});

		await page.goto("/status");

		const pills = page
			.locator(`a[href="/fleet/groups/${group.id}"]`)
			.first()
			.getByTestId("dot-strip")
			.locator("[data-testid='rank-row'] > span");
		await expect(pills).toHaveCount(2);
		await expect(pills.nth(0)).toHaveAttribute("data-maintenance", "holding");
		await expect(pills.nth(1)).toHaveAttribute("data-maintenance", "settling");

		await pills.nth(1).hover();
		await expect(
			page.getByRole("tooltip", { name: /maintenance just ended/ }),
		).toBeVisible();
		// Scoped by the box's name: a tooltip from the previous hover can still
		// be on the page, so a bare tooltip role matches several.
		await pills.nth(0).hover();
		await expect(
			page.getByRole("tooltip", {
				name: /aaa-still-working .* under maintenance/,
			}),
		).toBeVisible();

		// Both stages stripe the icon, and the strength says which. Identical
		// stripes would leave the attribute correct and the page unreadable.
		const marks = await pills.evaluateAll((els) =>
			els.map((el) => getComputedStyle(el).backgroundImage),
		);
		const [holdingMark, settlingMark] = marks;
		expect(holdingMark).not.toBe("none");
		expect(settlingMark).not.toBe("none");
		expect(
			settlingMark,
			"settling has to differ from being worked on",
		).not.toBe(holdingMark);


		// The legend names both, and its swatches are the same component, so a
		// reader can match what they are looking at to a name.
		await expect(
			page.getByText(
				/Striped: under maintenance, raising nothing\. On a machine's icon, an environment's row, or a group's card\./,
			),
		).toBeVisible();
		// The legend's swatch is the same stripes the card draws, so a reader can
		// match what they are looking at to the name. Scoped to the legend: the
		// unscoped lookup found the card's own marks and compared them to
		// themselves.
		const legendStripes = await page
			.getByTestId("maintenance-legend")
			.getByTestId("maintenance-stripes")
			.evaluate((el) => getComputedStyle(el).backgroundImage);
		expect(legendStripes).toBe(holdingMark);
		// And the dot's form, which sits with the other dot states, stripes
		// having no room to resolve at that size.
		const legendDot = await page
			.getByTestId("maintenance-dot-key")
			.getByTestId("status-dot")
			.evaluate((el) => ({
				fill: getComputedStyle(el).backgroundColor,
				ring: getComputedStyle(el).borderTopWidth,
			}));
		expect(legendDot.fill).toBe("rgba(0, 0, 0, 0)");
		expect(legendDot.ring).not.toBe("0px");
	});

	/// A group's window covers every box in it, so every pill on the card is
	/// marked rather than the operator having to know the window was group-wide.
	///
	/// spec: MNT#presentation
	test("a group-wide window marks every box on the card", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "whole-group-window" });
		await seedServer(sql, { name: "one", rank: "production", groupId: group.id });
		await seedServer(sql, { name: "two", rank: "production", groupId: group.id });
		await seedMaintenanceWindow(sql, {
			serverGroupId: group.id,
			endsInHours: 2,
		});

		await page.goto("/status");

		const strip = page
			.locator(`a[href="/fleet/groups/${group.id}"]`)
			.first()
			.getByTestId("dot-strip");
		await expect(strip).toBeVisible();

		// The window is the group's, so the card carries it and the boxes stay
		// plain: marking every pill would say each box was declared over.
		// spec: MNT#presentation
		await expect(page.getByTestId("group-card")).toHaveAttribute(
			"data-maintenance",
			"holding",
		);
		const pills = await strip
			.locator("[data-testid='rank-row'] > span")
			.evaluateAll((els) =>
				els.map((el) => getComputedStyle(el).backgroundImage),
			);
		expect(pills).toHaveLength(2);
		expect(pills.every((f) => f === "none")).toBe(true);
	});

	/// An environment's window covers the machines serving that rank and no
	/// others, so the settle period has to land on those same boxes. Nothing
	/// else exercises a window scoped to a rank reaching a machine.
	///
	/// spec: MNT#settling
	test("a settling environment window marks only that rank's boxes", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "rank-settle" });
		await seedServer(sql, { name: "aaa-clone", rank: "clone", groupId: group.id });
		await seedServer(sql, {
			name: "bbb-production",
			rank: "production",
			groupId: group.id,
		});
		await seedMaintenanceWindow(sql, {
			serverGroupId: group.id,
			rank: "clone",
			endedMinutesAgo: 2,
		});

		await page.goto("/status");

		const card = page.locator(`a[href="/fleet/groups/${group.id}"]`).first();
		// The window is the environment's, so its row carries it rather than
		// each box in it.
		await expect(
			card.locator("[data-testid='rank-row'][data-rank='clone']"),
		).toHaveAttribute("data-maintenance", "settling");
		await expect(
			card.locator("[data-testid='rank-row'][data-rank='production']"),
		).not.toHaveAttribute("data-maintenance");
		await expect(
			card
				.locator("[data-testid='rank-row'][data-rank='clone'] > span")
				.first(),
		).not.toHaveAttribute("data-maintenance");
	});

	/// A quiet card is two bands. The third says what is happening to the
	/// group, so a card only grows one when something is.
	///
	/// spec: CHK#presentation
	test("the status band appears only when there is something to put in it", async ({
		page,
		sql,
	}) => {
		const quiet = await seedServerGroup(sql, { name: "quiet-group" });
		await seedServer(sql, {
			name: "quiet-one",
			rank: "production",
			groupId: quiet.id,
		});
		const noisy = await seedServerGroup(sql, { name: "noisy-group" });
		await seedServer(sql, {
			name: "noisy-one",
			rank: "production",
			groupId: noisy.id,
		});
		await seedIncident(sql, { serverGroupId: noisy.id });

		await page.goto("/status");

		// The quiet card has no incident mark at all.
		const quietCard = page.locator(`a[href="/fleet/groups/${quiet.id}"]`).first();
		await expect(quietCard.getByText("incident", { exact: false })).toHaveCount(
			0,
		);

		// The noisy one carries its incident in a band of its own.
		const noisyCard = page.locator(`a[href="/fleet/groups/${noisy.id}"]`).first();
		await expect(
			noisyCard.getByText("incident", { exact: false }).first(),
		).toBeVisible();

		// The rank is spelled out behind the row rather than marked by a
		// separator between runs of dots.
		await expect(
			quietCard.locator("[data-testid='rank-row'][data-rank='production']"),
		).toHaveCount(1);
	});

	/// Two dots in one pill is the case the machine grain exists for, and an
	/// operator who has never seen a shared box has no way to know it means one
	/// host rather than two dots that happen to be adjacent. So the legend
	/// shows one.
	///
	/// spec: CHK#presentation
	test("the legend shows a machine carrying two applications", async ({
		page,
		sql,
	}) => {
		await seedServer(sql, { name: "legend-one", rank: "production" });

		await page.goto("/status");

		const entry = page
			.getByText("Two applications on one machine")
			.locator("xpath=..");
		await expect(entry).toBeVisible();
		await expect(entry.getByTestId("status-dot")).toHaveCount(2);
	});

	// spec: FIG#active-versions
	test("counts the release branches production is actively running", async ({
		page,
		sql,
	}) => {
		const live = await seedServer(sql, {
			name: "prod-live",
			rank: "production",
		});
		const quiet = await seedServer(sql, {
			name: "prod-quiet",
			rank: "production",
		});
		const testing = await seedServer(sql, { name: "test-box", rank: "test" });

		await seedStatus(sql, { serverId: live.id, version: "2.34.1" });
		// A later push from a source carrying no version must not drop the
		// server out of the summary.
		await seedStatus(sql, {
			serverId: live.id,
			source: "tamanu",
			extra: { uptimeSecs: 42 },
		});
		// Quiet for longer than a week: not running anything, as far as the
		// summary is concerned.
		await seedStatus(sql, {
			serverId: quiet.id,
			version: "2.10.0",
			createdAt: "NOW() - INTERVAL '8 days'",
		});
		await seedStatus(sql, { serverId: testing.id, version: "2.40.0" });

		await page.goto("/status");

		await expect(
			page.getByText(
				"1 release branch in active use: 2.34 (1 version: 2.34.1 — 2.34.1)",
			),
		).toBeVisible();

		// The card answers "which branches"; the figures page answers "which
		// servers, and what else are they running".
		await page.getByRole("link", { name: "Fleet figures" }).click();
		await expect(page).toHaveURL(/\/fleet\/figures$/);
		await expect(page.getByRole("group", { name: "Tamanu" })).toBeVisible();
	});
});
