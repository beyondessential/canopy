import {
	resetSeededTables,
	seedGroupSilencedRef,
	seedServer,
	seedServerGroup,
	seedStatus,
	type Sql,
} from "./seed";
import { expect, test } from "./test-fixtures";

/** The ceiling stored for one catalog entry, addressed by its namespace so a
 * same-named entry in another namespace cannot answer for it. */
async function ceilingOf(
	sql: Sql,
	source: string,
	applicationType: string | null,
	checkName: string,
): Promise<string | undefined> {
	const rows = await sql.query<{ ceiling: string }>(
		`SELECT ceiling FROM check_policies
		 WHERE source = $1 AND check_name = $2
		   AND application_type IS NOT DISTINCT FROM $3`,
		[source, checkName, applicationType],
	);
	return rows[0]?.ceiling;
}

test.describe("The check namespace in the catalog", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	/// Two application types reporting the same name are two checks, so they
	/// are two entries an operator grades apart. A central's postgres and a
	/// facility's postgres fail for different reasons and at different costs.
	///
	/// spec: CHK
	test("one name from two application types is two entries, graded apart", async ({
		page,
		sql,
	}) => {
		const central = await seedServer(sql, {
			name: "ns-central",
			type: "tamanu-central",
		});
		const facility = await seedServer(sql, {
			name: "ns-facility",
			type: "tamanu-facility",
		});
		for (const server of [central, facility]) {
			await seedStatus(sql, {
				serverId: server.id,
				health: [{ check: "postgres", result: "failed" }],
			});
		}

		await page.goto("/settings/healthchecks");

		// Each reads qualified by the type that reports it.
		const centralRow = page.getByRole("row", {
			name: /tamanu-central:postgres/,
		});
		const facilityRow = page.getByRole("row", {
			name: /tamanu-facility:postgres/,
		});
		await expect(centralRow).toBeVisible();
		await expect(facilityRow).toBeVisible();

		// Raising the central's ceiling is a decision about the central alone.
		await centralRow.getByRole("combobox").click();
		// Each option is a result chip whose tooltip supplies the accessible
		// name, so the option is picked by the label the operator reads.
		await page
			.getByRole("listbox")
			.getByText("failed", { exact: true })
			.click();
		await centralRow.getByRole("button", { name: "Save" }).click();

		await expect
			.poll(() => ceilingOf(sql, "alertd", "tamanu-central", "postgres"))
			.toBe("failed");
		expect(await ceilingOf(sql, "alertd", "tamanu-facility", "postgres")).toBe(
			"warning",
		);
	});

	/// A check that describes the box is one check however many workloads sit
	/// on it, so it catalogues once and reads by its bare name: qualifying it
	/// by a reporter's type would claim a distinction that isn't there.
	///
	/// spec: CHK
	test("a machine-subject name is one entry, whatever reports it", async ({
		page,
		sql,
	}) => {
		const central = await seedServer(sql, {
			name: "box-central",
			type: "tamanu-central",
		});
		const facility = await seedServer(sql, {
			name: "box-facility",
			type: "tamanu-facility",
		});
		for (const server of [central, facility]) {
			await seedStatus(sql, {
				serverId: server.id,
				health: [{ check: "disk_free", result: "passed" }],
			});
		}

		await page.goto("/settings/healthchecks");

		await expect(page.getByRole("row", { name: /disk_free/ })).toHaveCount(1);
		await expect(
			page.getByRole("link", { name: "disk_free", exact: true }),
		).toHaveAttribute("href", "/settings/healthchecks/alertd/machine/disk_free");
	});

	/// A source canopy curates itself names its checks fleet-wide, so its
	/// entries carry no namespace and read as the name alone.
	test("a curated source's check keeps its bare name", async ({ page, sql }) => {
		const server = await seedServer(sql, { name: "curated-server" });
		await seedStatus(sql, {
			serverId: server.id,
			source: "canopy",
			health: [{ check: "backup-maintenance", result: "warning" }],
		});

		await page.goto("/settings/healthchecks");

		await expect(
			page.getByRole("link", { name: "backup-maintenance", exact: true }),
		).toHaveAttribute(
			"href",
			"/settings/healthchecks/canopy/-/backup-maintenance",
		);
	});
});

test.describe("Links predating the namespace", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	/// Check pages were addressed by source and name alone, so bookmarks and
	/// pasted links arrive without a namespace. Where one entry has the name,
	/// the link means that entry and lands on it.
	test("a two-segment link lands on the one entry it means", async ({
		page,
		sql,
	}) => {
		const server = await seedServer(sql, {
			name: "bookmark-server",
			type: "tamanu-central",
		});
		await seedStatus(sql, {
			serverId: server.id,
			health: [{ check: "postgres", result: "failed" }],
		});

		await page.goto("/healthchecks/alertd/postgres");

		await expect(page).toHaveURL(
			/\/healthchecks\/alertd\/application\.tamanu-central\/postgres$/,
		);
		await expect(
			page.getByRole("heading", { name: "tamanu-central:postgres", exact: true }),
		).toBeVisible();
	});

	/// Where two types report the name, the old link is genuinely ambiguous:
	/// it names two checks. Guessing one would silently show the wrong check's
	/// history, so the operator picks.
	test("an ambiguous two-segment link asks which check was meant", async ({
		page,
		sql,
	}) => {
		const central = await seedServer(sql, {
			name: "ambiguous-central",
			type: "tamanu-central",
		});
		const facility = await seedServer(sql, {
			name: "ambiguous-facility",
			type: "tamanu-facility",
		});
		for (const server of [central, facility]) {
			await seedStatus(sql, {
				serverId: server.id,
				health: [{ check: "postgres", result: "failed" }],
			});
		}

		await page.goto("/healthchecks/alertd/postgres");

		await expect(
			page.getByRole("heading", { name: "Which postgres?" }),
		).toBeVisible();

		await page
			.getByRole("link", { name: "tamanu-facility:postgres", exact: true })
			.click();
		await expect(page).toHaveURL(
			/\/healthchecks\/alertd\/application\.tamanu-facility\/postgres$/,
		);
	});
});

test.describe("Silences follow the namespace", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	/// A group spans several application types, so a group silence names one
	/// of them. Quieting the centrals' postgres leaves the facilities' postgres
	/// alerting: they are different checks that happen to share a name.
	///
	/// spec: CHK#silences-follow-the-event
	test("a group silence on one type leaves the other type's check alerting", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "namespaced-silence" });
		const central = await seedServer(sql, {
			name: "silenced-central",
			type: "tamanu-central",
			groupId: group.id,
		});
		const facility = await seedServer(sql, {
			name: "alerting-facility",
			type: "tamanu-facility",
			groupId: group.id,
		});
		for (const server of [central, facility]) {
			await seedStatus(sql, {
				serverId: server.id,
				health: [{ check: "postgres", result: "failed" }],
			});
		}
		await seedGroupSilencedRef(sql, {
			groupId: group.id,
			ref: "health/postgres",
			applicationType: "tamanu-central",
		});

		// The central's postgres is quiet, and says so at the group scope.
		await page.goto(`/fleet/applications/${central.id}`);
		await page
			.getByRole("button", { name: "Manage silence for postgres" })
			.click();
		await expect(page.getByText("Silenced for this group")).toBeVisible();

		// The facility's postgres is untouched, and still offers the silence.
		await page.goto(`/fleet/applications/${facility.id}`);
		await page.getByRole("button", { name: "Silence postgres" }).click();
		await expect(
			page.getByRole("button", { name: "For this group" }),
		).toBeVisible();
		await expect(page.getByText("Silenced for this group")).toHaveCount(0);
	});
});
