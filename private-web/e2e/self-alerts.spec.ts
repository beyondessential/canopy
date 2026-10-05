import { resetSeededTables, seedIssue } from "./seed";
import { expect, test } from "./test-fixtures";

const NIL = "00000000-0000-0000-0000-000000000000";

// Self-alerts (canopy-wide issues, scoped to neither server nor group)
// get their own surface: a banner on every page and the /alerts view.
// They must NOT appear in the fleet issue listing on /incidents.
test.describe("self-alerts", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("active alert banners on any page and lists on /alerts", async ({
		page,
		sql,
	}) => {
		await seedIssue(sql, {
			source: "canopy",
			ref: "mcp-token-expiry",
			severity: "error",
			description: "MCP access token nearing expiry",
			message: "token claude expires in 10 days",
		});

		await page.goto("/status");
		const banner = page.getByText("Canopy: MCP access token nearing expiry");
		await expect(banner).toBeVisible();

		await page.getByRole("link", { name: "details" }).click();
		await expect(page).toHaveURL(/\/alerts$/);
		await expect(
			page.getByText("token claude expires in 10 days"),
		).toBeVisible();
	});

	/// The stale-healthcheck alert asks for a decommission, and decommissioning
	/// lives on a check's own policy page. Resolving the alert does nothing
	/// while the condition holds, so the alert has to lead somewhere an
	/// operator can act — and it has to name the whole identity, since a bare
	/// name can belong to several catalog entries.
	///
	/// spec: SELF#presentation
	test("an alert naming checks links each one to its policy", async ({
		page,
		sql,
	}) => {
		await seedIssue(sql, {
			source: "canopy",
			ref: "stale-healthchecks",
			severity: "warning",
			description: "Healthchecks gone quiet",
			message:
				"1 healthcheck(s) unreported fleet-wide for 30 days: alertd/tamanu-central:db_version",
			detail: {
				checks: [
					{
						source: "alertd",
						check: "db_version",
						qualified_name: "tamanu-central:db_version",
						subject: "application",
						application_type: "tamanu-central",
					},
				],
			},
		});

		await page.goto("/alerts");

		const link = page.getByRole("link", {
			name: "alertd/tamanu-central:db_version",
		});
		await expect(link).toBeVisible();
		await link.click();

		// The policy page for that one entry, namespace and all.
		await expect(page).toHaveURL(
			/\/settings\/healthchecks\/alertd\/application\.tamanu-central\/db_version$/,
		);
	});

	test("self-alerts stay out of the fleet issue listing", async ({
		page,
		sql,
	}) => {
		await seedIssue(sql, {
			source: "canopy",
			ref: "preflight-identity",
			severity: "critical",
			description: "Canopy IRSA identity broken",
			message: "sts:GetCallerIdentity failed",
		});

		await page.goto("/incidents?showAll=1");
		await expect(
			page.getByText("sts:GetCallerIdentity failed"),
		).not.toBeVisible();
		// And no link to the hidden nil-server page anywhere.
		await expect(page.locator(`a[href="/fleet/fleet/applications/${NIL}"]`)).toHaveCount(0);
	});

	test("resolve clears the banner", async ({ page, sql }) => {
		await seedIssue(sql, {
			source: "canopy",
			ref: "slack-delivery-failure",
			severity: "error",
			description: "Slack delivery permanently failed",
			message: "outbox row gave up after 10 attempts",
		});

		await page.goto("/alerts");
		await expect(
			page.getByText("outbox row gave up after 10 attempts"),
		).toBeVisible();
		await page.getByRole("button", { name: "Resolve" }).click();

		await expect(page.getByText("No active alerts.")).toBeVisible();
		await expect(
			page.getByText("Canopy: Slack delivery permanently failed"),
		).not.toBeVisible();
		// The resolved alert stays visible as history.
		await expect(page.getByText("Recovered and resolved")).toBeVisible();
	});
});
