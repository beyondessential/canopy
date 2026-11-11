import { type Locator, type Page } from "@playwright/test";
import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedIncident,
	seedIncidentNote,
	seedIssue,
	seedMachine,
	seedMemoryEntry,
	seedNotesRevision,
	seedServer,
	seedServerGroup,
} from "./seed";

/** Markdown with a hard-wrapped second line and an image whose error handler
 * would set `window.__pwned` if inline HTML were let through unsanitised. */
function markdown(label: string): string {
	return `**${label}** is in [the vault](https://vault.example.com)\nsecond ${label} line\n<img src="missing.png" onerror="window.__pwned = 1">`;
}

async function expectRendered(page: Page, scope: Locator, label: string) {
	await expect(scope.locator("strong", { hasText: label })).toBeVisible();
	await expect(scope.getByRole("link", { name: "the vault" }).first()).toHaveAttribute(
		"href",
		"https://vault.example.com",
	);
	await expect(scope.getByText(`**${label}**`)).toHaveCount(0);
	// Each single line break is kept rather than joined into the paragraph.
	await expect(
		scope.locator("p", { hasText: `second ${label} line` }).first().locator("br"),
	).toHaveCount(2);
	await expect(scope.locator("img[onerror]")).toHaveCount(0);
	expect(await page.evaluate(() => (window as { __pwned?: number }).__pwned)).toBeUndefined();
}

test.describe("notes and memory entries render as markdown", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("a group's notes and their history", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, {
			name: "md-group",
			notes: markdown("Current"),
		});
		await seedNotesRevision(sql, {
			groupId: group.id,
			revision: 1,
			notes: markdown("Earlier"),
			author: "operator@example.com",
			via: "operator",
		});

		await page.goto(`/fleet/groups/${group.id}`);
		await expectRendered(page, page.locator("main"), "Current");
		await page.getByRole("button", { name: "History (1)" }).click();
		await expectRendered(page, page.locator("main"), "Earlier");
	});

	test("a machine's notes", async ({ page, sql }) => {
		const machine = await seedMachine(sql, { name: "md-box" });
		await sql.query("UPDATE machines SET notes = $1 WHERE id = $2", [
			markdown("Box"),
			machine.id,
		]);

		await page.goto(`/fleet/machines/${machine.id}`);
		await expectRendered(page, page.locator("main"), "Box");
	});

	test("an application's notes and its group's", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, {
			name: "md-app-group",
			notes: markdown("Group"),
		});
		const app = await seedServer(sql, {
			groupId: group.id,
			notes: markdown("App"),
		});

		await page.goto(`/fleet/applications/${app.id}`);
		await expectRendered(page, page.locator("main"), "App");
		await expectRendered(page, page.locator("main"), "Group");
	});

	test("an incident's notes", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "md-incident-group" });
		const incident = await seedIncident(sql, { serverGroupId: group.id });
		await seedIncidentNote(sql, { incidentId: incident.id, body: markdown("Note") });

		await page.goto(`/incidents/${incident.id}`);
		await expectRendered(page, page.locator("main"), "Note");
	});

	test("an issue's notes", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "md-issue-group" });
		const issue = await seedIssue(sql, {
			serverGroupId: group.id,
			message: "md issue",
		});
		await sql.query(
			"INSERT INTO issue_notes (issue_id, author, body) VALUES ($1, $2, $3)",
			[issue.id, "operator@example.com", markdown("Issue")],
		);

		await page.goto("/incidents?showAll=1");
		await page.getByRole("button", { name: /^expand$/i }).first().click();
		await expectRendered(page, page.locator("main"), "Issue");
	});

	test("a memory entry's body", async ({ page, sql }) => {
		await seedMemoryEntry(sql, { title: "Vault key", body: markdown("Entry") });

		await page.goto("/incidents/memory");
		await page.getByText("Vault key").click();
		await expectRendered(page, page.locator("main"), "Entry");
	});
});
