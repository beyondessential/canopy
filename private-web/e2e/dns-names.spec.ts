import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedDeniedDnsName,
	seedServer,
	seedServerGroup,
	seedServerGroupDomain,
	seedUndeclaredDnsName,
} from "./seed";

// Declaring DNS names for addresses and for certificates, the requests of each
// kind that resolved to no application on a box, denying them, and the notices
// that say a declaration is wanted (DNS). The two kinds are separate features:
// each has its own section, declarations, requests and denials.

test.describe("DNS names", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	/** A group claiming fiji.tamanu.app, and a box in it running a Tamanu central
	 * and a SENAITE lab, both allowed certificates, and addresses too where
	 * `dns` is set. */
	async function sharedBox(
		sql: Parameters<typeof seedServer>[0],
		opts: { dns?: boolean } = {},
	) {
		const group = await seedServerGroup(sql, { name: "fiji" });
		await seedServerGroupDomain(sql, {
			groupId: group.id,
			domain: "fiji.tamanu.app",
		});
		const central = await seedServer(sql, {
			name: "central",
			groupId: group.id,
			mayManageTls: true,
			mayManageDns: opts.dns ?? false,
		});
		const lab = await seedServer(sql, {
			name: "lab",
			type: "senaite",
			groupId: group.id,
			mayManageTls: true,
			mayManageDns: opts.dns ?? false,
			machineId: central.machineId,
		});
		return { group, central, lab, machineId: central.machineId };
	}

	test("an operator declares and releases a DNS name for certificates on an application", async ({
		page,
		sql,
	}) => {
		const { central } = await sharedBox(sql);
		await page.goto(`/fleet/applications/${central.id}`);

		const field = page.getByLabel("DNS name to declare for certificates");
		await field.fill("site.fiji.tamanu.app");
		await page.getByRole("button", { name: "Declare", exact: true }).click();

		const row = page
			.getByTestId("declared-name-row")
			.filter({ hasText: "site.fiji.tamanu.app" });
		await expect(row).toBeVisible();
		await expect(row.getByText("outside the group's domains")).toHaveCount(0);

		// Outside every domain the group controls: allowed, and flagged.
		await field.fill("site.samoa.tamanu.app");
		await page.getByRole("button", { name: "Declare", exact: true }).click();
		await expect(
			page
				.getByTestId("declared-name-row")
				.filter({ hasText: "site.samoa.tamanu.app" })
				.getByText("outside the group's domains"),
		).toBeVisible();

		page.once("dialog", (dialog) => dialog.accept());
		await row.getByRole("button", { name: "Release" }).click();
		await expect(
			page
				.getByTestId("declared-name-row")
				.filter({ hasText: "site.fiji.tamanu.app" }),
		).toHaveCount(0);
	});

	test("a DNS name declared for addresses shows only in the DNS names section, and for certificates only in the TLS certificates section", async ({
		page,
		sql,
	}) => {
		const { central } = await sharedBox(sql, { dns: true });
		await page.goto(`/fleet/applications/${central.id}`);
		const dns = page.getByTestId("application-names-addresses");
		const tls = page.getByTestId("application-names-certificate");

		await dns
			.getByLabel("DNS name to declare for addresses")
			.fill("records.fiji.tamanu.app");
		await dns.getByRole("button", { name: "Declare", exact: true }).click();
		await expect(dns.getByText("records.fiji.tamanu.app")).toBeVisible();
		await expect(dns.getByText("no addresses registered")).toBeVisible();
		await expect(tls.getByText("records.fiji.tamanu.app")).toHaveCount(0);

		await tls
			.getByLabel("DNS name to declare for certificates")
			.fill("tls.fiji.tamanu.app");
		await tls.getByRole("button", { name: "Declare", exact: true }).click();
		await expect(tls.getByText("tls.fiji.tamanu.app")).toBeVisible();
		await expect(dns.getByText("tls.fiji.tamanu.app")).toHaveCount(0);

		// The same DNS name for both kinds on the one application shows in both.
		await tls
			.getByLabel("DNS name to declare for certificates")
			.fill("records.fiji.tamanu.app");
		await tls.getByRole("button", { name: "Declare", exact: true }).click();
		await expect(tls.getByText("records.fiji.tamanu.app")).toBeVisible();
		await expect(dns.getByText("records.fiji.tamanu.app")).toBeVisible();

		// Releasing it for addresses leaves it declared for certificates.
		page.once("dialog", (dialog) => dialog.accept());
		await dns
			.getByTestId("dns-name-row")
			.filter({ hasText: "records.fiji.tamanu.app" })
			.getByRole("button", { name: "Release" })
			.click();
		await expect(dns.getByText("records.fiji.tamanu.app")).toHaveCount(0);
		await expect(tls.getByText("records.fiji.tamanu.app")).toBeVisible();
	});

	test("a DNS name held for one kind by one application cannot be declared for the other by another", async ({
		page,
		sql,
	}) => {
		const { central, lab } = await sharedBox(sql, { dns: true });
		await page.goto(`/fleet/applications/${central.id}`);
		const dns = page.getByTestId("application-names-addresses");
		await dns
			.getByLabel("DNS name to declare for addresses")
			.fill("held.fiji.tamanu.app");
		await dns.getByRole("button", { name: "Declare", exact: true }).click();
		await expect(dns.getByText("held.fiji.tamanu.app")).toBeVisible();

		await page.goto(`/fleet/applications/${lab.id}`);
		const tls = page.getByTestId("application-names-certificate");
		await tls
			.getByLabel("DNS name to declare for certificates")
			.fill("held.fiji.tamanu.app");
		await tls.getByRole("button", { name: "Declare", exact: true }).click();
		await expect(tls.getByRole("alert")).toContainText("already declared by");
		await expect(tls.getByRole("alert")).toContainText("central");
		await expect(tls.getByText("held.fiji.tamanu.app")).toHaveCount(0);
	});

	test("an undeclared request is declared on one of the box's applications", async ({
		page,
		sql,
	}) => {
		const { machineId } = await sharedBox(sql);
		await seedUndeclaredDnsName(sql, {
			machineId,
			name: "lab.fiji.tamanu.app",
		});

		await page.goto(`/fleet/machines/${machineId}`);
		const section = page.getByTestId("machine-names-certificate");
		const row = section.getByTestId("undeclared-row");
		await expect(row.getByText("lab.fiji.tamanu.app")).toBeVisible();

		await row.getByLabel("Application to declare it on").click();
		await page.getByRole("option", { name: "lab" }).click();
		await row.getByRole("button", { name: "Declare" }).click();

		await expect(section.getByTestId("undeclared-row")).toHaveCount(0);
		const declared = section.getByRole("row", { name: /lab\.fiji\.tamanu\.app/ });
		await expect(declared.getByRole("cell", { name: "lab", exact: true })).toBeVisible();
	});

	test("a machine's DNS names and TLS certificates sections each carry only their own kind", async ({
		page,
		sql,
	}) => {
		const { machineId } = await sharedBox(sql, { dns: true });
		await seedUndeclaredDnsName(sql, {
			machineId,
			name: "records.fiji.tamanu.app",
			kind: "addresses",
		});
		await seedUndeclaredDnsName(sql, {
			machineId,
			name: "tls.fiji.tamanu.app",
			kind: "certificate",
		});
		await seedDeniedDnsName(sql, {
			machineId,
			name: "no-records.fiji.tamanu.app",
			kind: "addresses",
		});
		await seedDeniedDnsName(sql, {
			machineId,
			name: "no-tls.fiji.tamanu.app",
			kind: "certificate",
		});

		await page.goto(`/fleet/machines/${machineId}`);
		const dns = page.getByTestId("machine-names-addresses");
		const tls = page.getByTestId("machine-names-certificate");
		await expect(
			dns.getByRole("heading", { name: "DNS names" }),
		).toBeVisible();
		await expect(
			tls.getByRole("heading", { name: "TLS certificates" }),
		).toBeVisible();

		await expect(dns.getByText("records.fiji.tamanu.app")).toBeVisible();
		await expect(dns.getByText("no-records.fiji.tamanu.app")).toBeVisible();
		await expect(dns.getByText("tls.fiji.tamanu.app")).toHaveCount(0);
		await expect(dns.getByText("no-tls.fiji.tamanu.app")).toHaveCount(0);

		await expect(tls.getByText("tls.fiji.tamanu.app")).toBeVisible();
		await expect(tls.getByText("no-tls.fiji.tamanu.app")).toBeVisible();
		await expect(tls.getByText("records.fiji.tamanu.app")).toHaveCount(0);
		await expect(tls.getByText("no-records.fiji.tamanu.app")).toHaveCount(0);

		// Declaring one kind ends that kind's request and leaves the other's.
		const row = tls.getByTestId("undeclared-row");
		await row.getByLabel("Application to declare it on").click();
		await page.getByRole("option", { name: "lab" }).click();
		await row.getByRole("button", { name: "Declare" }).click();
		await expect(tls.getByTestId("undeclared-row")).toHaveCount(0);
		await expect(dns.getByTestId("undeclared-row")).toHaveCount(1);
	});

	test("denying a DNS name for addresses leaves certificates unaffected, and lifting is of the one kind", async ({
		page,
		sql,
	}) => {
		const { machineId } = await sharedBox(sql, { dns: true });
		await seedUndeclaredDnsName(sql, {
			machineId,
			name: "both.fiji.tamanu.app",
			kind: "addresses",
		});
		await seedUndeclaredDnsName(sql, {
			machineId,
			name: "both.fiji.tamanu.app",
			kind: "certificate",
		});

		await page.goto(`/fleet/machines/${machineId}`);
		const dns = page.getByTestId("machine-names-addresses");
		const tls = page.getByTestId("machine-names-certificate");
		await dns
			.getByTestId("undeclared-row")
			.getByRole("button", { name: "Deny" })
			.click();
		await page.getByRole("dialog").getByRole("button", { name: "Deny" }).click();

		await expect(dns.getByTestId("undeclared-row")).toHaveCount(0);
		await expect(dns.getByTestId("denied-row")).toHaveCount(1);
		await expect(tls.getByTestId("undeclared-row")).toHaveCount(1);
		await expect(tls.getByTestId("denied-row")).toHaveCount(0);

		await dns.getByTestId("denied-row").getByRole("button", { name: "Lift" }).click();
		await expect(page.getByTestId("machine-names-addresses")).toHaveCount(0);
		await expect(tls.getByTestId("undeclared-row")).toHaveCount(1);
	});

	test("an undeclared request is denied with a note, and the denial lifted", async ({
		page,
		sql,
	}) => {
		const { machineId } = await sharedBox(sql);
		await seedUndeclaredDnsName(sql, {
			machineId,
			name: "old.fiji.tamanu.app",
		});

		await page.goto(`/fleet/machines/${machineId}`);
		const section = page.getByTestId("machine-names-certificate");
		await section
			.getByTestId("undeclared-row")
			.getByRole("button", { name: "Deny" })
			.click();
		await page.getByLabel("Note").fill("site retired");
		await page.getByRole("dialog").getByRole("button", { name: "Deny" }).click();

		await expect(section.getByTestId("undeclared-row")).toHaveCount(0);
		const denied = section.getByTestId("denied-row");
		await expect(denied.getByText("old.fiji.tamanu.app")).toBeVisible();
		await expect(denied.getByText(/site retired/)).toBeVisible();

		await denied.getByRole("button", { name: "Lift" }).click();
		// Nothing left to show, so the section goes altogether.
		await expect(page.getByTestId("machine-names-certificate")).toHaveCount(0);
	});

	test("a box that never asked about a DNS name shows no section", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "quiet" });
		const only = await seedServer(sql, { name: "solo", groupId: group.id });
		await page.goto(`/fleet/machines/${only.machineId}`);
		await expect(page.getByTestId("applications-on-box")).toBeVisible();
		await expect(page.getByTestId("machine-names-addresses")).toHaveCount(0);
		await expect(page.getByTestId("machine-names-certificate")).toHaveCount(0);
	});

	test("undeclared requests raise a notice on the group and on Status", async ({
		page,
		sql,
	}) => {
		const { group, machineId } = await sharedBox(sql);
		await seedUndeclaredDnsName(sql, { machineId, name: "a.fiji.tamanu.app" });
		await seedUndeclaredDnsName(sql, { machineId, name: "b.fiji.tamanu.app" });
		// Not asked about for over a day, so it no longer counts.
		await seedUndeclaredDnsName(sql, {
			machineId,
			name: "stale.fiji.tamanu.app",
			askedMinutesAgo: 25 * 60,
		});
		// A denial raises nothing.
		await seedDeniedDnsName(sql, { machineId, name: "c.fiji.tamanu.app" });

		await page.goto(`/fleet/groups/${group.id}`);
		const groupNotice = page.getByTestId("undeclared-dns-names-notice");
		await expect(groupNotice).toContainText("2 requests waiting on a declaration");
		await expect(groupNotice).toContainText("2 for TLS certificates");
		await groupNotice.getByRole("link", { name: "central" }).click();
		await expect(page).toHaveURL(new RegExp(`/fleet/machines/${machineId}$`));

		await page.goto("/status");
		const statusNotice = page.getByTestId("undeclared-dns-names-notice");
		await expect(statusNotice).toContainText("2 requests waiting on a declaration");
		await expect(statusNotice.getByRole("link", { name: "fiji" })).toBeVisible();
	});

	test("an operator who is not an admin sees the requests but no controls", async ({
		page,
		sql,
	}) => {
		const { machineId } = await sharedBox(sql);
		await seedUndeclaredDnsName(sql, { machineId, name: "lab.fiji.tamanu.app" });
		await seedDeniedDnsName(sql, { machineId, name: "old.fiji.tamanu.app" });
		await page.route("**/api/commons/is_current_user_admin", (route) =>
			route.fulfill({ json: false }),
		);

		await page.goto(`/fleet/machines/${machineId}`);
		const section = page.getByTestId("machine-names-certificate");
		await expect(section.getByText("lab.fiji.tamanu.app")).toBeVisible();
		await expect(section.getByText("old.fiji.tamanu.app")).toBeVisible();
		await expect(section.getByRole("button")).toHaveCount(0);
	});

	test("with nothing waiting there is no notice", async ({ page, sql }) => {
		const { group, machineId } = await sharedBox(sql);
		await seedDeniedDnsName(sql, { machineId, name: "c.fiji.tamanu.app" });

		await page.goto(`/fleet/groups/${group.id}`);
		await expect(page.getByRole("heading", { level: 1, name: "fiji" })).toBeVisible();
		await expect(page.getByTestId("undeclared-dns-names-notice")).toHaveCount(0);
	});

	test("the notice says how many requests there are of each kind", async ({
		page,
		sql,
	}) => {
		const { group, machineId } = await sharedBox(sql);
		await seedUndeclaredDnsName(sql, {
			machineId,
			name: "a.fiji.tamanu.app",
			kind: "addresses",
		});
		await seedUndeclaredDnsName(sql, {
			machineId,
			name: "b.fiji.tamanu.app",
			kind: "certificate",
		});

		await page.goto(`/fleet/groups/${group.id}`);
		const notice = page.getByTestId("undeclared-dns-names-notice");
		await expect(notice).toContainText("2 requests waiting on a declaration");
		await expect(notice).toContainText("1 for DNS address");
		await expect(notice).toContainText("1 for TLS certificate");
	});

	test("a pause shows in both of an application's sections", async ({
		page,
		sql,
	}) => {
		const { central } = await sharedBox(sql, { dns: true });
		await page.goto(`/fleet/applications/${central.id}`);
		const dns = page.getByTestId("application-names-addresses");
		const tls = page.getByTestId("application-names-certificate");

		await tls.getByRole("button", { name: "Pause" }).click();
		await page.getByLabel("Reason").fill("looking into an odd request pattern");
		await page
			.getByRole("button", { name: "Pause", exact: true })
			.last()
			.click();

		for (const section of [dns, tls]) {
			await expect(section.getByText("Paused")).toBeVisible();
			await expect(
				section.getByText(/looking into an odd request pattern/),
			).toBeVisible();
			await expect(section.getByText(/admin@localhost/)).toBeVisible();
		}
	});
});
