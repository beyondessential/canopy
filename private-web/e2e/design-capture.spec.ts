// Scratch capture rig for the P6 design loop. Not a test: it seeds the
// identity-shaped surfaces with plausible fleet data and screenshots them, so
// the loop's critics look at the app as it ships rather than at empty tables.
// Deleted when the run finishes.

import { mkdirSync } from "node:fs";
import { resolve } from "node:path";

import { expect, test } from "./test-fixtures";
import { seedDevice, seedDeviceConnection, seedDeviceKey } from "./seed";

const SHOTS = resolve(
	__dirname,
	"../../.workhorse/plans/p6/design/shots/current",
);

test.describe.configure({ mode: "serial" });

test("capture the identity surfaces", async ({ page, request, sql }) => {
	test.setTimeout(240_000);
	mkdirSync(SHOTS, { recursive: true });
	await page.setViewportSize({ width: 1440, height: 1000 });

	// Admins: a plausible on-call roster.
	for (const email of [
		"ana.whitford@example.invalid",
		"dev.ops@example.invalid",
		"j.marsh@example.invalid",
		"k.tahana@example.invalid",
		"support.lead@example.invalid",
	]) {
		await request.post("/api/admins/add", { data: { email } });
	}

	// MCP access: tokens in the states the page can show.
	for (const name of [
		"incident-triage-bot",
		"grafana-sidecar",
		"on-call-laptop",
		"retired-runner",
	]) {
		await request.post("/api/mcp_tokens/mint", { data: { name } });
	}

	// Devices: a mix of roles and node names, with keys and a connection each,
	// so the listing and a detail page both have something to show.
	const devices = [
		{ role: "machine", node: "tamanu-prod-db-1" },
		{ role: "machine", node: "tamanu-prod-web-1" },
		{ role: "pgro", node: "pgro-restore-operator" },
		{ role: "backup", node: "backup-runner-syd" },
		{ role: "machine", node: null as string | null },
		{ role: "bestool", node: "bestool-ci" },
	];
	let first: string | null = null;
	for (const d of devices) {
		const device = await seedDevice(sql, {
			role: d.role,
			tailscaleNodeName: d.node ?? undefined,
		});
		first ??= device.id;
		await seedDeviceKey(sql, { deviceId: device.id, name: `${d.role}-key` });
		await seedDeviceConnection(sql, {
			deviceId: device.id,
			userAgent: "canopy-agent/2.4.1",
		});
	}

	const pages: Array<[string, string]> = [
		["/settings/admins", "admins"],
		["/settings/mcp-tokens", "mcp-access"],
		["/settings/certificate-authority", "certificate-authority"],
		["/settings/identity-provider", "identity-provider"],
		["/settings/audit-log", "audit-log"],
		["/settings/restore-consumers", "restore-consumers"],
		["/devices/all", "devices-all"],
		["/devices", "devices-search"],
	];

	for (const [path, name] of pages) {
		await page.goto(path);
		await page.waitForLoadState("networkidle").catch(() => {});
		await page.waitForTimeout(600);
		await page.screenshot({ path: `${SHOTS}/${name}.png`, fullPage: true });
	}

	if (first) {
		await page.goto(`/devices/${first}`);
		await page.waitForLoadState("networkidle").catch(() => {});
		await page.waitForTimeout(600);
		await page.screenshot({
			path: `${SHOTS}/device-detail.png`,
			fullPage: true,
		});
	}

	expect(true).toBe(true);
});
