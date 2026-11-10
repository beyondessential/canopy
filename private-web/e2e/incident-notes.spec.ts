import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedIncident,
	seedIncidentNote,
	seedServerGroup,
} from "./seed";

test.describe("incident notes", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("an agent's note is marked, and deleted notes show only when an administrator asks", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "notes-group" });
		const incident = await seedIncident(sql, { serverGroupId: group.id });
		await seedIncidentNote(sql, {
			incidentId: incident.id,
			author: "scribe",
			agent: true,
			body: "disk is nearly full",
		});
		await seedIncidentNote(sql, {
			incidentId: incident.id,
			body: "paged the DBA",
			deletedBy: "someone@example.com",
		});

		await page.goto(`/incidents/${incident.id}`);
		await expect(page.getByText("disk is nearly full")).toBeVisible();
		await expect(page.getByText("Agent", { exact: true })).toBeVisible();
		await expect(page.getByText("paged the DBA")).toHaveCount(0);

		await page.getByLabel("Show deleted notes").check();
		await expect(page.getByText("paged the DBA")).toBeVisible();
		await expect(page.getByText(/Deleted by someone@example.com/)).toBeVisible();
	});

	test("an administrator deletes a note, which leaves the timeline but stays in the record", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "delete-group" });
		const incident = await seedIncident(sql, { serverGroupId: group.id });
		await seedIncidentNote(sql, {
			incidentId: incident.id,
			body: "wrong guess",
		});

		await page.goto(`/incidents/${incident.id}`);
		await page.getByRole("button", { name: "Delete note" }).click();
		await expect(page.getByText("wrong guess")).toHaveCount(0);

		const rows = await sql.query<{ deleted_by: string | null }>(
			"SELECT deleted_by FROM incident_notes WHERE incident_id = $1",
			[incident.id],
		);
		expect(rows).toHaveLength(1);
		expect(rows[0]!.deleted_by).toBeTruthy();
	});
});
