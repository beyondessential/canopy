import { expect, test } from "./test-fixtures";
import { resetSeededTables, seedDevice, seedDeviceKey } from "./seed";

test.describe("devices index", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("renders the tabs and the search input", async ({ page }) => {
		await page.goto("/devices");
		await expect(page.getByRole("tab", { name: "Search" })).toBeVisible();
		await expect(
			page.getByRole("tab", { name: "All devices" }),
		).toBeVisible();
		await expect(
			page.getByRole("searchbox", { name: /Search by public key/i }),
		).toBeVisible();
	});
});

test.describe("devices list", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("all-devices tab lists every device regardless of role", async ({
		page,
		sql,
	}) => {
		const server = await seedDevice(sql, { role: "machine" });
		await seedDeviceKey(sql, {
			deviceId: server.id,
			name: "server-cert",
			isActive: true,
		});
		const releaser = await seedDevice(sql, { role: "releaser" });
		await seedDeviceKey(sql, {
			deviceId: releaser.id,
			name: "releaser-cert",
			isActive: true,
		});

		await page.goto("/devices/all");
		await expect(page).toHaveURL(/\/devices\/all$/);
		await expect(page.getByText("server-cert")).toBeVisible();
		await expect(page.getByText("releaser-cert")).toBeVisible();
	});
});

test.describe("device detail page", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("renders the role for a seeded device", async ({ page, sql }) => {
		const device = await seedDevice(sql, { role: "machine" });
		await seedDeviceKey(sql, {
			deviceId: device.id,
			name: "detail-cert",
			isActive: true,
		});

		await page.goto(`/devices/${device.id}`);

		// The named key shows up as the device name in the header.
		await expect(
			page.getByRole("heading", { name: "detail-cert", level: 1 }),
		).toBeVisible();
	});
});

/// A device reads by what it is known by, never by its id: a named key, then
/// its tailnet name, and failing everything as an unnamed device.
/// spec: FLT#naming
test.describe("device naming", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("a device with only a tailnet name reads by it", async ({
		page,
		sql,
	}) => {
		const device = await seedDevice(sql, {
			tailscaleNodeName: "node-7.tailnet.ts.net",
		});
		await page.goto(`/devices/${device.id}`);
		await expect(
			page.getByRole("heading", {
				level: 1,
				name: "Device node-7.tailnet.ts.net",
			}),
		).toBeVisible();
	});

	test("a device with nothing to go by reads as unnamed", async ({
		page,
		sql,
	}) => {
		const device = await seedDevice(sql);
		await page.goto(`/devices/${device.id}`);
		await expect(
			page.getByRole("heading", { level: 1, name: "Unnamed device" }),
		).toBeVisible();
	});
});
