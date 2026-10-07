import { expect, test } from "./test-fixtures";

test.describe("Settings", () => {
	test("groups Admins + Recovery vault under one nav item", async ({ page }) => {
		await page.goto("/status");

		// The top bar has a single "Settings" item (no standalone Admins /
		// Recovery vault). Following it lands on Admins (the index).
		await page.getByRole("link", { name: "Settings" }).click();
		await expect(page).toHaveURL(/\/settings\/admins$/);
		await expect(page.getByRole("tab", { name: "Admins" })).toBeVisible();

		// The Recovery vault tab switches to its page.
		await page.getByRole("tab", { name: "Recovery vault" }).click();
		await expect(page).toHaveURL(/\/settings\/recovery$/);
		await expect(
			page.getByRole("heading", { name: /recovery vault/i }),
		).toBeVisible();
	});

	test("backup defaults editor edits the canopy-wide per-type default", async ({
		page,
		sql,
	}) => {
		await page.goto("/settings/backup-defaults");
		// The seeded tamanu-postgres default is shown.
		const card = page.getByTestId("type-default-tamanu-postgres");
		await expect(card).toBeVisible();
		await card.getByRole("radio", { name: "Interval" }).check();
		await card.getByLabel("Back up every (hours)").fill("8");
		await card.getByRole("button", { name: /^save$/i }).click();

		await expect
			.poll(async () => {
				const rows = await sql.query<{ secs: string }>(
					`SELECT EXTRACT(EPOCH FROM default_interval)::text AS secs
					 FROM backup_type_defaults WHERE type = 'tamanu-postgres'`,
				);
				return rows[0] ? Number(rows[0].secs) : null;
			})
			.toBe(28800);
	});

	test("backup defaults editor adds a new canopy-wide type default", async ({
		page,
		sql,
	}) => {
		await page.goto("/settings/backup-defaults");

		const add = page.getByTestId("type-default-new");
		await add.getByLabel("Backup type").fill("tamanu-files");
		await add.getByRole("radio", { name: "Interval" }).check();
		await add.getByLabel("Back up every (hours)").fill("12");
		await add.getByRole("button", { name: /add type/i }).click();

		// The new default lands in the DB...
		await expect
			.poll(async () => {
				const rows = await sql.query<{ secs: string }>(
					`SELECT EXTRACT(EPOCH FROM default_interval)::text AS secs
					 FROM backup_type_defaults WHERE type = 'tamanu-files'`,
				);
				return rows[0] ? Number(rows[0].secs) : null;
			})
			.toBe(43200);

		// ...and the list reloads to show it as its own editor card.
		await expect(page.getByTestId("type-default-tamanu-files")).toBeVisible();
	});

	test("backup defaults dangerous toggle allows retention below the floor", async ({
		page,
		sql,
	}) => {
		await page.goto("/settings/backup-defaults");
		const card = page.getByTestId("type-default-tamanu-postgres");
		await expect(card).toBeVisible();

		// A below-floor daily without the toggle blocks saving.
		await card.getByLabel("Daily").fill("1");
		await expect(
			card.getByText(/daily must be ≥ 7/i),
		).toBeVisible();
		await expect(card.getByRole("button", { name: /^save$/i })).toBeDisabled();

		// Flip the dangerous toggle → the floor no longer blocks; save persists it.
		await card
			.getByLabel(/allow retention below the org minimum/i)
			.check();
		await card.getByRole("button", { name: /^save$/i }).click();

		await expect
			.poll(async () => {
				const rows = await sql.query<{
					allow: boolean;
					keep_daily: string;
				}>(
					`SELECT allow_below_floor AS allow,
					        (default_retention->>'keep_daily') AS keep_daily
					 FROM backup_type_defaults WHERE type = 'tamanu-postgres'`,
				);
				return rows[0] ? `${rows[0].allow}:${rows[0].keep_daily}` : null;
			})
			.toBe("true:1");
	});

	test("backup defaults editor blocks adding a duplicate type", async ({
		page,
	}) => {
		await page.goto("/settings/backup-defaults");

		const add = page.getByTestId("type-default-new");
		await add.getByLabel("Backup type").fill("tamanu-postgres");
		await expect(
			add.getByText(/a default for this type already exists/i),
		).toBeVisible();
		await expect(add.getByRole("button", { name: /add type/i })).toBeDisabled();
	});
});
