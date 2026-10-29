import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedDevice,
	seedDeviceConnection,
	seedServer,
	seedStatus,
} from "./seed";

const BESTOOL_EXTRA = {
	bestoolVersion: "2.10.5",
	pgVersion: "PostgreSQL 17.2, (x86_64-pc-linux-gnu, compiled by gcc)",
	nodeVersion: "20.11.0",
};

// spec: FIG
test.describe("reported server figures", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("presents the bestool version, and holds it through a later push from another source", async ({
		page,
		sql,
	}) => {
		const server = await seedServer(sql, { name: "runs-bestool" });
		await seedStatus(sql, {
			serverId: server.id,
			source: "alertd",
			extra: BESTOOL_EXTRA,
			createdAt: "NOW() - INTERVAL '10 minutes'",
		});
		// The legacy Tamanu source reports later and carries none of the figures.
		await seedStatus(sql, {
			serverId: server.id,
			source: "tamanu",
			extra: { uptimeSecs: 6038594 },
		});

		await page.goto(`/fleet/applications/${server.id}`);

		await expect(page.getByText("bestool", { exact: true })).toBeVisible();
		await expect(page.getByText("2.10.5", { exact: true })).toBeVisible();
		await expect(page.getByText("17.2", { exact: true })).toBeVisible();
		await expect(page.getByText("20.11.0", { exact: true })).toBeVisible();
	});

	test("shows a central's active mobile devices, zero included", async ({
		page,
		sql,
	}) => {
		const server = await seedServer(sql, {
			name: "no-tablets",
			type: "tamanu-central",
		});
		await seedStatus(sql, {
			serverId: server.id,
			source: "alertd",
			extra: { ...BESTOOL_EXTRA, activeMobileDevices: 0 },
		});

		await page.goto(`/fleet/applications/${server.id}`);

		const field = page
			.getByText("Mobile devices (30d)", { exact: true })
			.locator("..");
		await expect(field).toBeVisible();
		await expect(field.getByText("0", { exact: true })).toBeVisible();
	});

	test("shows no mobile figure where none is reported", async ({
		page,
		sql,
	}) => {
		const server = await seedServer(sql, { name: "older-bestool" });
		await seedStatus(sql, {
			serverId: server.id,
			source: "alertd",
			extra: BESTOOL_EXTRA,
		});

		await page.goto(`/fleet/applications/${server.id}`);

		await expect(page.getByText("2.10.5", { exact: true })).toBeVisible();
		await expect(
			page.getByText("Mobile devices (30d)", { exact: true }),
		).toHaveCount(0);
	});

	test("shows no bestool figure for a server no bestool reports on", async ({
		page,
		sql,
	}) => {
		const server = await seedServer(sql, { name: "no-bestool" });
		await seedStatus(sql, {
			serverId: server.id,
			source: "tamanu",
			extra: { uptimeSecs: 42 },
		});

		await page.goto(`/fleet/applications/${server.id}`);

		await expect(
			page.getByRole("heading", { level: 1, name: /no-bestool/ }),
		).toBeVisible();
		await expect(page.getByText("bestool", { exact: true })).toHaveCount(0);
	});

	test("carries the figures into the point-in-time snapshot", async ({
		page,
		sql,
	}) => {
		const server = await seedServer(sql, {
			name: "snapshot-bestool",
			type: "tamanu-central",
		});
		await seedStatus(sql, {
			serverId: server.id,
			source: "alertd",
			extra: BESTOOL_EXTRA,
			health: [{ check: "postgres", result: "failed" }],
			createdAt: "NOW() - INTERVAL '30 minutes'",
		});
		// A later push from another source carries none of the figures.
		await seedStatus(sql, {
			serverId: server.id,
			source: "tamanu",
			extra: { uptimeSecs: 6038594 },
		});
		// The failed check's own issue row is what opens the snapshot panel.
		await page.goto("/incidents?showAll=1");
		await page
			.getByRole("button", {
				name: "Status snapshot when this issue was last seen",
			})
			.first()
			.click();

		await expect(
			page.getByText("Status snapshot", { exact: true }),
		).toBeVisible();
		await expect(page.getByText("bestool", { exact: true })).toBeVisible();
		await expect(page.getByText("2.10.5", { exact: true })).toBeVisible();
	});

	test("reads the runtime off the machine's identity when a push doesn't name one", async ({
		page,
		sql,
	}) => {
		// The identity that belongs to the box, and a second one that files a
		// push on its behalf. What the box runs is the first one's; the second
		// is only that push's provenance.
		const box = await seedDevice(sql, { role: "machine" });
		await seedDeviceConnection(sql, {
			deviceId: box.id,
			userAgent: "bestool/2.10.5 Node.js/22.3.0",
		});
		const filer = await seedDevice(sql, { role: "relay" });
		await seedDeviceConnection(sql, {
			deviceId: filer.id,
			userAgent: "bestool/2.10.5 Node.js/14.21.3",
		});

		const server = await seedServer(sql, {
			name: "runtime-from-identity",
			deviceId: box.id,
		});
		await seedStatus(sql, {
			serverId: server.id,
			deviceId: filer.id,
			source: "alertd",
			// No `nodeVersion`, so the figure has to come from elsewhere.
			extra: { bestoolVersion: "2.10.5" },
		});

		await page.goto(`/fleet/applications/${server.id}`);

		await expect(page.getByText("Node.js", { exact: true })).toBeVisible();
		await expect(page.getByText("22.3.0", { exact: true })).toBeVisible();
		await expect(page.getByText("14.21.3", { exact: true })).toHaveCount(0);
	});
});
