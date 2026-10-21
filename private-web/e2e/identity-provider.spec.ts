import { expect, test } from "./test-fixtures";
import {
	resetAuditLog,
	resetSeededTables,
	seedAuditEntry,
	seedIdentityGrant,
	seedIdentityKey,
	seedMachine,
	seedRelyingParty,
	seedServer,
	seedServerGroup,
} from "./seed";

const OLD_KID = "oldKeyThumbprint0000000000000000000000000001";
const NEW_KID = "newKeyThumbprint0000000000000000000000000002";

/// The identity provider's settings page.
///
/// spec: IDP
test.describe("identity provider settings", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
		await resetAuditLog(sql);
	});

	test("presents the issuer, the keys mid-rotation and the register", async ({
		page,
		sql,
	}) => {
		await seedIdentityKey(sql, {
			kid: OLD_KID,
			keyRef: "e2e-key-old",
			servedMinutesAgo: 60 * 24 * 30,
			signedMinutesAgo: 60 * 24 * 30,
		});
		await seedIdentityKey(sql, {
			kid: NEW_KID,
			keyRef: "e2e-key-new",
			servedMinutesAgo: 5,
		});
		const party = await seedRelyingParty(sql, {
			name: "Anthropic",
			audience: "https://api.anthropic.example",
		});
		const app = await seedServer(sql, { name: "central" });
		await seedIdentityGrant(sql, { relyingPartyId: party.id, applicationId: app.id });
		await seedAuditEntry(sql, {
			action: "identity_token_issued",
			minutesAgo: 2,
			records: [
				{ kind: "application", id: app.id, name: "central" },
				{ kind: "relying_party", id: party.id, name: "Anthropic" },
			],
			detail: {
				jti: "e2e-jti",
				kid: OLD_KID,
				sub: "e2e",
				expires_at: new Date(Date.now() + 3 * 60_000).toISOString(),
			},
		});

		await page.goto("/settings/identity-provider");
		await expect(page.getByText("https://identity.e2e.invalid", { exact: true })).toBeVisible();
		await expect(
			page.getByText("https://identity.e2e.invalid/.well-known/openid-configuration"),
		).toBeVisible();

		const keys = page.getByRole("table").first().getByRole("row");
		// Newest first: the new key is published while it matures, and the old
		// one still signs.
		await expect(keys.nth(1)).toContainText("e2e-key-new");
		await expect(keys.nth(1)).toContainText("published");
		await expect(keys.nth(2)).toContainText("e2e-key-old");
		await expect(keys.nth(2)).toContainText("signing");
		await expect(keys.nth(2).getByRole("cell").last()).toHaveText("1");

		const row = page.getByRole("row").filter({ hasText: "https://api.anthropic.example" });
		await expect(row).toContainText("Anthropic");
		await expect(row).toContainText("5 minutes");
		await expect(row).toContainText("1 application");
		await expect(row).not.toContainText("none in 90 days");
	});

	test("registers, edits and removes a relying party", async ({ page, sql }) => {
		await page.goto("/settings/identity-provider");
		await page.getByLabel("Name", { exact: true }).fill("AWS");
		await page.getByLabel("Audience").fill("sts.amazonaws.com");
		await page.getByLabel("Token lifetime, minutes").fill("15");
		await page.getByRole("button", { name: "Add" }).click();

		const row = page.getByRole("row").filter({ hasText: "sts.amazonaws.com" });
		await expect(row).toContainText("15 minutes");
		await expect(row).toContainText("none in 90 days");

		await row.getByRole("button", { name: "edit AWS" }).click();
		const edit = page.getByRole("dialog");
		await expect(edit.getByLabel("Audience")).toBeDisabled();
		await edit.getByLabel("Name").fill("AWS STS");
		await edit.getByLabel("Token lifetime, minutes").fill("10");
		await edit.getByRole("button", { name: "Save" }).click();
		await expect(row).toContainText("AWS STS");
		await expect(row).toContainText("10 minutes");

		const [party] = await sql.query<{ id: string }>(
			"SELECT id FROM relying_parties WHERE audience = 'sts.amazonaws.com'",
		);
		const machine = await seedMachine(sql, { name: "granted-box" });
		await seedIdentityGrant(sql, { relyingPartyId: party!.id, machineId: machine.id });
		await page.reload();

		await row.getByRole("button", { name: "remove AWS STS" }).click();
		const confirm = page.getByRole("dialog");
		await expect(confirm).toContainText("This revokes 1 grant.");
		await confirm.getByRole("button", { name: "Remove" }).click();
		await expect(row).toHaveCount(0);
	});

	test("a lifetime that is not whole minutes in range cannot be saved", async ({ page }) => {
		await page.goto("/settings/identity-provider");
		await page.getByLabel("Name", { exact: true }).fill("AWS");
		await page.getByLabel("Audience").fill("sts.amazonaws.com");
		const lifetime = page.getByLabel("Token lifetime, minutes");
		for (const bad of ["1.5", "0", "61", ""]) {
			await lifetime.fill(bad);
			await expect(page.getByText("Whole minutes, 1 to 60")).toBeVisible();
			await expect(page.getByRole("button", { name: "Add" })).toBeDisabled();
		}
		await lifetime.fill("60");
		await expect(page.getByText("Whole minutes, 1 to 60")).toHaveCount(0);
		await expect(page.getByRole("button", { name: "Add" })).toBeEnabled();
	});
});

/// The grant choices on a machine's edit page.
///
/// spec: IDP#presentation
test.describe("identity token grants", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
		await resetAuditLog(sql);
	});

	test("grants an application a relying party and shows its subject", async ({
		page,
		request,
		sql,
	}) => {
		const party = await seedRelyingParty(sql, { name: "Anthropic" });
		const group = await seedServerGroup(sql, { name: "grants-group" });
		const app = await seedServer(sql, {
			name: "central",
			groupId: group.id,
			rank: "production",
		});

		await page.goto(`/fleet/machines/${app.machineId}/edit`);
		const section = page.locator(`[data-application="${app.id}"]`);
		await expect(
			section.getByText(
				`group:${group.id}:rank:production:application:tamanu-central:${app.id}:`,
			),
		).toBeVisible();
		await expect(
			page
				.getByTestId("machine-section")
				.getByText(`group:${group.id}:rank:production:machine:${app.machineId}:`),
		).toBeVisible();

		await section.getByLabel("May obtain tokens for").click();
		await page.getByRole("option", { name: "Anthropic" }).click();
		await page.getByRole("button", { name: "Save" }).click();
		await expect(page).toHaveURL(`/fleet/machines/${app.machineId}`);

		const grants = await (
			await request.post("/api/identity/machine_grants", {
				data: { machine_id: app.machineId },
			})
		).json();
		const appGrants = grants.subjects.find(
			(s: { subject: { id: string } }) => s.subject.id === app.id,
		);
		expect(appGrants.granted).toEqual([party.id]);
	});

	test("an unchanged save sends no grant change", async ({ page, sql }) => {
		const party = await seedRelyingParty(sql, { name: "Anthropic" });
		const group = await seedServerGroup(sql, { name: "unchanged-group" });
		const app = await seedServer(sql, { groupId: group.id, rank: "production" });
		await seedIdentityGrant(sql, { relyingPartyId: party.id, applicationId: app.id });

		const grantCalls: string[] = [];
		page.on("request", (r) => {
			if (r.url().includes("/api/identity/set_grants")) grantCalls.push(r.url());
		});
		await page.goto(`/fleet/machines/${app.machineId}/edit`);
		await expect(
			page.locator(`[data-application="${app.id}"]`).getByText("Anthropic"),
		).toBeVisible();
		await page.getByRole("button", { name: "Save" }).click();
		await expect(page).toHaveURL(`/fleet/machines/${app.machineId}`);
		expect(grantCalls).toEqual([]);
	});

	test("a pending machine is offered no choice", async ({ page, sql }) => {
		await seedRelyingParty(sql, { name: "Anthropic" });
		const group = await seedServerGroup(sql, { name: "pending-group" });
		const machine = await seedMachine(sql, { groupId: group.id });

		await page.goto(`/fleet/machines/${machine.id}/edit`);
		await expect(page.getByTestId("machine-section")).toBeVisible();
		await expect(page.getByText("Identity tokens")).toHaveCount(0);
	});

	test("ranking a pending machine in the form offers the choice", async ({
		page,
		request,
		sql,
	}) => {
		const party = await seedRelyingParty(sql, { name: "Anthropic" });
		const group = await seedServerGroup(sql, { name: "ranked-here-group" });
		const machine = await seedMachine(sql, { groupId: group.id });

		await page.goto(`/fleet/machines/${machine.id}/edit`);
		const box = page.getByTestId("machine-section");
		await expect(box.getByText("Identity tokens")).toHaveCount(0);
		await box.getByRole("combobox", { name: "Rank" }).click();
		await page.getByRole("option", { name: "production" }).click();

		await expect(box.getByText("Identity tokens")).toBeVisible();
		await expect(box.getByText("The subject changes once saved.")).toBeVisible();
		await box.getByLabel("May obtain tokens for").click();
		await page.getByRole("option", { name: "Anthropic" }).click();
		await page.getByRole("button", { name: "Save" }).click();
		await expect(page).toHaveURL(`/fleet/machines/${machine.id}`);

		const grants = await (
			await request.post("/api/identity/machine_grants", {
				data: { machine_id: machine.id },
			})
		).json();
		expect(grants.subjects[0].granted).toEqual([party.id]);
		expect(grants.subjects[0].sub).toBe(
			`group:${group.id}:rank:production:machine:${machine.id}:`,
		);
	});

	test("changing the rank hides the saved subject", async ({ page, sql }) => {
		await seedRelyingParty(sql, { name: "Anthropic" });
		const group = await seedServerGroup(sql, { name: "rerank-group" });
		const app = await seedServer(sql, { groupId: group.id, rank: "production" });

		await page.goto(`/fleet/machines/${app.machineId}/edit`);
		const box = page.getByTestId("machine-section");
		const saved = `group:${group.id}:rank:production:machine:${app.machineId}:`;
		await expect(box.getByText(saved)).toBeVisible();
		await box.getByRole("combobox", { name: "Rank" }).click();
		await page.getByRole("option", { name: "demo" }).click();
		await expect(box.getByText(saved)).toHaveCount(0);
		await expect(box.getByText("The subject changes once saved.")).toBeVisible();
	});

	test("clearing the group hides the saved subject", async ({ page, sql }) => {
		const party = await seedRelyingParty(sql, { name: "Anthropic" });
		const group = await seedServerGroup(sql, { name: "ungroup-here-group" });
		const app = await seedServer(sql, { groupId: group.id, rank: "production" });
		await seedIdentityGrant(sql, { relyingPartyId: party.id, machineId: app.machineId });

		await page.goto(`/fleet/machines/${app.machineId}/edit`);
		const box = page.getByTestId("machine-section");
		const saved = `group:${group.id}:rank:production:machine:${app.machineId}:`;
		await expect(box.getByText(saved)).toBeVisible();
		const groupField = box.getByRole("combobox", { name: "Group" });
		await groupField.hover();
		await box.getByRole("button", { name: "Clear" }).click();
		await expect(box.getByText(saved)).toHaveCount(0);
		// The grant is still held, so it stays withdrawable.
		await expect(box.getByText("Identity tokens")).toBeVisible();
	});

	test("the form stays usable when grants cannot be read", async ({ page, sql }) => {
		await seedRelyingParty(sql, { name: "Anthropic" });
		const group = await seedServerGroup(sql, { name: "grants-down-group" });
		const app = await seedServer(sql, { groupId: group.id, rank: "production" });
		await page.route("**/api/identity/machine_grants", (route) =>
			route.fulfill({ status: 500, body: "{}" }),
		);

		await page.goto(`/fleet/machines/${app.machineId}/edit`);
		const box = page.getByTestId("machine-section");
		await expect(box.getByText("Identity token grants could not be read")).toBeVisible();
		await expect(box.getByText("Identity tokens")).toHaveCount(0);
		await expect(page.getByRole("button", { name: "Save" })).toBeEnabled();
	});

	test("with no relying party registered the choice is hidden", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "no-party-group" });
		const app = await seedServer(sql, { groupId: group.id, rank: "production" });

		await page.goto(`/fleet/machines/${app.machineId}/edit`);
		await expect(page.getByTestId("machine-section")).toBeVisible();
		await expect(page.getByText("Identity tokens")).toHaveCount(0);
	});

	test("a held grant stays withdrawable on a pending machine", async ({
		page,
		request,
		sql,
	}) => {
		const party = await seedRelyingParty(sql, { name: "Anthropic" });
		const group = await seedServerGroup(sql, { name: "held-group" });
		const machine = await seedMachine(sql, { groupId: group.id });
		await seedIdentityGrant(sql, { relyingPartyId: party.id, machineId: machine.id });

		await page.goto(`/fleet/machines/${machine.id}/edit`);
		const box = page.getByTestId("machine-section");
		await expect(box.getByText("Identity tokens")).toBeVisible();
		await box.getByRole("button", { name: "Anthropic" }).getByTestId("CancelIcon").click();
		await page.getByRole("button", { name: "Save" }).click();
		await expect(page).toHaveURL(`/fleet/machines/${machine.id}`);

		const grants = await (
			await request.post("/api/identity/machine_grants", {
				data: { machine_id: machine.id },
			})
		).json();
		expect(grants.subjects[0].granted).toEqual([]);
	});
});
