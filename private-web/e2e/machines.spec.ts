import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedDevice,
	seedIssue,
	seedMachine,
	seedMachineReport,
	seedServer,
	seedServerGroup,
	seedStatus,
} from "./seed";

/// The machine's own page: the box, what it reports about itself, and the
/// workloads on it.
///
/// spec: FLT
test.describe("machine detail", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("presents what the box reports about itself", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "box-group" });
		const machine = await seedMachine(sql, {
			name: "big-box",
			groupId: group.id,
		});
		await seedMachineReport(sql, {
			machineId: machine.id,
			extra: {
				hostname: "big-box.internal",
				platform: "Debian 12",
				cpuCores: 8,
				totalMemoryBytes: 34359738368,
				bestoolVersion: "2.10.5",
			},
		});

		await page.goto(`/fleet/machines/${machine.id}`);

		await expect(
			page.getByRole("heading", { level: 1, name: /big-box/ }),
		).toBeVisible();
		await expect(page.getByText("Debian 12")).toBeVisible();
		await expect(page.getByText("8", { exact: true })).toBeVisible();
		await expect(page.getByText("32.0 GiB")).toBeVisible();
		await expect(page.getByText("2.10.5")).toBeVisible();
	});

	test("a box carrying two workloads lists both, and each links to its own page", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "shared-box-group" });
		const machine = await seedMachine(sql, {
			name: "shared-box",
			groupId: group.id,
		});
		// Two workloads on the one box: the case the machine grain exists for.
		await sql.query(
			`INSERT INTO applications (id, name, host, type, rank, group_id, machine_id)
			 VALUES (gen_random_uuid(), 'central-on-shared', 'https://c.e2e.invalid',
			         'tamanu-central', 'production', $1, $2),
			        (gen_random_uuid(), 'facility-on-shared', 'https://f.e2e.invalid',
			         'tamanu-facility', 'production', $1, $2)`,
			[group.id, machine.id],
		);

		await page.goto(`/fleet/machines/${machine.id}`);

		// The box's own section, rather than the group tree the page ends with,
		// which lists every workload in the group.
		const onThisBox = page.getByTestId("applications-on-box");
		await expect(onThisBox.getByText("Applications (2)")).toBeVisible();
		await expect(
			onThisBox.getByRole("link", { name: "central-on-shared" }),
		).toBeVisible();
		await expect(
			onThisBox.getByRole("link", { name: "facility-on-shared" }),
		).toBeVisible();
	});

	test("a box with nothing on it reads as awaiting check-in", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "empty-box-group" });
		const machine = await seedMachine(sql, {
			name: "fresh-box",
			groupId: group.id,
		});

		await page.goto(`/fleet/machines/${machine.id}`);

		await expect(page.getByText(/hasn't checked in yet/i)).toBeVisible();
		await expect(
			page.getByTestId("applications-on-box").getByText("Applications (0)"),
		).toBeVisible();
		// A box created a minute ago carrying nothing is its normal condition,
		// not a count of zero and not an error.
		await expect(page.getByText("not yet reporting")).toBeVisible();
		await expect(
			page.getByText(/applications appear here as the machine reports them/i),
		).toBeVisible();
	});

	/// Enrolment admits the box, so the setup instructions live on the machine
	/// and the ticket is minted for it. Nothing runs here until the enrolled
	/// agent reports it, so no application is involved.
	///
	/// spec: FLT#machines-come-from-operators
	test("an unenrolled box offers the setup instructions and mints its ticket", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "enrol-group" });
		const machine = await seedMachine(sql, {
			name: "unenrolled-box",
			groupId: group.id,
		});

		await page.goto(`/fleet/machines/${machine.id}`);

		await expect(
			page.getByRole("heading", { name: "Set up this machine" }),
		).toBeVisible();
		await expect(page.getByText(/bestool canopy register/)).toBeVisible();
		// The ticket is the machine's: it is minted against the box's id.
		const rows = await sql.query<{ count: string }>(
			"SELECT COUNT(*) AS count FROM machine_enrollment_tokens WHERE machine_id = $1",
			[machine.id],
		);
		expect(Number(rows[0].count)).toBeGreaterThan(0);
	});

	/// The identity is bound to the box, not to any workload on it, so the
	/// device detail and re-enrolment sit behind the machine's own accordion.
	///
	/// spec: DTR
	test("the box carries the identity that speaks for it", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "identity-group" });
		const device = await seedDevice(sql, {
			tailscaleNodeName: "identity-box.tailnet.ts.net",
		});
		const machine = await seedMachine(sql, {
			name: "identity-box",
			groupId: group.id,
			deviceId: device.id,
		});
		await seedMachineReport(sql, { machineId: machine.id });

		await page.goto(`/fleet/machines/${machine.id}`);

		await page.getByRole("button", { name: "Identity" }).click();
		await expect(
			page.locator(`a[href="/devices/${device.id}"]`),
		).toBeVisible();
		await expect(
			page.getByRole("heading", { name: "Tailscale identity" }),
		).toBeVisible();
	});

	test("an application links to the box it runs on, and back", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "round-trip" });
		const server = await seedServer(sql, {
			name: "round-trip-app",
			groupId: group.id,
		});

		await page.goto(`/fleet/applications/${server.id}`);
		await page.getByRole("link", { name: "This box" }).click();
		await expect(page).toHaveURL(new RegExp(`/fleet/machines/${server.machineId}$`));

		await page
			.getByTestId("applications-on-box")
			.getByRole("link", { name: "round-trip-app" })
			.click();
		await expect(page).toHaveURL(new RegExp(`/fleet/applications/${server.id}$`));
	});

	/// Backups are taken of a box and an identity speaks for a box, so both are
	/// the machine's. An application's page is about the software, and offering
	/// either there would give a two-workload box two places to change one
	/// setting.
	/// spec: FLT, BAK, DID
	test("backups and identity are the box's, not the application's", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "grain-group" });
		const device = await seedDevice(sql, {
			tailscaleNodeName: "grain-box.tailnet.ts.net",
		});
		const server = await seedServer(sql, {
			name: "grain-app",
			groupId: group.id,
			deviceId: device.id,
		});

		await page.goto(`/fleet/applications/${server.id}`);
		await expect(page.getByRole("heading", { name: "grain-app" })).toBeVisible();
		await expect(
			page.getByRole("heading", { name: "Backups" }),
		).toHaveCount(0);
		await expect(page.getByRole("heading", { name: "Identity" })).toHaveCount(0);
		await expect(page.getByRole("button", { name: "Identity" })).toHaveCount(0);

		// Both are on the box, one page away.
		await page.goto(`/fleet/machines/${server.machineId}`);
		await expect(page.getByRole("heading", { name: "Backups" })).toBeVisible();
		await expect(page.getByRole("button", { name: "Identity" })).toBeVisible();
	});

	test("the group lists its boxes, including one with nothing on it", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "tree-group" });
		const server = await seedServer(sql, {
			name: "tree-app",
			groupId: group.id,
		});
		const empty = await seedMachine(sql, {
			name: "tree-empty-box",
			groupId: group.id,
		});

		await page.goto(`/fleet/groups/${group.id}`);

		// Both boxes are here — the one carrying a workload and the one that has
		// not reported yet, which was invisible when the group listed workloads.
		await expect(page.getByText("Machines (2)")).toBeVisible();
		await expect(
			page.getByRole("link", { name: "tree-empty-box" }),
		).toBeVisible();
		await expect(page.getByText("Awaiting check-in.")).toBeVisible();
		// The workload sits under its box rather than beside it. Matched by its
		// own href: `seedServer` names the box after the workload, so the two
		// links share a label.
		await expect(
			page.locator(`a[href="/fleet/applications/${server.id}"]`),
		).toBeVisible();

		await page.getByRole("link", { name: "tree-empty-box" }).click();
		await expect(page).toHaveURL(new RegExp(`/fleet/machines/${empty.id}$`));
	});

	test("creating a machine lands on its own page", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "landing-group" });

		await page.goto(`/fleet/groups/${group.id}/machines/new`);
		await page.getByLabel(/^Name(\s*\*)?$/i).fill("landed-box");
		await page.getByRole("button", { name: "Create machine" }).click();

		await expect(page).toHaveURL(/\/machines\/[0-9a-f-]{36}$/);
		await expect(
			page.getByRole("heading", { level: 1, name: /landed-box/ }),
		).toBeVisible();
	});

	/// A box serves one environment, so its rank is offered once, on the
	/// machine's own section, and no application section offers one.
	///
	/// spec: FLT#editing
	test("the rank is the machine's to edit, with no empty choice once ranked", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "ranked-group" });
		const central = await seedServer(sql, {
			name: "ranked-central",
			groupId: group.id,
			rank: "production",
		});
		const database = await seedServer(sql, {
			name: "ranked-database",
			groupId: group.id,
			machineId: central.machineId,
		});

		await page.goto(`/fleet/machines/${central.machineId}/edit`);

		const rank = page
			.getByTestId("machine-section")
			.getByRole("combobox", { name: "Rank" });
		await expect(rank).toHaveText("production");
		for (const id of [central.id, database.id]) {
			await expect(
				page
					.locator(`[data-application="${id}"]`)
					.getByRole("combobox", { name: "Rank" }),
			).toHaveCount(0);
		}

		await rank.click();
		await expect(page.getByRole("option")).toHaveText([
			"production",
			"clone",
			"demo",
			"test",
			"dev",
		]);
	});

	/// A box nothing has ranked reads as not ranked yet, and ranking it ranks
	/// every application on it in one save.
	///
	/// spec: FLT#editing
	test("ranking a pending box ranks every application on it", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "pending-group" });
		const first = await seedServer(sql, {
			name: "pending-central",
			groupId: group.id,
			rank: null,
		});
		const second = await seedServer(sql, {
			name: "pending-database",
			groupId: group.id,
			machineId: first.machineId,
			rank: null,
		});

		await page.goto(`/fleet/machines/${first.machineId}/edit`);
		const rank = page
			.getByTestId("machine-section")
			.getByRole("combobox", { name: "Rank" });
		await expect(rank).toHaveText("Not ranked yet");

		await rank.click();
		await page.getByRole("option", { name: "demo" }).click();
		await page.getByRole("button", { name: "Save" }).click();
		await expect(page).toHaveURL(new RegExp(`/fleet/machines/${first.machineId}$`));

		const ranks = await sql.query<{ rank: string | null }>(
			"SELECT rank FROM applications WHERE id = ANY($1::uuid[])",
			[[first.id, second.id]],
		);
		expect(ranks.map((row) => row.rank)).toEqual(["demo", "demo"]);
	});

	/// A machine always has a name, so the edit form will not save one
	/// cleared to blank.
	/// spec: FLT#naming
	test("a machine cannot be saved without a name", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "named-group" });
		const machine = await seedMachine(sql, {
			name: "named-box",
			groupId: group.id,
		});

		await page.goto(`/fleet/machines/${machine.id}/edit`);
		const name = page
			.getByTestId("machine-section")
			.getByLabel(/^Name(\s*\*)?$/i);
		await expect(name).toHaveValue("named-box");
		await name.fill("   ");
		await expect(page.getByRole("button", { name: "Save" })).toBeDisabled();
		await name.fill("renamed-box");
		await expect(page.getByRole("button", { name: "Save" })).toBeEnabled();
	});

	/// A check filed against a box is silenced against that box. The scopes
	/// offered are the ones the check applies at — the machine and its group —
	/// and never one above, silencing everywhere being the check's own ceiling.
	///
	/// spec: CHK#silences-follow-the-event
	test("a machine's check can be silenced from the machine", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "silence-group" });
		const machine = await seedMachine(sql, {
			name: "noisy-box",
			groupId: group.id,
		});
		await seedIssue(sql, {
			machineId: machine.id,
			source: "alertd",
			ref: "health/disk_free",
			message: "Disk nearly full",
		});

		await page.goto(`/fleet/machines/${machine.id}`);

		// The check is here, and the scopes offered are the box and its group —
		// the ones this check applies at, and nothing above them.
		await page.getByRole("button", { name: "Silence disk_free" }).click();
		await expect(
			page.getByRole("button", { name: "For this machine" }),
		).toBeVisible();
		await expect(
			page.getByRole("button", { name: "For this group" }),
		).toBeVisible();
		await expect(
			page.getByRole("button", { name: /everywhere|fleet/i }),
		).toHaveCount(0);

		await page.getByRole("button", { name: "For this machine" }).click();

		// Wait on the section first: it renders only once the write has landed
		// and the page refetched, so it is the signal that the silence exists.
		// The popover's own text re-renders off the same fetch, and asserting on
		// it first raced that round trip under load.
		await expect(
			page.getByRole("heading", { name: /Silenced refs/ }),
		).toBeVisible();
		await expect(
			page.getByText("issues with these refs on this machine"),
		).toBeVisible();
		// A successful write closes the popover, so reopen it to read back what
		// it recorded. The trigger's label is the signal that the silence has
		// landed: it reads "Manage" only once the row knows it is silenced.
		await page
			.getByRole("button", { name: "Manage silence for disk_free" })
			.click();
		// And it now offers to lift the silence rather than to set it.
		await expect(page.getByText("Silenced for this machine")).toBeVisible();
	});

	/// An application presents its own checks; the box's are read on the box.
	///
	/// spec: CHK#presentation
	test("an application lists none of its box's checks", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "shared-check-group" });
		const server = await seedServer(sql, {
			name: "workload-a",
			groupId: group.id,
		});
		await seedStatus(sql, { serverId: server.id, healthy: true, health: [] });
		// One fact about the box, one about the workload on it.
		await seedIssue(sql, {
			machineId: server.machineId,
			source: "alertd",
			ref: "health/disk_free",
			message: "Disk nearly full",
		});
		await seedIssue(sql, {
			serverId: server.id,
			source: "alertd",
			ref: "health/tamanu_version",
			severity: "warning",
			message: "Behind the release train",
		});

		await page.goto(`/fleet/applications/${server.id}`);

		await expect(
			page.getByText("tamanu-central:tamanu_version", { exact: true }),
		).toBeVisible();
		await expect(page.getByText("disk_free", { exact: true })).toHaveCount(0);

		// The headline is the workload's own: its version check is warning, and
		// the failing disk under it is the box's to answer for.
		// spec: CHK#health-rollup
		await expect(page.getByText("Warning", { exact: true })).toBeVisible();
		await expect(
			page.getByText("Unhealthy", { exact: true }),
		).toHaveCount(0);

		// The box's check is on the box.
		await page.goto(`/fleet/machines/${server.machineId}`);
		await expect(page.getByText("disk_free", { exact: true })).toBeVisible();
	});

	/// An application's check is silenced on the application or its group.
	///
	/// spec: CHK#silences-follow-the-event
	test("an application's check offers the application and group scopes", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "app-silence-group" });
		const server = await seedServer(sql, {
			name: "workload-b",
			groupId: group.id,
		});
		await seedIssue(sql, {
			serverId: server.id,
			source: "alertd",
			ref: "health/tamanu_version",
			message: "Behind the release train",
		});

		await page.goto(`/fleet/applications/${server.id}`);

		await page
			.getByRole("button", { name: "Silence tamanu_version" })
			.click();
		await expect(
			page.getByRole("button", { name: "For this server" }),
		).toBeVisible();
		await expect(
			page.getByRole("button", { name: "For this group" }),
		).toBeVisible();
		await expect(
			page.getByRole("button", { name: "For this machine" }),
		).toHaveCount(0);
	});
});
