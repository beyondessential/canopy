import type { Page } from "@playwright/test";
import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedApplicationReport,
	seedMachine,
	seedMachinePackages,
	seedRepositoryPackages,
	seedServer,
	seedServerGroup,
	seedVulnerability,
	seedVulnerabilityDecision,
	seedVulnerabilityLapse,
	seedVulnerabilitySuggestion,
	type Sql,
} from "./seed";

/// The vulnerabilities pages and the packages line on a machine's page.
///
/// spec: VUL, VTR, PKG
async function seedFleet(sql: Sql) {
	const group = await seedServerGroup(sql, { name: "vuln-group" });
	await seedRepositoryPackages(sql, {
		repository: "tamanu",
		packages: [
			{
				name: "axios",
				version: "1.7.4",
				paths: [
					{ workspace: "central-server", direct: true },
					{ workspace: "facility-server" },
					{ workspace: "web", scope: "development", chain: ["vite", "axios"] },
				],
			},
			{
				name: "lodash.merge",
				version: "4.6.1",
				paths: [
					{ workspace: "central-server" },
					{ workspace: "facility-server" },
					{ workspace: "mobile", direct: true },
				],
			},
		],
	});
	await seedRepositoryPackages(sql, {
		repository: "canopy",
		packages: [
			{
				name: "axios",
				version: "1.7.9",
				paths: [{ workspace: "app", scope: "development", direct: true }],
			},
		],
	});
	const application = await seedServer(sql, {
		name: "central-a",
		groupId: group.id,
		type: "tamanu-central",
		rank: "production",
	});
	await seedApplicationReport(sql, {
		applicationId: application.id,
		version: "2.30.2",
	});
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

/// The sidebar entry, whose name carries the count of what needs triage.
function vulnerabilitiesLink(page: Page) {
	return page
		.getByRole("navigation", { name: "Main" })
		.getByRole("link", { name: /^Vulnerabilities/ });
}

const AXIOS_REASON = "central-server calls axios directly, so the vulnerable path is reachable.";

/// Three items need triage: axios in Tamanu (with a suggestion) and in Canopy,
/// and sshd on Linux. The merge item is decided, and the last is closed.
async function seedItems(sql: Sql, opts: { decideMerge?: boolean } = {}) {
	await seedVulnerability(sql, {
		id: "GHSA-axios-0001",
		sources: ["tamanu", "canopy"],
		severity: "high",
		summary: "Server-side request forgery via absolute URL",
		details: "A request built with a relative URL is sent to an absolute one.",
		aliases: ["CVE-2025-27152"],
		matches: [
			{ package: "axios", version: "1.7.4" },
			{ package: "axios", version: "1.7.9" },
		],
		fixed: "1.8.2",
	});
	await seedVulnerabilitySuggestion(sql, {
		id: "GHSA-axios-0001",
		source: "tamanu",
		entries: [{ outcome: "will_fix", reason: AXIOS_REASON }],
		basis: "dependency_path_and_advisory_text",
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
	if (opts.decideMerge ?? true) {
		await seedVulnerabilityDecision(sql, {
			id: "GHSA-merge-0003",
			outcome: "will_fix",
		});
	}
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

	test("is a top-level section of the sidebar, counting what needs triage", async ({
		page,
		sql,
	}) => {
		await seedFleet(sql);
		await seedItems(sql);
		await page.goto("/status");
		await expect(vulnerabilitiesLink(page)).toContainText("3");
		await vulnerabilitiesLink(page).click();
		await expect(page).toHaveURL(/\/vulnerabilities$/);
		await expect(
			page.getByRole("heading", { level: 1, name: "Vulnerabilities" }),
		).toBeVisible();
	});

	test("the sources page shows each source, and a rescan marks one due", async ({ page, sql }) => {
		await seedFleet(sql);
		await seedItems(sql);
		await page.goto("/vulnerabilities");
		await page
			.getByRole("group", { name: "Vulnerabilities pages" })
			.getByRole("link", { name: "Sources" })
			.click();
		await expect(page).toHaveURL(/\/vulnerabilities\/sources$/);

		const tamanu = page.getByRole("row").filter({ hasText: "tamanu" });
		await expect(tamanu).toContainText("beyondessential/tamanu");
		await expect(tamanu).toContainText("2 packages");
		const linux = page.getByRole("row").filter({ hasText: "linux" });
		await expect(linux).toContainText("machine");
		await expect(linux.getByRole("button", { name: "Rescan" })).toHaveCount(0);

		// A repository nothing has read yet is listed, and already due.
		const unread = page.getByRole("row").filter({ hasText: "bliti" });
		await expect(unread).toContainText("0 packages");
		await expect(unread).toContainText("Due");

		await tamanu.getByRole("button", { name: "Rescan" }).click();
		await expect(tamanu).toContainText("Due");
		await expect(tamanu).toContainText("2 packages");
	});

	test("lists what needs triage by severity, with package, scope and suggestion", async ({
		page,
		sql,
	}) => {
		await seedFleet(sql);
		await seedItems(sql);
		await page.goto("/vulnerabilities");

		await expect(page.getByText("3 results")).toBeVisible();
		const rows = page.getByRole("row");
		await expect(rows.nth(1)).toContainText("CVE-2024-6387");
		const linux = rows.filter({ hasText: "CVE-2024-6387" });
		await expect(linux).toContainText("Critical");
		await expect(linux).toContainText("openssh-server 1:9.6p1-3ubuntu13");
		await expect(linux).toContainText("fix in 1:9.6p1-3ubuntu13.4");
		await expect(linux).toContainText("linux");
		await expect(linux).toContainText("1 machine");
		await expect(linux).toContainText("Untriaged");
		await expect(linux).not.toContainText("Suggested");

		const tamanu = rows.filter({ hasText: "GHSA-axios-0001" }).filter({ hasText: "tamanu" });
		await expect(tamanu).toContainText("High");
		await expect(tamanu).toContainText("Server-side request forgery");
		await expect(tamanu).toContainText("axios 1.7.4");
		await expect(tamanu).toContainText("fix in 1.8.2");
		await expect(tamanu).toContainText("development, runtime");
		await expect(tamanu).toContainText("Suggested: will fix");
		await expect(tamanu).toContainText("Based on dependency paths and advisory text");

		const canopy = rows.filter({ hasText: "GHSA-axios-0001" }).filter({ hasText: "canopy" });
		await expect(canopy).toContainText("development");

		await expect(page.getByText("GHSA-merge-0003")).toHaveCount(0);
		await expect(page.getByText("GHSA-gone-0004")).toHaveCount(0);
	});

	test("filters show how many items each choice gives, and the closed switch brings back closed items", async ({
		page,
		sql,
	}) => {
		await seedFleet(sql);
		await seedItems(sql);
		await page.goto("/vulnerabilities");

		await page.getByRole("combobox", { name: "Status" }).click();
		await expect(page.getByRole("option", { name: /^Needs triage\s*3$/ })).toBeVisible();
		await expect(page.getByRole("option", { name: /^Will fix\s*1$/ })).toBeVisible();
		await page.getByRole("option", { name: /^Will fix/ }).click();
		const decided = page.getByRole("row").filter({ hasText: "GHSA-merge-0003" });
		await expect(decided).toContainText("Will fix");
		await expect(page.getByText("1 result")).toBeVisible();

		await page.getByRole("combobox", { name: "Status" }).click();
		await page.getByRole("option", { name: /^All/ }).click();
		await expect(page.getByText("4 results")).toBeVisible();
		await expect(page.getByText("GHSA-gone-0004")).toHaveCount(0);

		await page.getByLabel("Show closed").click();
		await expect(page.getByLabel("Show closed")).toBeChecked();
		await expect(page.getByText("5 results")).toBeVisible();
		const gone = page.getByRole("row").filter({ hasText: "GHSA-gone-0004" });
		await expect(gone).toContainText("Closed");

		await page.getByRole("combobox", { name: "Source" }).click();
		await expect(page.getByRole("option", { name: /^canopy\s*1$/ })).toBeVisible();
		await page.getByRole("option", { name: /^linux/ }).click();
		await expect(page.getByText("1 result")).toBeVisible();
		await expect(page.getByText("CVE-2024-6387")).toBeVisible();

		await page.getByRole("combobox", { name: "Source" }).click();
		await page.getByRole("option", { name: /^All/ }).click();
		await page.getByRole("combobox", { name: "Scope" }).click();
		await page.getByRole("option", { name: /^Development/ }).click();
		await expect(page.getByText("GHSA-axios-0001").first()).toBeVisible();
		await expect(page.getByText("CVE-2024-6387")).toHaveCount(0);
	});

	test("a search matches partial words and marks the match", async ({ page, sql }) => {
		await seedFleet(sql);
		await seedItems(sql);
		await page.goto("/vulnerabilities");

		await page.getByRole("textbox", { name: "Search" }).fill("axios");
		await expect(page).toHaveURL(/q=axios/);
		await expect(page.getByText("2 results")).toBeVisible();
		await expect(page.locator("mark").first()).toHaveText(/axios/i);
		await expect(page.getByText("CVE-2024-6387")).toHaveCount(0);

		await page.getByRole("textbox", { name: "Search" }).fill("ssh");
		await expect(page.getByText("1 result")).toBeVisible();
		await expect(page.getByText("CVE-2024-6387")).toBeVisible();

		await page.getByRole("button", { name: "Clear search" }).click();
		await expect(page.getByText("3 results")).toBeVisible();
	});

	test("every column sorts, and the choice is in the address", async ({ page, sql }) => {
		await seedFleet(sql);
		await seedItems(sql);
		await page.goto("/vulnerabilities");
		await expect(page.getByRole("row").nth(1)).toContainText("CVE-2024-6387");

		await page.getByRole("columnheader", { name: "Source" }).getByRole("button").click();
		await expect(page).toHaveURL(/sort=source/);
		await expect(page.getByRole("row").nth(1)).toContainText("canopy");

		await page.getByRole("columnheader", { name: "Source" }).getByRole("button").click();
		await expect(page).toHaveURL(/dir=desc/);
		await expect(page.getByRole("row").nth(1)).toContainText("tamanu");

		await page.getByRole("columnheader", { name: "Package" }).getByRole("button").click();
		await expect(page).toHaveURL(/sort=package/);
		await expect(page.getByRole("row").nth(1)).toContainText("axios");
	});

	test("a repository item lists affected packages with how they are reached", async ({
		page,
		sql,
	}) => {
		await seedFleet(sql);
		await seedItems(sql);
		await page.goto("/vulnerabilities");
		await page
			.getByRole("row")
			.filter({ hasText: "GHSA-axios-0001" })
			.filter({ hasText: "tamanu" })
			.getByRole("link", { name: /Server-side request forgery/ })
			.click();

		await expect(
			page.getByRole("heading", {
				level: 1,
				name: "Server-side request forgery via absolute URL",
			}),
		).toBeVisible();
		await expect(page.getByText("GHSA-axios-0001 · CVE-2025-27152")).toBeVisible();
		await expect(page.getByText("A request built with a relative URL")).toBeVisible();
		await expect(page.getByText(/Fixed in 1\.8\.2/)).toBeVisible();
		await expect(page.getByText(/read at commit 3f9c2ab/)).toBeVisible();

		const central = page.getByRole("row").filter({ hasText: "central-server" }).first();
		await expect(central).toContainText("axios 1.7.4");
		await expect(central).toContainText("runtime, direct");
		await expect(central).toContainText("1.8.2");
		const web = page.getByRole("row").filter({ hasText: "web" });
		await expect(web).toContainText("development");
		await expect(web).toContainText("via vite");
	});

	test("a Linux item lists exposed machines with group, installed version and fix", async ({
		page,
		sql,
	}) => {
		const { machine } = await seedFleet(sql);
		await seedItems(sql);
		await page.goto("/vulnerabilities/linux/CVE-2024-6387");

		const row = page.getByRole("row").filter({ hasText: "ssh-box" });
		await expect(row.getByRole("link", { name: "vuln-group" })).toBeVisible();
		await expect(row).toContainText("openssh-server 1:9.6p1-3ubuntu13");
		await expect(row).toContainText("1:9.6p1-3ubuntu13.4");

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
		await expect(
			page.getByText(/Based on dependency paths and advisory text\. Made without access to source code\./),
		).toBeVisible();
		await expect(page.getByRole("button", { name: "Will fix" })).toHaveAttribute(
			"aria-pressed",
			"true",
		);
		await expect(page.getByLabel("Decision reason")).toHaveValue(AXIOS_REASON);
		await page.getByLabel("Decision issue link").fill("https://example.com/issues/1");

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
		await expect(page.getByRole("row").filter({ hasText: "GHSA-axios-0001" })).toHaveCount(1);
		await expect(vulnerabilitiesLink(page)).toContainText("2");
	});

	test("a split decision covers every package before it can be recorded, and reads as mixed", async ({
		page,
		sql,
	}) => {
		await seedFleet(sql);
		await seedItems(sql, { decideMerge: false });
		await page.goto("/vulnerabilities/tamanu/GHSA-merge-0003");

		await page.getByRole("button", { name: "Split by package" }).click();
		const record = page.getByRole("button", { name: "Record" });
		await expect(page.getByText("0 of 3 packages covered")).toBeVisible();
		await expect(record).toBeDisabled();

		await page.getByRole("combobox", { name: "Outcome of mobile" }).click();
		await page.getByRole("option", { name: "Will fix" }).click();
		await expect(page.getByText("1 of 3 packages covered")).toBeVisible();

		await page.getByRole("combobox", { name: "Outcome of everything else" }).click();
		await page.getByRole("option", { name: "Not affected" }).click();
		await expect(record).toBeDisabled();
		await page.getByLabel("Everything else reason").fill("Only called with data we build ourselves.");
		await expect(page.getByText("All 3 packages covered")).toBeVisible();
		await expect(record).toBeEnabled();
		await record.click();

		await expect(
			page.getByRole("listitem").filter({ hasText: "Decided mixed" }),
		).toContainText("Only called with data we build ourselves.");
		await expect(page.getByRole("listitem").filter({ hasText: "Decided mixed" })).toContainText("mobile");

		await page.goto("/vulnerabilities?status=mixed");
		await expect(
			page.getByRole("row").filter({ hasText: "GHSA-merge-0003" }),
		).toContainText("Mixed");
	});

	test("an accepted risk needs a reason and replaces the decision before it", async ({
		page,
		sql,
	}) => {
		await seedFleet(sql);
		await seedItems(sql);
		await page.goto("/vulnerabilities/tamanu/GHSA-merge-0003");

		await page.getByRole("button", { name: "Accepted risk" }).click();
		const record = page.getByRole("button", { name: "Record" });
		await expect(record).toBeDisabled();
		await page.getByLabel("Decision reason").fill("Only reachable from the admin tool");
		await expect(page.getByLabel("Review by")).toHaveCount(0);
		await expect(record).toBeEnabled();
		await record.click();

		await expect(
			page.getByRole("listitem").filter({ hasText: "Decided accepted risk" }),
		).toContainText("Only reachable from the admin tool");
		await expect(
			page.getByRole("listitem").filter({ hasText: "Decided will fix" }),
		).toBeVisible();
	});

	test("a decision can be edited and withdrawn, and the history shows both", async ({
		page,
		sql,
	}) => {
		await seedFleet(sql);
		await seedItems(sql, { decideMerge: false });
		await seedVulnerabilityDecision(sql, {
			id: "GHSA-merge-0003",
			outcome: "accepted_risk",
			reason: "Old reason",
		});
		await page.goto("/vulnerabilities/tamanu/GHSA-merge-0003");

		await page.getByRole("button", { name: "Edit" }).click();
		await page.getByLabel("Entry 1 reason").fill("Only reachable by admins on the VPN");
		await page.getByRole("button", { name: "Save" }).click();

		const edited = page.getByRole("listitem").filter({ hasText: "Edited the reason" });
		await expect(edited).toContainText("Only reachable by admins on the VPN");
		await expect(edited.getByText("Old reason")).toHaveCSS("text-decoration-line", "line-through");
		await expect(page.getByText("The decision was edited since it was made.")).toBeVisible();

		await page.getByRole("button", { name: "Withdraw" }).click();
		await page.getByRole("dialog").getByLabel("Reason").fill("Needs another look");
		await page.getByRole("button", { name: "Withdraw" }).last().click();

		await expect(
			page.getByRole("listitem").filter({ hasText: "Withdrew the decision" }),
		).toContainText("Needs another look");
		await expect(page.getByText("The previous decision was withdrawn.")).toBeVisible();

		await page.goto("/vulnerabilities");
		const row = page.getByRole("row").filter({ hasText: "GHSA-merge-0003" });
		await expect(row).toContainText("Untriaged");
		await expect(row).toContainText("Withdrawn");
	});

	test("names the model and a caller through the query interface in the history", async ({
		page,
		sql,
	}) => {
		await seedFleet(sql);
		await seedItems(sql, { decideMerge: false });
		await seedVulnerabilityDecision(sql, {
			id: "GHSA-merge-0003",
			outcome: "not_affected",
			via: "model",
			actor: { kind: "canopy", job: "ai_worker" },
		});
		await seedVulnerabilityDecision(sql, {
			id: "GHSA-merge-0003",
			outcome: "will_fix",
			via: "mcp",
			actor: { kind: "token", id: "00000000-0000-0000-0000-000000000001", name: "triage-agent" },
		});
		await seedVulnerabilityDecision(sql, { id: "GHSA-merge-0003", outcome: "will_fix" });
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

	test("a lapsed decision says why and prefills the panel from it", async ({ page, sql }) => {
		await seedFleet(sql);
		await seedItems(sql, { decideMerge: false });
		await seedVulnerabilityDecision(sql, {
			id: "GHSA-merge-0003",
			entries: [
				{ workspacePackages: ["mobile"], outcome: "will_fix" },
				{ outcome: "not_affected", reason: "Not reached from shipped code." },
			],
			coveredHeld: ["central-server", "facility-server", "mobile"],
		});
		await seedVulnerabilityLapse(sql, { id: "GHSA-merge-0003" });
		await page.goto("/vulnerabilities/tamanu/GHSA-merge-0003");

		await expect(
			page.getByText("Decision lapsed: Severity rose from medium to critical"),
		).toBeVisible();
		await expect(page.getByText("Untriaged").first()).toBeVisible();
		await expect(page.getByRole("button", { name: "Split by package" })).toHaveAttribute(
			"aria-pressed",
			"true",
		);
		await expect(page.getByLabel("Everything else reason")).toHaveValue(
			"Not reached from shipped code.",
		);
		await expect(
			page.getByRole("listitem").filter({ hasText: "Decision lapsed" }),
		).toContainText("Canopy");
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
