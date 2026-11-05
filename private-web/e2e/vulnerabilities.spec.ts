import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedApplicationReport,
	seedMachine,
	seedMachinePackages,
	seedServer,
	seedServerGroup,
	seedTamanuDependencies,
	seedVersion,
	seedVulnerability,
	seedVulnerabilityDecision,
	seedVulnerabilitySuggestion,
	type Sql,
} from "./seed";
import { sidebarLink } from "./nav";

/// The vulnerabilities pages and the packages line on a machine's page.
///
/// spec: VUL, VTR, PKG
async function seedFleet(sql: Sql) {
	const group = await seedServerGroup(sql, { name: "vuln-group" });
	// The newest published version has nothing running it; the older one has
	// two production applications.
	const older = await seedVersion(sql, { major: 2, minor: 30, patch: 2 });
	const newest = await seedVersion(sql, { major: 2, minor: 31, patch: 4 });
	// Neither the newest nor run: it counts for nothing.
	const idle = await seedVersion(sql, { major: 2, minor: 29, patch: 6 });
	await seedTamanuDependencies(sql, {
		versionId: older.id,
		packages: [
			{
				name: "axios",
				version: "1.7.4",
				workspaces: ["central-server", "facility-server"],
				direct: true,
			},
			{ name: "lodash.merge", version: "4.6.1" },
		],
	});
	await seedTamanuDependencies(sql, {
		versionId: newest.id,
		packages: [
			{
				name: "axios",
				version: "1.7.9",
				workspaces: ["central-server", "facility-server"],
			},
		],
	});
	await seedTamanuDependencies(sql, {
		versionId: idle.id,
		packages: [{ name: "axios", version: "1.6.0", workspaces: ["web"] }],
	});
	for (const name of ["central-a", "central-b"]) {
		const application = await seedServer(sql, {
			name,
			groupId: group.id,
			type: "tamanu-central",
			rank: "production",
		});
		await seedApplicationReport(sql, {
			applicationId: application.id,
			version: "2.30.2",
		});
	}
	const machine = await seedMachine(sql, {
		name: "ssh-box",
		groupId: group.id,
		rank: "production",
	});
	await seedMachinePackages(sql, {
		machineId: machine.id,
		packages: [
			{ name: "openssh-server", version: "1:9.6p1-3ubuntu13" },
			{ name: "curl", version: "8.5.0-2ubuntu10" },
		],
	});
	return { group, machine };
}

async function seedItems(sql: Sql) {
	await seedVulnerability(sql, {
		id: "GHSA-axios-0001",
		source: "tamanu",
		severity: "high",
		summary: "Server-side request forgery via absolute URL",
		details: "A request built with a relative URL is sent to an absolute one.",
		aliases: ["CVE-2025-27152"],
		matches: [
			{ package: "axios", version: "1.7.4" },
			{ package: "axios", version: "1.7.9" },
			{ package: "axios", version: "1.6.0" },
		],
		fixed: "1.8.2",
	});
	await seedVulnerabilitySuggestion(sql, {
		id: "GHSA-axios-0001",
		source: "tamanu",
		outcome: "will_fix",
		reason: "central-server calls axios directly, so the vulnerable path is reachable.",
	});
	await seedVulnerability(sql, {
		id: "CVE-2024-6387",
		source: "linux",
		severity: "critical",
		summary: "Signal handler race in sshd",
		matches: [{ package: "openssh-server", version: "1:9.6p1-3ubuntu13" }],
		fixed: "1:9.6p1-3ubuntu13.4",
	});
	await seedVulnerability(sql, {
		id: "GHSA-merge-0003",
		source: "tamanu",
		severity: "medium",
		summary: "Prototype pollution in nested merge",
		matches: [{ package: "lodash.merge", version: "4.6.1" }],
	});
	await seedVulnerabilityDecision(sql, {
		id: "GHSA-merge-0003",
		source: "tamanu",
		outcome: "will_fix",
	});
	await seedVulnerability(sql, {
		id: "GHSA-gone-0004",
		source: "tamanu",
		severity: "low",
		summary: "Advisory nothing counts for",
		matches: [{ package: "left-pad", version: "1.0.0" }],
	});
}

test.describe("vulnerabilities", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("is a top-level section of the sidebar", async ({ page }) => {
		await page.goto("/status");
		await sidebarLink(page, "Vulnerabilities").click();
		await expect(page).toHaveURL(/\/vulnerabilities$/);
		await expect(
			page.getByRole("heading", { level: 1, name: "Vulnerabilities" }),
		).toBeVisible();
	});

	test("lists open items by severity, with exposure and the suggested outcome", async ({
		page,
		sql,
	}) => {
		await seedFleet(sql);
		await seedItems(sql);
		await page.goto("/vulnerabilities");

		const rows = page.getByRole("row");
		await expect(rows.nth(1)).toContainText("CVE-2024-6387");
		const linux = rows.filter({ hasText: "CVE-2024-6387" });
		await expect(linux).toContainText("Critical");
		await expect(linux).toContainText("Signal handler race in sshd");
		await expect(linux).toContainText("openssh-server");
		await expect(linux).toContainText("Linux");
		await expect(linux).toContainText("1 machine");
		await expect(linux).toContainText("Open");
		await expect(linux).not.toContainText("Suggested");

		await expect(rows.nth(2)).toContainText("GHSA-axios-0001");
		const tamanu = rows.filter({ hasText: "GHSA-axios-0001" });
		await expect(tamanu).toContainText("High");
		await expect(tamanu).toContainText("Server-side request forgery");
		await expect(tamanu).toContainText("axios");
		await expect(tamanu).toContainText("Tamanu");
		await expect(tamanu).toContainText("2 applications");
		await expect(tamanu).toContainText("Suggested: will fix");

		await expect(page.getByText("GHSA-merge-0003")).toHaveCount(0);
		await expect(page.getByText("GHSA-gone-0004")).toHaveCount(0);
	});

	test("filters by source and status, and includes uncounted items", async ({
		page,
		sql,
	}) => {
		await seedFleet(sql);
		await seedItems(sql);
		await page.goto("/vulnerabilities");

		await page.getByRole("combobox", { name: "Source" }).click();
		await page.getByRole("option", { name: "Linux" }).click();
		await expect(page.getByText("CVE-2024-6387")).toBeVisible();
		await expect(page.getByText("GHSA-axios-0001")).toHaveCount(0);

		await page.getByRole("combobox", { name: "Source" }).click();
		await page.getByRole("option", { name: "All" }).click();
		await page.getByRole("combobox", { name: "Status" }).click();
		await page.getByRole("option", { name: "Will fix" }).click();
		const decided = page.getByRole("row").filter({ hasText: "GHSA-merge-0003" });
		await expect(decided).toContainText("Will fix");
		await expect(decided).toContainText("2 applications");
		await expect(page.getByText("GHSA-axios-0001")).toHaveCount(0);

		await page.getByRole("combobox", { name: "Status" }).click();
		await page.getByRole("option", { name: "All" }).click();
		await expect(page.getByText("GHSA-axios-0001")).toBeVisible();
		await expect(page.getByText("GHSA-merge-0003")).toBeVisible();
		await expect(page.getByText("GHSA-gone-0004")).toHaveCount(0);

		await page.getByLabel("Show all").click();
		await expect(page.getByLabel("Show all")).toBeChecked();
		const gone = page.getByRole("row").filter({ hasText: "GHSA-gone-0004" });
		await expect(gone).toContainText("Not counted");
	});

	test("a Tamanu item lists exposed versions with workspaces and applications", async ({
		page,
		sql,
	}) => {
		await seedFleet(sql);
		await seedItems(sql);
		await page.goto("/vulnerabilities");
		await page.getByRole("link", { name: /Server-side request forgery/ }).click();

		await expect(
			page.getByRole("heading", {
				level: 1,
				name: "Server-side request forgery via absolute URL",
			}),
		).toBeVisible();
		await expect(page.getByText("GHSA-axios-0001 · CVE-2025-27152")).toBeVisible();
		await expect(page.getByText("A request built with a relative URL")).toBeVisible();
		await expect(page.getByText(/Fixed in 1\.8\.2/)).toBeVisible();

		const run = page.getByRole("row").filter({ hasText: "2.30.2" });
		await expect(run).toContainText("axios 1.7.4");
		await expect(run).toContainText("central-server, facility-server");
		await expect(run).toContainText("Direct");
		await expect(run).not.toContainText("Newest published");
		await expect(run.getByRole("cell").last()).toHaveText("2");

		const newest = page.getByRole("row").filter({ hasText: "2.31.4" });
		await expect(newest).toContainText("axios 1.7.9");
		await expect(newest).toContainText("Newest published");
		await expect(newest.getByRole("cell").last()).toHaveText("0");

		await expect(page.getByRole("row").filter({ hasText: "2.29.6" })).toHaveCount(0);
	});

	test("a Linux item lists exposed machines with group and installed version", async ({
		page,
		sql,
	}) => {
		const { machine } = await seedFleet(sql);
		await seedItems(sql);
		await page.goto("/vulnerabilities/linux/CVE-2024-6387");

		const row = page.getByRole("row").filter({ hasText: "ssh-box" });
		await expect(row.getByRole("link", { name: "vuln-group" })).toBeVisible();
		await expect(row).toContainText("openssh-server 1:9.6p1-3ubuntu13");

		await row.getByRole("link", { name: "ssh-box" }).click();
		await expect(page).toHaveURL(new RegExp(`/fleet/machines/${machine.id}$`));
	});

	test("the decision panel is prefilled from the suggestion and records it in one step", async ({
		page,
		sql,
	}) => {
		await seedFleet(sql);
		await seedItems(sql);
		await page.goto("/vulnerabilities/tamanu/GHSA-axios-0001");

		await expect(page.getByText("AI suggestion")).toBeVisible();
		await expect(page.getByRole("button", { name: "Will fix" })).toHaveAttribute(
			"aria-pressed",
			"true",
		);
		const reason = "central-server calls axios directly, so the vulnerable path is reachable.";
		await expect(page.getByLabel("Reason")).toHaveValue(reason);
		await page.getByLabel("Issue link").fill("https://example.com/issues/1");

		await page.getByRole("button", { name: "Record" }).click();

		await expect(page.getByRole("heading", { name: "History" })).toBeVisible();
		const history = page.getByRole("listitem").filter({ hasText: "Decided will fix" });
		await expect(history).toContainText("admin@localhost");
		await expect(history).toContainText("operator interface");
		await expect(history).toContainText("https://example.com/issues/1");
		await expect(
			page.getByRole("listitem").filter({ hasText: "Suggested will fix" }),
		).toContainText("AI");
		await expect(page.getByText("AI suggestion")).toHaveCount(0);

		await page.goto("/vulnerabilities");
		await expect(page.getByText("CVE-2024-6387")).toBeVisible();
		await expect(page.getByText("GHSA-axios-0001")).toHaveCount(0);
	});

	test("an accepted risk needs a reason and a review date, and replaces the decision before it", async ({
		page,
		sql,
	}) => {
		await seedFleet(sql);
		await seedItems(sql);
		await page.goto("/vulnerabilities/tamanu/GHSA-merge-0003");

		await page.getByRole("button", { name: "Accepted risk" }).click();
		const record = page.getByRole("button", { name: "Record" });
		await page.getByLabel("Reason").fill("Only reachable from the admin tool");
		await expect(record).toBeDisabled();
		const review = new Date();
		review.setFullYear(review.getFullYear() + 1);
		await page.getByLabel("Review by").fill(review.toLocaleDateString("en-CA"));
		await expect(record).toBeEnabled();
		await record.click();

		await expect(
			page.getByRole("listitem").filter({ hasText: "Decided accepted risk" }),
		).toContainText("Only reachable from the admin tool");
		await expect(
			page.getByRole("listitem").filter({ hasText: "Decided will fix" }),
		).toBeVisible();
		await expect(page.getByText("Accepted risk").first()).toBeVisible();
	});

	test("names the model and a caller through the query interface in the history", async ({
		page,
		sql,
	}) => {
		await seedFleet(sql);
		await seedItems(sql);
		await seedVulnerabilityDecision(sql, {
			id: "GHSA-merge-0003",
			source: "tamanu",
			outcome: "not_affected",
			via: "model",
			actor: { kind: "canopy", job: "ai_worker" },
		});
		await seedVulnerabilityDecision(sql, {
			id: "GHSA-merge-0003",
			source: "tamanu",
			outcome: "will_fix",
			via: "mcp",
			actor: { kind: "token", id: "00000000-0000-0000-0000-000000000001", name: "triage-agent" },
		});
		await page.goto("/vulnerabilities/tamanu/GHSA-merge-0003");

		await expect(
			page.getByRole("listitem").filter({ hasText: "Decided not affected" }),
		).toContainText("AI");
		await expect(
			page.getByRole("listitem").filter({ hasText: "triage-agent" }),
		).toContainText("fleet query interface");
		await expect(
			page.getByRole("listitem").filter({ hasText: "seed@example.com" }),
		).toContainText("operator interface");
	});

	test("a reopened item says why", async ({ page, sql }) => {
		await seedFleet(sql);
		await seedItems(sql);
		await seedVulnerabilityDecision(sql, {
			id: "GHSA-merge-0003",
			source: "tamanu",
			outcome: "accepted_risk",
			reviewBy: "2020-01-01",
		});
		await page.goto("/vulnerabilities/tamanu/GHSA-merge-0003");

		await expect(page.getByText("Reopened: Review date passed")).toBeVisible();
		await expect(page.getByText("Open", { exact: true }).first()).toBeVisible();
	});
});

test.describe("machine packages line", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("shows how many packages the machine reported and when", async ({
		page,
		sql,
	}) => {
		const { machine } = await seedFleet(sql);
		await page.goto(`/fleet/machines/${machine.id}`);

		await expect(page.getByText(/2 packages reported/)).toBeVisible();
		await expect(page.getByText(/Not checked/)).toHaveCount(0);
	});

	test("flags a report that could not be placed", async ({ page, sql }) => {
		const machine = await seedMachine(sql, { name: "odd-box", rank: "production" });
		await seedMachinePackages(sql, {
			machineId: machine.id,
			distribution: "gentoo",
			release: "2.15",
			ecosystem: null,
			packages: [{ name: "bash", version: "5.2" }],
		});
		await page.goto(`/fleet/machines/${machine.id}`);

		await expect(page.getByText(/1 package reported/)).toBeVisible();
		await expect(page.getByText("Not checked: gentoo 2.15 is not a recognised release")).toBeVisible();
	});

	test("shows no line for a machine that has not reported", async ({ page, sql }) => {
		const machine = await seedMachine(sql, { name: "quiet-box" });
		await page.goto(`/fleet/machines/${machine.id}`);

		await expect(page.getByRole("heading", { level: 1, name: /quiet-box/ })).toBeVisible();
		await expect(page.getByText(/packages? reported/)).toHaveCount(0);
	});
});
