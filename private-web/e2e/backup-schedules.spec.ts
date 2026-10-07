import type { Locator, Page } from "@playwright/test";
import { raiseTo } from "./safety";
import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	type Sql,
	seedMachineBackupSchedule,
	seedMachineTimezone,
	seedServer,
	seedServerBackupCapability,
	seedServerGroup,
	seedServerGroupBackupConfig,
} from "./seed";

// Cron schedules and per-machine overrides (BKO). The e2e server treats every
// caller as an admin and the fixture raises each session to danger, so these
// drive the editors the way an operator in danger mode would; the grading
// itself is covered in safety-modes.spec.ts.
//
// Firings depend on the current time, so these assert on structure and labels
// rather than on exact instants.

/** A group with ready backups and one box advertising tamanu-postgres. */
async function seedGroupWithBox(
	sql: Sql,
	opts: {
		name: string;
		box?: string;
		timezone?: string;
		/** A schedule override on the group, with no retention override. */
		groupIntervalSeconds?: number;
	},
) {
	const group = await seedServerGroup(sql, { name: opts.name });
	await seedServerGroupBackupConfig(sql, {
		groupId: group.id,
		status: "ready",
		intervalSeconds: opts.groupIntervalSeconds,
	});
	const box = await seedServer(sql, {
		name: opts.box ?? `${opts.name}-box`,
		groupId: group.id,
	});
	await seedServerBackupCapability(sql, { machineId: box.machineId });
	if (opts.timezone) {
		await seedMachineTimezone(sql, {
			machineId: box.machineId,
			timezone: opts.timezone,
		});
	}
	return { group, box };
}

async function chooseZone(scope: Locator | Page, page: Page, zone: string) {
	await scope.getByRole("combobox", { name: "Timezone" }).fill(zone);
	await page.getByRole("option", { name: zone, exact: true }).click();
}

test.describe("fleet default schedule", () => {
	test("switches a type to cron in a zone and saves it as a cron row", async ({
		page,
		sql,
	}) => {
		const before = await sql.query<{ secs: string | null }>(
			`SELECT EXTRACT(EPOCH FROM default_interval)::text AS secs
			 FROM backup_type_defaults WHERE type = 'tamanu-postgres'`,
		);
		try {
			await page.goto("/settings/backup-defaults");
			const card = page.getByTestId("type-default-tamanu-postgres");
			await card.getByRole("radio", { name: "Cron" }).check();
			await card.getByLabel("Cron expression").fill("0 2 * * *");
			await chooseZone(card, page, "Pacific/Auckland");

			// The preview names the zone each firing is read in.
			const preview = card.getByTestId("schedule-preview");
			await expect(preview).toContainText("Pacific/Auckland");
			await expect(preview.getByRole("listitem")).toHaveCount(5);

			await card.getByRole("button", { name: /^save$/i }).click();

			await expect
				.poll(async () => {
					const rows = await sql.query<{
						cron: string | null;
						zone: string | null;
						interval: string | null;
					}>(
						`SELECT default_cron AS cron, default_zone AS zone,
						        default_interval::text AS interval
						 FROM backup_type_defaults WHERE type = 'tamanu-postgres'`,
					);
					const r = rows[0];
					return r ? `${r.cron}|${r.zone}|${r.interval}` : null;
				})
				.toBe("0 2 * * *|Pacific/Auckland|null");

			// The change is on the fleet layer's history, with who made it.
			const history = card.getByTestId("schedule-history");
			await expect(history).toContainText("Cron 0 2 * * * in Pacific/Auckland");
			await expect(history).toContainText("admin@localhost");
		} finally {
			await sql.query(
				`UPDATE backup_type_defaults
				 SET default_cron = NULL, default_zone = NULL,
				     default_interval = CASE WHEN $1::float8 IS NULL THEN NULL
				                             ELSE make_interval(secs => $1::float8) END
				 WHERE type = 'tamanu-postgres'`,
				[before[0]?.secs ?? null],
			);
		}
	});

	test("adding a type with a cron schedule leaves the interval null", async ({
		page,
		sql,
	}) => {
		await page.goto("/settings/backup-defaults");
		const add = page.getByTestId("type-default-new");
		await add.getByLabel("Backup type").fill("tamanu-cron-files");
		await add.getByRole("radio", { name: "Cron" }).check();
		await add.getByLabel("Cron expression").fill("30 3 * * 1-5");
		await add.getByRole("button", { name: /add type/i }).click();

		await expect
			.poll(async () => {
				const rows = await sql.query<{ cron: string | null; zone: string | null }>(
					`SELECT default_cron AS cron, default_zone AS zone
					 FROM backup_type_defaults WHERE type = 'tamanu-cron-files'`,
				);
				return rows[0] ? `${rows[0].cron}|${rows[0].zone}` : null;
			})
			.toBe("30 3 * * 1-5|null");
		const card = page.getByTestId("type-default-tamanu-cron-files");
		await expect(card).toBeVisible();
		await expect(card.getByLabel("Cron expression")).toHaveValue("30 3 * * 1-5");

		await sql.query(
			`DELETE FROM backup_type_defaults WHERE type = 'tamanu-cron-files'`,
		);
	});

	test("refused expressions give their reason as they are typed and cannot be saved", async ({
		page,
	}) => {
		await page.goto("/settings/backup-defaults");
		const card = page.getByTestId("type-default-tamanu-postgres");
		await card.getByRole("radio", { name: "Cron" }).check();
		const expression = card.getByLabel("Cron expression");
		const save = card.getByRole("button", { name: /^save$/i });

		await expression.fill("*/30 * * * *");
		await expect(card.getByText(/less than an hour apart/i)).toBeVisible();
		await expect(save).toBeDisabled();
		await expect(card.getByTestId("schedule-preview")).toHaveCount(0);

		await expression.fill("0 0 30 2 *");
		await expect(card.getByText(/never fires/i)).toBeVisible();
		await expect(save).toBeDisabled();

		// A timezone belongs in its own field, not on the expression.
		await expression.fill("0 2 * * * Pacific/Auckland");
		await expect(card.getByText(/only the five fields|five fields/i)).toBeVisible();
		await expect(save).toBeDisabled();

		// A good one clears the reason and enables saving.
		await expression.fill("0 2 * * *");
		await expect(card.getByTestId("schedule-preview")).toBeVisible();
		await expect(card.getByText(/never fires|apart|five fields/i)).toHaveCount(0);
		await expect(save).toBeEnabled();
	});

	test("an interval under an hour is refused in the editor", async ({ page }) => {
		await page.goto("/settings/backup-defaults");
		const card = page.getByTestId("type-default-tamanu-postgres");
		await card.getByRole("radio", { name: "Interval" }).check();
		await card.getByLabel("Back up every (hours)").fill("0");
		await expect(card.getByText(/at least 1 hour/i)).toBeVisible();
		await expect(card.getByRole("button", { name: /^save$/i })).toBeDisabled();
	});
});

test.describe("group schedule override", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("sets a cron override, shows it as an override, and resets to the default", async ({
		page,
		sql,
	}) => {
		const { group } = await seedGroupWithBox(sql, { name: "cron-group" });

		await page.goto(`/fleet/groups/${group.id}/backups`);
		const type = page.getByTestId("type-schedule-tamanu-postgres");
		await expect(type.getByText("fleet default").first()).toBeVisible();

		await type.getByRole("button", { name: /^override schedule$/i }).click();
		await type.getByRole("radio", { name: "Cron" }).check();
		await type.getByLabel("Cron expression").fill("0 3 * * *");
		await chooseZone(type, page, "Australia/Sydney");
		await expect(type.getByTestId("schedule-preview")).toContainText(
			"Australia/Sydney",
		);
		await type.getByRole("button", { name: /save schedule override/i }).click();

		await expect(type.getByText("group override")).toBeVisible();
		await expect(type).toContainText("Cron 0 3 * * * in Australia/Sydney");
		const rows = await sql.query<{
			cron: string | null;
			zone: string | null;
			interval: string | null;
			has_schedule: boolean;
		}>(
			`SELECT expected_cron AS cron, schedule_zone AS zone,
			        expected_interval::text AS interval, has_schedule
			 FROM server_group_backup_schedule WHERE group_id = $1`,
			[group.id],
		);
		expect(rows).toEqual([
			{
				cron: "0 3 * * *",
				zone: "Australia/Sydney",
				interval: null,
				has_schedule: true,
			},
		]);

		// The group layer's history lists the change.
		await type.getByRole("button", { name: /^edit schedule override$/i }).click();
		await expect(type.getByTestId("schedule-history")).toContainText(
			"Cron 0 3 * * * in Australia/Sydney",
		);
		await expect(type.getByTestId("schedule-history")).toContainText(
			"admin@localhost",
		);

		await type.getByRole("button", { name: /reset to default/i }).click();
		await expect(type.getByText("group override")).toHaveCount(0);
		await expect(type.getByText("fleet default").first()).toBeVisible();
		await expect
			.poll(async () => {
				const r = await sql.query<{ has_schedule: boolean }>(
					`SELECT has_schedule FROM server_group_backup_schedule WHERE group_id = $1`,
					[group.id],
				);
				return r[0]?.has_schedule ?? "gone";
			})
			.not.toBe(true);

		// And the clearing is in the history too.
		await type.getByRole("button", { name: /^override schedule$/i }).click();
		await expect(type.getByTestId("schedule-history")).toContainText("Cleared");
	});

	test("a manual-only override is a schedule of its own", async ({
		page,
		sql,
	}) => {
		const { group } = await seedGroupWithBox(sql, { name: "manual-group" });
		await page.goto(`/fleet/groups/${group.id}/backups`);
		const type = page.getByTestId("type-schedule-tamanu-postgres");
		await type.getByRole("button", { name: /^override schedule$/i }).click();
		await type.getByRole("radio", { name: "Manual only" }).check();
		await type.getByRole("button", { name: /save schedule override/i }).click();

		await expect(type.getByText("group override")).toBeVisible();
		await expect(type).toContainText("Manual only");
		const rows = await sql.query<{ has_schedule: boolean }>(
			`SELECT has_schedule FROM server_group_backup_schedule WHERE group_id = $1`,
			[group.id],
		);
		expect(rows[0]?.has_schedule).toBe(true);
	});

	test("the group preview lists one machine per distinct zone and flags a UTC fallback", async ({
		page,
		sql,
	}) => {
		const { group } = await seedGroupWithBox(sql, {
			name: "zones-group",
			box: "box-auckland",
			timezone: "Pacific/Auckland",
		});
		const second = await seedServer(sql, {
			name: "box-silent",
			groupId: group.id,
		});
		await seedServerBackupCapability(sql, { machineId: second.machineId });

		await page.goto(`/fleet/groups/${group.id}/backups`);
		const type = page.getByTestId("type-schedule-tamanu-postgres");
		await type.getByRole("button", { name: /^override schedule$/i }).click();
		await type.getByRole("radio", { name: "Cron" }).check();
		await type.getByLabel("Cron expression").fill("0 2 * * *");

		const preview = type.getByTestId("schedule-preview");
		await expect(preview).toContainText("box-auckland");
		await expect(preview).toContainText("Pacific/Auckland");
		await expect(preview).toContainText("box-silent");
		await expect(preview.getByText(/no timezone reported/i)).toBeVisible();
		await expect(preview.getByText(/no timezone reported/i)).toHaveCount(1);
	});

	test("retention saves and resets apart from the schedule", async ({
		page,
		sql,
	}) => {
		const { group } = await seedGroupWithBox(sql, {
			name: "retention-group",
			groupIntervalSeconds: 43200,
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		const type = page.getByTestId("type-schedule-tamanu-postgres");
		// The seeded schedule override stands; retention inherits.
		await expect(type.getByText("group override")).toBeVisible();
		await expect(type.getByText("retention override", { exact: true })).toHaveCount(0);

		await type.getByRole("button", { name: /^override retention$/i }).click();
		await type.getByLabel("Daily").fill("10");
		await type
			.getByRole("button", { name: /save retention override/i })
			.click();
		await expect(type.getByText("retention override", { exact: true })).toBeVisible();
		await expect(type).toContainText("daily 10");
		await expect
			.poll(async () => {
				const r = await sql.query<{ keep_daily: string }>(
					`SELECT retention->>'keep_daily' AS keep_daily
					 FROM server_group_backup_schedule WHERE group_id = $1`,
					[group.id],
				);
				return r[0]?.keep_daily ?? null;
			})
			.toBe("10");

		await type
			.getByRole("button", { name: /^edit retention override$/i })
			.click();
		await type
			.getByRole("button", { name: /reset retention to default/i })
			.click();
		await expect(type.getByText("retention override", { exact: true })).toHaveCount(0);
		// The schedule override is untouched by clearing retention.
		await expect(type.getByText("group override")).toBeVisible();
		const rows = await sql.query<{
			retention: unknown;
			interval: string | null;
		}>(
			`SELECT retention, expected_interval::text AS interval
			 FROM server_group_backup_schedule WHERE group_id = $1`,
			[group.id],
		);
		expect(rows[0]?.retention).toBeNull();
		expect(rows[0]?.interval).not.toBeNull();
	});
});

test.describe("machine schedule override", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("on the group view: shows the inherited schedule, sets an override, then resets", async ({
		page,
		sql,
	}) => {
		const { group, box } = await seedGroupWithBox(sql, {
			name: "mach-group",
			box: "mach-box",
			timezone: "Pacific/Auckland",
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		const panel = page
			.getByRole("heading", { name: /^machines$/i })
			.locator("..");
		const row = panel.getByRole("row").filter({ hasText: "mach-box" });
		await expect(row.getByText("fleet default")).toBeVisible();

		await row
			.getByRole("button", { name: /override tamanu-postgres schedule/i })
			.click();
		const dialog = page.getByRole("dialog");
		// With no override of its own, the editor says what it would replace.
		await expect(dialog).toContainText("Following");
		await expect(dialog).toContainText("fleet default");

		await dialog.getByRole("radio", { name: "Cron" }).check();
		await dialog.getByLabel("Cron expression").fill("15 4 * * *");
		// The preview is this machine's own, read in its reported zone.
		await expect(dialog.getByTestId("schedule-preview")).toContainText(
			"mach-box",
		);
		await expect(dialog.getByTestId("schedule-preview")).toContainText(
			"Pacific/Auckland",
		);
		await dialog.getByRole("button", { name: /save override/i }).click();
		await expect(dialog).toHaveCount(0);

		await expect(row.getByText("machine override")).toBeVisible();
		await expect(row).toContainText("Cron 15 4 * * *");
		// Next backup is the next firing, read in the machine's zone.
		const next = row.getByRole("cell").nth(3);
		await expect(next).toContainText(/NZ|GMT\+1[23]|\(in /);
		await expect(next).not.toHaveText("—");

		const stored = await sql.query<{ cron: string; zone: string | null }>(
			`SELECT expected_cron AS cron, schedule_zone AS zone
			 FROM machine_backup_schedule WHERE machine_id = $1`,
			[box.machineId],
		);
		expect(stored).toEqual([{ cron: "15 4 * * *", zone: null }]);

		// History for this machine's layer lists who and when.
		await row
			.getByRole("button", { name: /edit tamanu-postgres schedule/i })
			.click();
		const again = page.getByRole("dialog");
		const history = again.getByTestId("schedule-history");
		await expect(history).toContainText("Cron 15 4 * * *");
		await expect(history).toContainText("admin@localhost");

		await again.getByRole("button", { name: /reset to inherited/i }).click();
		await expect(again).toHaveCount(0);
		await expect(row.getByText("fleet default")).toBeVisible();
		await expect(row.getByText("machine override")).toHaveCount(0);
		const gone = await sql.query(
			`SELECT 1 FROM machine_backup_schedule WHERE machine_id = $1`,
			[box.machineId],
		);
		expect(gone).toHaveLength(0);
	});

	test("on the machine page: sets an override, shows its layer and next backup, then resets", async ({
		page,
		sql,
	}) => {
		const { box } = await seedGroupWithBox(sql, { name: "page-group" });

		await page.goto(`/fleet/machines/${box.machineId}`);
		const backups = page.locator("#backups");
		await expect(backups.getByText("fleet default")).toBeVisible();
		await expect(backups.getByText(/next backup:/i)).toBeVisible();

		await backups
			.getByRole("button", { name: /override tamanu-postgres schedule/i })
			.click();
		const dialog = page.getByRole("dialog");
		await dialog.getByRole("radio", { name: "Cron" }).check();
		await dialog.getByLabel("Cron expression").fill("0 1 * * *");
		await chooseZone(dialog, page, "Pacific/Fiji");
		await dialog.getByRole("button", { name: /save override/i }).click();
		await expect(dialog).toHaveCount(0);

		await expect(backups.getByText("machine override")).toBeVisible();
		await expect(backups).toContainText("Cron 0 1 * * * in Pacific/Fiji");
		await expect(backups).toContainText(/next backup:.*\(in /i);

		await backups
			.getByRole("button", { name: /edit tamanu-postgres schedule/i })
			.click();
		const again = page.getByRole("dialog");
		await expect(again.getByTestId("schedule-history")).toContainText(
			"Cron 0 1 * * * in Pacific/Fiji",
		);
		await again.getByRole("button", { name: /reset to inherited/i }).click();
		await expect(again).toHaveCount(0);
		await expect(backups.getByText("machine override")).toHaveCount(0);
		await expect(backups.getByText("fleet default")).toBeVisible();
	});

	test("a machine with no reported timezone is flagged on both views and in the preview", async ({
		page,
		sql,
	}) => {
		const { group, box } = await seedGroupWithBox(sql, {
			name: "utc-group",
			box: "utc-box",
		});
		await seedMachineBackupSchedule(sql, {
			machineId: box.machineId,
			cron: { expression: "0 2 * * *" },
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		const row = page.getByRole("row").filter({ hasText: "utc-box" });
		await expect(row.getByText("machine override")).toBeVisible();
		await expect(row.getByText(/no timezone reported/i)).toBeVisible();

		await page.goto(`/fleet/machines/${box.machineId}`);
		const backups = page.locator("#backups");
		await expect(backups.getByText(/no timezone reported/i)).toBeVisible();

		await backups
			.getByRole("button", { name: /edit tamanu-postgres schedule/i })
			.click();
		const dialog = page.getByRole("dialog");
		await expect(
			dialog.getByTestId("schedule-preview").getByText(/no timezone reported/i),
		).toBeVisible();
		// Giving the schedule a zone of its own ends the fallback.
		await chooseZone(dialog, page, "UTC");
		await expect(
			dialog.getByTestId("schedule-preview").getByText(/no timezone reported/i),
		).toHaveCount(0);
	});

	test("a stored expression that can no longer be read shows as unreadable, not manual", async ({
		page,
		sql,
	}) => {
		const { box } = await seedGroupWithBox(sql, {
			name: "unreadable-group",
			box: "unreadable-box",
		});
		await seedMachineBackupSchedule(sql, {
			machineId: box.machineId,
			cron: { expression: "0 2 * * * *" },
		});

		await page.goto(`/fleet/machines/${box.machineId}`);
		const backups = page.locator("#backups");
		await expect(backups.getByText("unreadable schedule")).toBeVisible();
		await expect(backups.getByText(/next backup:\s*manual/i)).toHaveCount(0);
	});

	test("a machine override wins over the group's", async ({
		page,
		sql,
	}) => {
		const { group, box } = await seedGroupWithBox(sql, {
			name: "win-group",
			groupIntervalSeconds: 43200,
		});
		await seedMachineBackupSchedule(sql, {
			machineId: box.machineId,
			intervalSeconds: 7200,
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		const row = page.getByRole("row").filter({ hasText: "win-group-box" });
		await expect(row.getByText("machine override")).toBeVisible();
		await expect(row).toContainText("Every 2 hours");
		// The group's own line still reads its own schedule.
		await expect(
			page.getByTestId("type-schedule-tamanu-postgres"),
		).toContainText("Every 12 hours");
	});
});

test.describe("safety grading", () => {
	test.use({ safetyMode: "read-only" });
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("every schedule set and clear works in write mode; clearing retention needs danger", async ({
		page,
		sql,
	}) => {
		const { group } = await seedGroupWithBox(sql, {
			name: "graded-group",
			box: "graded-box",
			groupIntervalSeconds: 43200,
		});
		await sql.query(
			`UPDATE server_group_backup_schedule
			 SET retention = '{"keep_latest":1,"keep_daily":7,"keep_weekly":4,"keep_monthly":6,"keep_annual":0}'::jsonb
			 WHERE group_id = $1`,
			[group.id],
		);

		await page.goto(`/fleet/groups/${group.id}/backups`);
		const type = page.getByTestId("type-schedule-tamanu-postgres");
		const row = page.getByRole("row").filter({ hasText: "graded-box" });

		// Read-only: every schedule control is there and blocked for write.
		await expect(
			type
				.getByLabel(/requires write mode/i)
				.getByRole("button", { name: /^edit schedule override$/i }),
		).toBeVisible();
		await expect(
			row
				.getByLabel(/requires write mode/i)
				.getByRole("button", { name: /override tamanu-postgres schedule/i }),
		).toBeVisible();

		await raiseTo(page, "write");

		// Group schedule: set, then reset.
		await type.getByRole("button", { name: /^edit schedule override$/i }).click();
		await type.getByLabel("Back up every (hours)").fill("24");
		await type.getByRole("button", { name: /save schedule override/i }).click();
		await expect(type).toContainText("Every 24 hours");
		await type.getByRole("button", { name: /^edit schedule override$/i }).click();
		await type.getByRole("button", { name: /reset to default/i }).click();
		await expect(type.getByText("group override", { exact: true })).toHaveCount(0);

		// Machine schedule: set, then reset.
		await row
			.getByRole("button", { name: /override tamanu-postgres schedule/i })
			.click();
		const dialog = page.getByRole("dialog");
		await dialog.getByRole("radio", { name: "Manual only" }).check();
		await dialog.getByRole("button", { name: /save override/i }).click();
		await expect(row.getByText("machine override")).toBeVisible();
		await row
			.getByRole("button", { name: /edit tamanu-postgres schedule/i })
			.click();
		await page.getByRole("button", { name: /reset to inherited/i }).click();
		await expect(row.getByText("machine override")).toHaveCount(0);

		// Retention: saving is write, clearing is danger.
		await type
			.getByRole("button", { name: /^edit retention override$/i })
			.click();
		await expect(
			type.getByRole("button", { name: /save retention override/i }),
		).toBeEnabled();
		const reset = type.getByRole("button", {
			name: /reset retention to default/i,
		});
		await expect(
			type.getByLabel(/requires danger mode/i).getByRole("button", {
				name: /reset retention to default/i,
			}),
		).toBeVisible();
		await reset.click();
		const raise = page.getByRole("dialog");
		await expect(raise).toContainText("This action needs danger mode");
		await raise.getByRole("button", { name: "Continue in danger mode" }).click();
		await expect(
			type.getByText("retention override", { exact: true }),
		).toHaveCount(0);
	});
});
