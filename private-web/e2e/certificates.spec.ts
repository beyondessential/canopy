import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedServer,
	seedCertificateName,
	seedServerCertificate,
	seedServerGroup,
	seedServerGroupDomain,
	seedServerName,
} from "./seed";

// The e2e fixture runs the private-server in a debug build, so the Tailscale
// auth bypass treats every caller as `admin@localhost` (an admin), the managed
// zones are `tamanu.app` and `demo.tamanu.app`, and the certificate authority is
// the in-process fake — which advertises the `classic` and `shortlived`
// profiles and accepts revocations.

test.describe("an application's DNS names and certificates", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("a server with neither grant and nothing registered shows no section", async ({
		page,
		sql,
	}) => {
		const server = await seedServer(sql, { name: "plain" });
		await page.goto(`/fleet/applications/${server.id}`);
		// Wait for the page proper before asserting an absence, or the assertion
		// would pass against a page that simply had not loaded.
		await expect(
			page.getByRole("heading", { level: 1, name: /plain/ }),
		).toBeVisible();
		await expect(page.getByTestId("application-names-addresses")).toHaveCount(0);
		await expect(page.getByTestId("application-names-certificate")).toHaveCount(0);
	});

	test("registered names show their addresses and whether the zone caught up", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "fiji" });
		await seedServerGroupDomain(sql, {
			groupId: group.id,
			domain: "fiji.tamanu.app",
		});
		const server = await seedServer(sql, {
			name: "central",
			groupId: group.id,
			mayManageDns: true,
		});
		await seedServerName(sql, {
			serverId: server.id,
			name: "a.fiji.tamanu.app",
			addresses: ["192.0.2.1", "2001:db8::1"],
			publishedAddresses: ["192.0.2.1", "2001:db8::1"],
		});
		await seedServerName(sql, {
			serverId: server.id,
			name: "b.fiji.tamanu.app",
			addresses: ["192.0.2.9"],
			lastError: "route53 refused the change",
		});

		await page.goto(`/fleet/applications/${server.id}`);
		const panel = page.getByTestId("application-names-addresses");

		await expect(panel.getByText("may manage DNS records")).toBeVisible();
		await expect(panel.getByText("within fiji.tamanu.app")).toBeVisible();

		await expect(panel.getByText("a.fiji.tamanu.app")).toBeVisible();
		await expect(panel.getByText("192.0.2.1, 2001:db8::1")).toBeVisible();
		await expect(panel.getByText("published", { exact: true })).toBeVisible();

		await expect(panel.getByText("b.fiji.tamanu.app")).toBeVisible();
		await expect(panel.getByText("waiting to publish")).toBeVisible();
		await expect(
			panel.getByText("route53 refused the change"),
		).toBeVisible();
	});

	test("certificates show profile, expiry, and how long is left", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "fiji" });
		await seedServerGroupDomain(sql, {
			groupId: group.id,
			domain: "fiji.tamanu.app",
		});
		const server = await seedServer(sql, {
			name: "central",
			groupId: group.id,
			mayManageTls: true,
		});
		await seedServerCertificate(sql, {
			serverId: server.id,
			name: "a.fiji.tamanu.app",
			profile: "classic",
			lifetimeDays: 90,
			expiresInDays: 80,
		});

		await page.goto(`/fleet/applications/${server.id}`);
		const panel = page.getByTestId("application-names-certificate");

		await expect(
			panel.getByRole("heading", { name: /TLS certificates/ }),
		).toBeVisible();
		await expect(panel.getByText("a.fiji.tamanu.app").first()).toBeVisible();
		await expect(panel.getByText("valid", { exact: true })).toBeVisible();
		await expect(panel.getByText("classic", { exact: true })).toBeVisible();
		// The expiry is shown once, as a duration, floored, with the instant on
		// hover rather than a second reading beside it.
		const left = panel.getByText("expires in 79 days");
		await expect(left).toBeVisible();
		await expect(panel.getByText(/days left/)).toHaveCount(0);
		await expect(left).toHaveAttribute("data-risk", "none");
		await left.hover();
		await expect(page.getByRole("tooltip")).toContainText(/\d{4}/);
		// Nothing about addresses in the certificates section.
		await expect(panel.getByText("published")).toHaveCount(0);
	});

	test("a certificate past renewal reads as due, and one nearly gone as expiring", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "fiji" });
		await seedServerGroupDomain(sql, {
			groupId: group.id,
			domain: "fiji.tamanu.app",
		});
		const server = await seedServer(sql, {
			name: "central",
			groupId: group.id,
			mayManageTls: true,
		});
		// A third of ninety days left is where renewal is due; a sixth is where it
		// stops being a warning.
		await seedServerCertificate(sql, {
			serverId: server.id,
			name: "due.fiji.tamanu.app",
			lifetimeDays: 90,
			expiresInDays: 20,
		});
		await seedServerCertificate(sql, {
			serverId: server.id,
			name: "gone.fiji.tamanu.app",
			lifetimeDays: 90,
			expiresInDays: 2,
		});

		await page.goto(`/fleet/applications/${server.id}`);
		const panel = page.getByTestId("application-names-certificate");

		await expect(panel.getByText("due for renewal")).toBeVisible();
		await expect(panel.getByText("expiring", { exact: true })).toBeVisible();
		// The time left is coloured on the same measure as the chip beside it.
		await expect(panel.getByText("expires in 19 days")).toHaveAttribute(
			"data-risk",
			"at_risk",
		);
		await expect(panel.getByText("expires in 1 day")).toHaveAttribute(
			"data-risk",
			"critical",
		);
	});

	test("a pending first issuance shows its reason for failing", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "fiji" });
		await seedServerGroupDomain(sql, {
			groupId: group.id,
			domain: "fiji.tamanu.app",
		});
		const server = await seedServer(sql, {
			name: "central",
			groupId: group.id,
			mayManageTls: true,
		});
		await seedServerCertificate(sql, {
			serverId: server.id,
			name: "never.fiji.tamanu.app",
			state: "pending",
			attempts: 7,
			lastError: "the authority did not validate the name",
		});

		await page.goto(`/fleet/applications/${server.id}`);
		const panel = page.getByTestId("application-names-certificate");

		await expect(panel.getByText("pending", { exact: true })).toBeVisible();
		await expect(
			panel.getByText(/after 7 attempt\(s\).*did not validate the name/),
		).toBeVisible();
	});

	test("the profile can be set from the authority's advertised list", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "fiji" });
		await seedServerGroupDomain(sql, {
			groupId: group.id,
			domain: "fiji.tamanu.app",
		});
		const server = await seedServer(sql, {
			name: "central",
			groupId: group.id,
			mayManageTls: true,
		});

		await page.goto(`/fleet/applications/${server.id}`);
		const picker = page.getByLabel("Certificate lifetime");
		await expect(picker).toBeVisible();
		// The default is the authority's own, which is its longest-lived: a short
		// lifetime is adopted deliberately rather than inherited.
		await expect(
			page.getByText("Authority default (longest-lived)"),
		).toBeVisible();

		await picker.click();
		await page.getByRole("option", { name: "shortlived" }).click();

		// Polled rather than read once: the select closes before the request that
		// saves the choice has landed, so a single read races it.
		await expect
			.poll(async () => {
				const [row] = await sql.query<{ certificate_profile: string | null }>(
					"SELECT certificate_profile FROM applications WHERE id = $1",
					[server.id],
				);
				return row.certificate_profile;
			})
			.toBe("shortlived");
	});

	test("pausing records who and why, and shows what it stops", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "fiji" });
		await seedServerGroupDomain(sql, {
			groupId: group.id,
			domain: "fiji.tamanu.app",
		});
		const server = await seedServer(sql, {
			name: "central",
			groupId: group.id,
			mayManageTls: true,
		});
		await seedServerCertificate(sql, {
			serverId: server.id,
			name: "a.fiji.tamanu.app",
		});

		await page.goto(`/fleet/applications/${server.id}`);
		await page.getByRole("button", { name: "Pause" }).click();
		await page
			.getByLabel("Reason")
			.fill("looking into an odd request pattern");
		await page
			.getByRole("button", { name: "Pause", exact: true })
			.last()
			.click();

		await expect(page.getByText("Paused")).toBeVisible();
		await expect(
			page.getByText(/looking into an odd request pattern/),
		).toBeVisible();
		await expect(page.getByText(/admin@localhost/)).toBeVisible();
		// What the pause does and does not do, said where the operator is.
		await expect(
			page.getByText(/What is already in place stands and keeps working/),
		).toBeVisible();

		// And it lifts again.
		page.once("dialog", (dialog) => dialog.accept());
		await page.getByRole("button", { name: "Resume" }).click();
		await expect(page.getByText("Paused")).toHaveCount(0);
	});

	test("revoking says it cannot be undone, and pauses the server", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "fiji" });
		await seedServerGroupDomain(sql, {
			groupId: group.id,
			domain: "fiji.tamanu.app",
		});
		const server = await seedServer(sql, {
			name: "central",
			groupId: group.id,
			mayManageTls: true,
		});
		const cert = await seedServerCertificate(sql, {
			serverId: server.id,
			name: "leaked.fiji.tamanu.app",
		});

		await page.goto(`/fleet/applications/${server.id}`);
		await page.getByRole("button", { name: "Revoke" }).click();

		await expect(
			page.getByText(/This cannot be undone/),
		).toBeVisible();
		await expect(page.getByText(/pauses this server/)).toBeVisible();

		await page.getByLabel("Reason").click();
		await page.getByRole("option", { name: "Key compromise" }).click();
		await expect(
			page.getByText(/never be certified again, for any name by any server/),
		).toBeVisible();
		await page
			.getByRole("button", { name: "Revoke", exact: true })
			.last()
			.click();

		await expect(page.getByText("revoked", { exact: true })).toBeVisible();
		// The pause the revocation set, without being asked.
		await expect(page.getByText("Paused")).toBeVisible();

		const [row] = await sql.query<{
			state: string;
			revocation_reason: string | null;
			revoked_by: string | null;
		}>(
			"SELECT state, revocation_reason, revoked_by FROM application_certificates WHERE id = $1",
			[cert.id],
		);
		expect(row.state).toBe("revoked");
		expect(row.revocation_reason).toBe("key_compromise");
		expect(row.revoked_by).toBe("admin@localhost");

		// A compromised key is barred from ever being certified again.
		const barred = await sql.query<{ count: string }>(
			"SELECT count(*) FROM compromised_keys",
		);
		expect(Number(barred[0].count)).toBe(1);
	});
});

test.describe("group DNS names and certificates", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("each kind lists its own names beneath each domain", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "fiji" });
		await seedServerGroupDomain(sql, {
			groupId: group.id,
			domain: "fiji.tamanu.app",
		});
		const server = await seedServer(sql, {
			name: "central",
			groupId: group.id,
			mayManageDns: true,
			mayManageTls: true,
		});
		await seedServerName(sql, {
			serverId: server.id,
			name: "covered.fiji.tamanu.app",
			addresses: ["192.0.2.1"],
			publishedAddresses: ["192.0.2.1"],
		});
		await seedServerCertificate(sql, {
			serverId: server.id,
			name: "covered.fiji.tamanu.app",
		});
		// Registered but never certified, which is the case worth spotting from
		// the group's page.
		await seedServerName(sql, {
			serverId: server.id,
			name: "bare.fiji.tamanu.app",
			addresses: ["192.0.2.2"],
			publishedAddresses: ["192.0.2.2"],
		});
		await seedCertificateName(sql, {
			serverId: server.id,
			name: "bare.fiji.tamanu.app",
		});
		// Certified but with no addresses of its own.
		await seedServerCertificate(sql, {
			serverId: server.id,
			name: "tls-only.fiji.tamanu.app",
		});

		await page.goto(`/fleet/groups/${group.id}`);
		const dns = page.getByTestId("group-names-addresses");
		const tls = page.getByTestId("group-names-certificate");

		// Only addresses in the DNS names section, and nothing about certificates.
		await expect(dns.getByText("covered.fiji.tamanu.app")).toBeVisible();
		await expect(dns.getByText("bare.fiji.tamanu.app")).toBeVisible();
		await expect(dns.getByText("tls-only.fiji.tamanu.app")).toHaveCount(0);
		await expect(dns.getByText("certified")).toHaveCount(0);
		await expect(dns.getByText("no certificate")).toHaveCount(0);

		// Only certificates in the TLS certificates section, and nothing about
		// records.
		await expect(tls.getByText("covered.fiji.tamanu.app")).toBeVisible();
		await expect(tls.getByText("tls-only.fiji.tamanu.app")).toBeVisible();
		await expect(tls.getByText("certified", { exact: true })).toHaveCount(2);
		await expect(tls.getByText("bare.fiji.tamanu.app")).toBeVisible();
		await expect(tls.getByText("no certificate")).toBeVisible();
		await expect(tls.getByText("published")).toHaveCount(0);
	});

	test("a group with names of one kind shows no section for the other", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "fiji" });
		await seedServerGroupDomain(sql, {
			groupId: group.id,
			domain: "fiji.tamanu.app",
		});
		const server = await seedServer(sql, {
			name: "central",
			groupId: group.id,
			mayManageTls: true,
		});
		await seedServerCertificate(sql, {
			serverId: server.id,
			name: "a.fiji.tamanu.app",
		});

		await page.goto(`/fleet/groups/${group.id}`);
		await expect(page.getByTestId("group-names-certificate")).toBeVisible();
		await expect(page.getByTestId("group-names-addresses")).toHaveCount(0);
	});
});

test.describe("certificate authority settings", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("the authority, its profiles, and whether the account works", async ({
		page,
	}) => {
		await page.goto("/settings/certificate-authority");

		await expect(
			page.getByRole("heading", { name: "Certificate authority" }),
		).toBeVisible();
		await expect(page.getByText(/acme\.test\.invalid/)).toBeVisible();
		await expect(
			page.getByText(/Canopy holds a usable account at this authority/),
		).toBeVisible();
		await expect(page.getByText("classic", { exact: true })).toBeVisible();
		await expect(page.getByText("shortlived", { exact: true })).toBeVisible();
	});
});
