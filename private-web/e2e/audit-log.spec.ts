import { randomUUID } from "node:crypto";
import { expect, test } from "./test-fixtures";
import { resetAuditLog, seedAuditEntry } from "./seed";
import { sidebarLink } from "./nav";

const ALICE = { kind: "operator", login: "alice@example.org", name: "Alice Ng" } as const;
const BOB = { kind: "operator", login: "bob@example.org" } as const;

const rowOf = (page: import("@playwright/test").Page, text: string) =>
	page.getByRole("row").filter({ hasText: text });

async function pick(
	page: import("@playwright/test").Page,
	field: string,
	option: string | RegExp,
) {
	await page.getByRole("combobox", { name: field }).click();
	await page.getByRole("option", { name: option }).click();
}

test.describe("audit log page", () => {
	test.beforeEach(async ({ sql }) => {
		await resetAuditLog(sql);
	});

	test("lists entries newest first with actor, action, records and detail", async ({
		page,
		sql,
	}) => {
		const group = randomUUID();
		const machine = randomUUID();
		const device = randomUUID();
		const expectedEnd = new Date(Date.now() + 3 * 3600_000).toISOString();
		await seedAuditEntry(sql, {
			action: "maintenance_lifted",
			minutesAgo: 30,
			actor: BOB,
			records: [{ kind: "group", id: group, name: "Coastal" }],
		});
		await seedAuditEntry(sql, {
			action: "maintenance_declared",
			minutesAgo: 5,
			actor: ALICE,
			records: [
				{ kind: "group", id: group, name: "Coastal" },
				{ kind: "machine", id: machine, name: "host-3" },
				{ kind: "maintenance_window", id: randomUUID(), name: "Coastal window" },
			],
			detail: {
				expected_end: expectedEnd,
				note: "Patching hosts",
				nothing: null,
				blank: "",
				nested: { a: 1 },
			},
		});
		await seedAuditEntry(sql, {
			action: "maintenance_amended",
			minutesAgo: 15,
			actor: { kind: "device", id: device, name: "host-3", role: "backup-restore" },
		});

		await page.goto("/settings/audit-log");
		await expect(sidebarLink(page, "Audit log")).toHaveAttribute(
			"aria-current",
			"page",
		);

		const rows = page.getByRole("row");
		await expect(rows).toHaveCount(4);
		await expect(rows.nth(1)).toContainText("Declare maintenance");
		await expect(rows.nth(2)).toContainText("Amend maintenance");
		await expect(rows.nth(3)).toContainText("Lift maintenance");

		const declared = rows.nth(1);
		await expect(declared).toContainText("alice@example.org");
		await expect(declared.getByRole("link", { name: "Coastal" })).toHaveAttribute(
			"href",
			`/fleet/groups/${group}`,
		);
		await expect(declared.getByRole("link", { name: "host-3" })).toHaveAttribute(
			"href",
			`/fleet/machines/${machine}`,
		);
		// A kind with no page of its own is text, not a link.
		await expect(declared.getByText("Coastal window")).toBeVisible();
		await expect(declared.getByRole("link", { name: "Coastal window" })).toHaveCount(0);

		await expect(declared).toContainText("note");
		await expect(declared).toContainText("Patching hosts");
		await expect(declared).toContainText("expected end");
		await expect(declared).toContainText('{"a":1}');
		await expect(declared).not.toContainText("blank");
		await expect(declared).not.toContainText("nothing");
		const local = await page.evaluate(
			(iso) =>
				new Intl.DateTimeFormat(undefined, {
					dateStyle: "medium",
					timeStyle: "short",
				}).format(new Date(iso)),
			expectedEnd,
		);
		await expect(declared).toContainText(local);
		await expect(declared).not.toContainText(expectedEnd);

		const amended = rows.nth(2);
		await expect(amended.getByRole("link", { name: "host-3" })).toHaveAttribute(
			"href",
			`/devices/${device}`,
		);
		await expect(amended).toContainText("Backup-restore device");
	});

	test("shows failed and unconfirmed entries with a chip, and hides a fresh pending one", async ({
		page,
		sql,
	}) => {
		await seedAuditEntry(sql, { action: "maintenance_declared", minutesAgo: 3, outcome: "done" });
		await seedAuditEntry(sql, { action: "maintenance_lifted", minutesAgo: 4, outcome: "failed" });
		await seedAuditEntry(sql, { action: "maintenance_amended", minutesAgo: 120, outcome: "pending" });
		await seedAuditEntry(sql, { action: "maintenance_declared", minutesAgo: 2, outcome: "pending", detail: { note: "in flight" } });

		await page.goto("/settings/audit-log");

		await expect(page.getByRole("row")).toHaveCount(4);
		await expect(rowOf(page, "Lift maintenance")).toContainText("failed");
		await expect(rowOf(page, "Amend maintenance")).toContainText("unconfirmed");
		await expect(page.getByText("in flight")).toHaveCount(0);
		await expect(rowOf(page, "Declare maintenance").getByText(/failed|unconfirmed/)).toHaveCount(0);
	});

	test("filters by action, and a named action shows regardless of tier", async ({
		page,
		sql,
	}) => {
		await seedAuditEntry(sql, { action: "maintenance_declared", minutesAgo: 3 });
		await seedAuditEntry(sql, { action: "maintenance_lifted", minutesAgo: 4 });

		await page.goto("/settings/audit-log");
		await expect(page.getByRole("row")).toHaveCount(3);

		await pick(page, "Action", "Lift maintenance");
		await expect(page.getByRole("row")).toHaveCount(2);
		await expect(rowOf(page, "Lift maintenance")).toBeVisible();
		await expect(rowOf(page, "Declare maintenance")).toHaveCount(0);

		await pick(page, "Action", "All actions");
		await expect(page.getByRole("row")).toHaveCount(3);
	});

	test("offers important actions first, then all, then each action", async ({ page }) => {
		await page.goto("/settings/audit-log");
		await expect(page.getByRole("combobox", { name: "Action" })).toContainText(
			"Important actions",
		);
		await page.getByRole("combobox", { name: "Action" }).click();
		const options = page.getByRole("option");
		await expect(options.nth(0)).toHaveText("Important actions");
		await expect(options.nth(1)).toHaveText("All actions");
		await expect(options.filter({ hasText: "Declare maintenance" })).toHaveCount(1);
	});

	test("leaves routine actions out by default and includes them under All actions", async ({
		page,
		request,
		sql,
	}) => {
		const filters = await request.post("/api/audit/filters", { data: {} });
		const { actions } = (await filters.json()) as {
			actions: Array<{ name: string; label: string; tier: string }>;
		};
		const routine = actions.find((a) => a.tier === "routine");
		test.skip(!routine, "the catalogue has no routine action yet");
		if (!routine) return;

		await seedAuditEntry(sql, { action: routine.name, minutesAgo: 2 });
		await seedAuditEntry(sql, { action: "maintenance_declared", minutesAgo: 3 });

		await page.goto("/settings/audit-log");
		await expect(rowOf(page, "Declare maintenance")).toBeVisible();
		await expect(rowOf(page, routine.label)).toHaveCount(0);

		await pick(page, "Action", "All actions");
		await expect(rowOf(page, routine.label)).toBeVisible();

		// Naming the action shows it too.
		await pick(page, "Action", new RegExp(`^${routine.label}`));
		await expect(rowOf(page, routine.label)).toBeVisible();
		await expect(rowOf(page, "Declare maintenance")).toHaveCount(0);
	});

	test("filters by actor", async ({ page, sql }) => {
		const device = randomUUID();
		await seedAuditEntry(sql, { action: "maintenance_declared", minutesAgo: 3, actor: ALICE });
		await seedAuditEntry(sql, { action: "maintenance_lifted", minutesAgo: 4, actor: BOB });
		await seedAuditEntry(sql, {
			action: "maintenance_amended",
			minutesAgo: 5,
			actor: { kind: "device", id: device, name: "host-9", role: "machine" },
		});

		await page.goto("/settings/audit-log");
		await expect(page.getByRole("row")).toHaveCount(4);

		await pick(page, "Actor", "bob@example.org");
		await expect(page.getByRole("row")).toHaveCount(2);
		await expect(rowOf(page, "Lift maintenance")).toBeVisible();

		await pick(page, "Actor", /host-9/);
		await expect(page.getByRole("row")).toHaveCount(2);
		await expect(rowOf(page, "Amend maintenance")).toBeVisible();

		await pick(page, "Actor", "Anyone");
		await expect(page.getByRole("row")).toHaveCount(4);
	});

	test("filters by the record acted on, by name or by identifier", async ({
		page,
		sql,
	}) => {
		const coastal = randomUUID();
		await seedAuditEntry(sql, {
			action: "maintenance_declared",
			minutesAgo: 3,
			records: [{ kind: "group", id: coastal, name: "Coastal" }],
		});
		await seedAuditEntry(sql, {
			action: "maintenance_lifted",
			minutesAgo: 4,
			records: [{ kind: "group", id: randomUUID(), name: "Northern" }],
		});

		await page.goto("/settings/audit-log");
		await expect(page.getByRole("row")).toHaveCount(3);

		await page.getByLabel("Acted on").fill("coast");
		await expect(page.getByRole("row")).toHaveCount(2);
		await expect(rowOf(page, "Declare maintenance")).toBeVisible();

		await page.getByLabel("Acted on").fill(coastal);
		await expect(page.getByRole("row")).toHaveCount(2);
		await expect(rowOf(page, "Declare maintenance")).toBeVisible();

		await page.getByLabel("Acted on").fill("nothing-like-this");
		await expect(page.getByText("No entries.")).toBeVisible();
	});

	test("the time range narrows the request, and the longest asks for no cut-off", async ({
		page,
		sql,
	}) => {
		await seedAuditEntry(sql, { action: "maintenance_declared", minutesAgo: 3 });

		const bodies: Array<Record<string, unknown>> = [];
		await page.route("**/api/audit/list", async (route) => {
			bodies.push(route.request().postDataJSON());
			await route.continue();
		});

		await page.goto("/settings/audit-log");
		await expect(page.getByRole("row")).toHaveCount(2);
		expect(bodies.at(-1)?.since).toBeUndefined();

		await pick(page, "Time range", "Last 24 hours");
		await expect.poll(() => bodies.at(-1)?.since).toBeTruthy();
		const since = Date.parse(String(bodies.at(-1)?.since));
		expect(Math.abs(Date.now() - 24 * 3600_000 - since)).toBeLessThan(60_000);
		await expect(page.getByRole("row")).toHaveCount(2);
	});

	test("Load more reads the next page, and only while a page came back full", async ({
		page,
		sql,
	}) => {
		// Entries one action records share their time, so a page can end partway
		// through them. These straddle the first page's end.
		const shared = new Date(Date.now() - 50 * 60_000).toISOString();
		for (let i = 0; i < 55; i++) {
			await seedAuditEntry(sql, {
				action: "maintenance_declared",
				...(i >= 45 && i < 52 ? { recordedAt: shared } : { minutesAgo: 2 + i }),
				detail: { note: `entry-${i}` },
			});
		}

		await page.goto("/settings/audit-log");
		await expect(page.getByRole("row")).toHaveCount(51);
		await expect(page.getByText("entry-0", { exact: true })).toBeVisible();

		await page.getByRole("button", { name: "Load more" }).click();
		await expect(page.getByRole("row")).toHaveCount(56);
		for (let i = 0; i < 55; i++) {
			await expect(page.getByText(`entry-${i}`, { exact: true })).toHaveCount(1);
		}
		await expect(page.getByRole("button", { name: "Load more" })).toHaveCount(0);
	});

	test("an empty log says so", async ({ page }) => {
		await page.goto("/settings/audit-log");
		await expect(page.getByText("No entries.")).toBeVisible();
	});

	test("refresh picks up new entries", async ({ page, sql }) => {
		await page.goto("/settings/audit-log");
		await expect(page.getByText("No entries.")).toBeVisible();
		await seedAuditEntry(sql, { action: "maintenance_declared", minutesAgo: 1 });
		await page.getByRole("button", { name: "Refresh" }).click();
		await expect(rowOf(page, "Declare maintenance")).toBeVisible();
	});

	test("a non-administrator has no tab and no entries", async ({ page, sql }) => {
		await seedAuditEntry(sql, { action: "maintenance_declared", minutesAgo: 3 });
		await page.route("**/api/commons/is_current_user_admin", (route) =>
			route.fulfill({ json: false }),
		);
		let asked = false;
		await page.route("**/api/audit/**", async (route) => {
			asked = true;
			await route.continue();
		});

		await page.goto("/settings/audit-log");

		await expect(page.getByText("Administrators only.")).toBeVisible();
		await expect(sidebarLink(page, "Audit log")).toHaveCount(0);
		await expect(page.getByText("Declare maintenance")).toHaveCount(0);
		expect(asked).toBe(false);
	});
});
