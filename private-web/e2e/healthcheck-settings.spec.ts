import { expect, test } from "./test-fixtures";
import { resetSeededTables, seedCheckPolicy } from "./seed";

test.describe("healthcheck settings page", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("shows a single source-qualified 'flagging' link, not a duplicate", async ({
		page,
		sql,
	}) => {
		await seedCheckPolicy(sql, {
			checkName: "caddy_version",
			source: "alertd",
			applicationType: null,
			ceiling: "warning",
		});

		await page.goto("/settings/healthchecks/alertd/machine/caddy_version");

		// The link used to be rendered twice (a copy-paste bug). There must be
		// exactly one, and it must carry the source in its href — the "who's
		// affected" page is keyed on the (source, check) pair.
		const links = page.getByRole("link", {
			name: /See servers currently flagging this check/,
		});
		await expect(links).toHaveCount(1);
		await expect(links).toHaveAttribute(
			"href",
			"/healthchecks/alertd/machine/caddy_version",
		);
	});

	test("is scoped to one (source, check): same-named checks edit independently", async ({
		page,
		sql,
	}) => {
		// Two unrelated checks that happen to share a name, from different
		// sources, each with a distinct ceiling.
		await seedCheckPolicy(sql, {
			checkName: "version",
			source: "alertd",
			ceiling: "warning",
		});
		await seedCheckPolicy(sql, {
			checkName: "version",
			source: "bestool",
			ceiling: "failed",
		});

		// The alertd page shows only alertd's entry (its warning ceiling),
		// not a lumped-together view of both sources.
		await page.goto("/settings/healthchecks/alertd/application.tamanu-central/version");
		await expect(page.getByText("source: alertd")).toBeVisible();
		await expect(page.getByText("source: bestool")).toHaveCount(0);
		await expect(
			page.getByRole("link", {
				name: /See servers currently flagging this check/,
			}),
		).toHaveCount(1);

		// The bestool page is a separate, independently-addressable editor.
		await page.goto("/settings/healthchecks/bestool/application.tamanu-central/version");
		await expect(page.getByText("source: bestool")).toBeVisible();
		await expect(page.getByText("source: alertd")).toHaveCount(0);
	});

	test("admin sees editable controls (edit docs button, enabled escalate toggle)", async ({
		page,
		sql,
	}) => {
		await seedCheckPolicy(sql, {
			checkName: "caddy_version",
			source: "alertd",
			applicationType: null,
			// Escalation is only offered at a failed ceiling, so seed one to
			// see the toggle in its enabled state.
			ceiling: "failed",
		});

		await page.goto("/settings/healthchecks/alertd/machine/caddy_version");

		// The escalate toggle is an interactive, enabled switch for admins
		// (non-admins get a read-only chip instead). MUI's Switch is exposed
		// with role "switch".
		const escalate = page.getByRole("switch", { name: /Escalates/ });
		await expect(escalate).toBeEnabled();

		// The documentation editor's entry button is present.
		await expect(
			page.getByRole("button", { name: /Write documentation|^Edit$/ }),
		).toBeVisible();
	});

	test("escalate toggle is only enabled at a failed ceiling", async ({
		page,
		sql,
	}) => {
		// A warning-ceiling check can never produce a failed effective result,
		// so there is nothing for escalation to bypass grace on: the toggle is
		// disabled.
		await seedCheckPolicy(sql, {
			checkName: "caddy_version",
			source: "alertd",
			applicationType: null,
			ceiling: "warning",
		});

		await page.goto("/settings/healthchecks/alertd/machine/caddy_version");
		const escalate = page.getByRole("switch", { name: /Escalates/ });
		await expect(escalate).toBeDisabled();

		// Raising the ceiling to failed makes escalation meaningful, so the
		// toggle becomes enabled.
		await page.getByRole("combobox").first().click();
		await page
			.getByRole("option", { name: /Failures count in full/ })
			.click();
		await expect(escalate).toBeEnabled();
	});
});
