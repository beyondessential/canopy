import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedBackupCredentialIssuance,
	seedBackupMaintenanceRun,
	seedBackupRepoStats,
	seedBackupRun,
	seedDevice,
	seedMachine,
	seedServer,
	seedServerBackupCapability,
	seedServerGroup,
	seedServerGroupBackupConfig,
} from "./seed";

// The e2e fixture runs the private-server in a debug build, so the Tailscale
// auth bypass treats every caller as `admin@localhost` (an admin). These specs
// therefore exercise the admin-facing flows; non-admin gating is covered by
// the Rust security() annotations + the prod auth path.

test.describe("backups zero-state + config", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("unconfigured group shows the set-up CTA and panel zero-state", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "no-backups" });

		await page.goto(`/fleet/groups/${group.id}`);
		// The CTA is a MUI Button rendered as a router link (role "link").
		await expect(
			page.getByRole("link", { name: /set up backups/i }),
		).toBeVisible();

		await page.goto(`/fleet/groups/${group.id}/backups`);
		await expect(page.getByText(/backups not set up/i)).toBeVisible();
	});

	test("wizard create (empty bucket) writes a from-birth config row", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "cfg-group" });

		await page.goto(`/fleet/groups/${group.id}/backups/config`);
		await page.getByRole("button", { name: /use an existing bucket/i }).click();
		await page.getByLabel("Bucket").fill("bes-kopia-created");
		await page
			.getByLabel("Target role ARN")
			.fill("arn:aws:iam::999:role/created");
		await page
			.getByLabel("Maintenance role ARN")
			.fill("arn:aws:iam::999:role/created-maint");
		await page.getByRole("button", { name: /check bucket/i }).click();
		await expect(page.getByText(/empty bucket/i)).toBeVisible();
		// No schedule step — schedule/retention inherit the per-type defaults.
		await page.getByRole("button", { name: /create & provision/i }).click();

		await expect(page).toHaveURL(new RegExp(`/fleet/groups/${group.id}/backups$`));

		const rows = await sql.query<{
			status: string;
			mode: string;
			maintenance_role_arn: string;
		}>(
			`SELECT status, mode, maintenance_role_arn
			 FROM server_group_backup_config WHERE group_id = $1`,
			[group.id],
		);
		expect(rows).toHaveLength(1);
		expect(rows[0]!.status).toBe("provisioning");
		expect(rows[0]!.mode).toBe("from_birth");
		expect(rows[0]!.maintenance_role_arn).toBe(
			"arn:aws:iam::999:role/created-maint",
		);
		// The wizard writes no per-(group,type) schedule override.
		const sched = await sql.query(
			`SELECT 1 FROM server_group_backup_schedule WHERE group_id = $1`,
			[group.id],
		);
		expect(sched).toHaveLength(0);
	});

	test("wizard create (existing repo) requires a passphrase and persists passphrase mode", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "passphrase-group" });
		await page.goto(`/fleet/groups/${group.id}/backups/config`);
		await page.getByRole("button", { name: /use an existing bucket/i }).click();
		// `…existing…` → the fake prober reports an existing kopia repo.
		await page.getByLabel("Bucket").fill("bes-existing-repo");
		await page.getByLabel("Target role ARN").fill("arn:aws:iam::999:role/dev");
		await page
			.getByLabel("Maintenance role ARN")
			.fill("arn:aws:iam::999:role/maint");
		await page.getByRole("button", { name: /check bucket/i }).click();

		await expect(page.getByText(/existing kopia repository/i)).toBeVisible();
		// Provisioning is gated on the passphrase.
		await expect(
			page.getByRole("button", { name: /create & provision/i }),
		).toBeDisabled();
		await page
			.getByLabel("Existing repository passphrase")
			.fill("an-existing-repo-passphrase");
		await page.getByRole("button", { name: /create & provision/i }).click();

		await expect(page).toHaveURL(new RegExp(`/fleet/groups/${group.id}/backups$`));
		const rows = await sql.query<{ mode: string }>(
			`SELECT mode FROM server_group_backup_config WHERE group_id = $1`,
			[group.id],
		);
		expect(rows[0]!.mode).toBe("passphrase");
	});

	test("wizard blocks other (non-kopia) content", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "other-group" });
		await page.goto(`/fleet/groups/${group.id}/backups/config`);
		await page.getByRole("button", { name: /use an existing bucket/i }).click();
		// `…other…` → the fake prober reports non-kopia content.
		await page.getByLabel("Bucket").fill("bes-other-stuff");
		await page.getByLabel("Target role ARN").fill("arn");
		await page.getByLabel("Maintenance role ARN").fill("maint-arn");
		await page.getByRole("button", { name: /check bucket/i }).click();

		await expect(page.getByText(/other \(non-kopia\) content/i)).toBeVisible();
		// No proceeding: Create & provision disabled, Re-check offered.
		await expect(
			page.getByRole("button", { name: /create & provision/i }),
		).toBeDisabled();
		await expect(page.getByRole("button", { name: /re-check/i })).toBeVisible();
	});

	test("'shared backups' option provisions a canopy-managed bucket (no AWS account)", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "Acme Prod" });

		await page.goto(`/fleet/groups/${group.id}/backups/config`);
		// "Create a bucket" = shared-account backups — no bucket/roles, no probe.
		await page.getByRole("button", { name: /create a bucket/i }).click();
		await page.getByRole("button", { name: /create & provision/i }).click();

		await expect(page).toHaveURL(new RegExp(`/fleet/groups/${group.id}/backups$`));
		// The panel shows which placement was used.
		await expect(page.getByText(/shared account/i)).toBeVisible();

		const rows = await sql.query<{
			status: string;
			mode: string;
			placement: string;
			bucket: string;
		}>(
			`SELECT status, mode, placement, bucket
			 FROM server_group_backup_config WHERE group_id = $1`,
			[group.id],
		);
		expect(rows).toHaveLength(1);
		expect(rows[0]!.placement).toBe("shared");
		expect(rows[0]!.mode).toBe("from_birth");
		expect(rows[0]!.status).toBe("provisioning");
		// Auto-named from the group name + a random suffix.
		expect(rows[0]!.bucket.startsWith("bes-canopy-backup-acme-prod-")).toBe(true);
	});
});

test.describe("backups ready: stats + backup-now", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("per-type schedule inherits the default and saves an override", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "sched-group" });
		const server = await seedServer(sql, { groupId: group.id });
		// An enabled tamanu-postgres capability makes the type appear in the panel.
		await seedServerBackupCapability(sql, { machineId: server.machineId });
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);

		// Scope to the schedule panel: the type also appears in the Servers
		// panel (as a declared-type label), so a page-wide text match is
		// ambiguous. (Next-expected lives in the Servers panel now, not here.)
		const schedules = page
			.getByRole("heading", { name: /schedule & retention/i })
			.locator("..");
		// No override yet → inherits the seeded canopy-wide default.
		await expect(schedules.getByText("tamanu-postgres")).toBeVisible();
		await expect(schedules.getByText("fleet default").first()).toBeVisible();

		// Override the interval to 12h.
		await page.getByRole("button", { name: /^override schedule$/i }).click();
		await page.getByLabel("Back up every (hours)").fill("12");
		await page.getByRole("button", { name: /save schedule override/i }).click();

		await expect(schedules.getByText("group override")).toBeVisible();
		const rows = await sql.query<{ secs: string }>(
			`SELECT EXTRACT(EPOCH FROM expected_interval)::text AS secs
			 FROM server_group_backup_schedule
			 WHERE group_id = $1 AND type = 'tamanu-postgres'`,
			[group.id],
		);
		expect(rows).toHaveLength(1);
		expect(Number(rows[0]!.secs)).toBe(43200);
	});

	test("override editor dangerous toggle allows retention below the floor", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "danger-group" });
		const server = await seedServer(sql, { groupId: group.id });
		await seedServerBackupCapability(sql, { machineId: server.machineId });
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		await page.getByRole("button", { name: /^override retention$/i }).click();

		// Below-floor daily is blocked until the dangerous toggle is on.
		await page.getByLabel("Daily").fill("2");
		await expect(page.getByText(/daily must be ≥ 7/i)).toBeVisible();
		await page
			.getByLabel(/allow retention below the org minimum/i)
			.check();
		await page.getByRole("button", { name: /save retention override/i }).click();

		await expect(page.getByText("below floor")).toBeVisible();
		await expect
			.poll(async () => {
				const rows = await sql.query<{ allow: boolean; keep_daily: string }>(
					`SELECT allow_below_floor AS allow,
					        (retention->>'keep_daily') AS keep_daily
					 FROM server_group_backup_schedule
					 WHERE group_id = $1 AND type = 'tamanu-postgres'`,
					[group.id],
				);
				return rows[0] ? `${rows[0].allow}:${rows[0].keep_daily}` : null;
			})
			.toBe("true:2");
	});

	test("machines panel groups by rank then kind, like the group page", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "rank-order-group" });
		// Seed out of display order: expected order is the production bucket
		// (central before facility) then clone then dev, with rank subheaders —
		// not insertion or alphabetical order.
		await seedServer(sql, {
			groupId: group.id,
			name: "aaa-dev",
			rank: "dev",
		});
		await seedServer(sql, {
			groupId: group.id,
			name: "aaa-prod-facility",
			rank: "production",
			type: "tamanu-facility",
		});
		await seedServer(sql, {
			groupId: group.id,
			name: "ccc-clone",
			rank: "clone",
		});
		await seedServer(sql, {
			groupId: group.id,
			name: "zzz-prod-central",
			rank: "production",
			type: "tamanu-central",
		});
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);

		// Boxes, not workloads: each seeded application gets a box of its own,
		// named after it, and the box takes its workload's rank.
		const panel = page
			.getByRole("heading", { name: "Machines", exact: true })
			.locator("..");
		await expect(panel.locator('a[href^="/fleet/machines/"]')).toHaveText([
			"zzz-prod-central",
			"aaa-prod-facility",
			"ccc-clone",
			"aaa-dev",
		]);
		// Rank subheaders bucket the rows, same as the group page.
		await expect(panel.getByText("production", { exact: true })).toBeVisible();
		await expect(panel.getByText("clone", { exact: true })).toBeVisible();
		await expect(panel.getByText("dev", { exact: true })).toBeVisible();
	});

	test("stats render with unknown bucket bytes and recent runs", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "stats-group" });
		const device = await seedDevice(sql);
		const server = await seedServer(sql, {
			name: "stats-srv",
			groupId: group.id,
			deviceId: device.id,
		});
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
			intervalSeconds: 3600,
		});
		await seedBackupRepoStats(sql, {
			groupId: group.id,
			snapshotCount: 42,
			sourceCount: 3,
			logicalBytes: 1048576,
			physicalBytes: 524288,
			bucketBytes: null, // renders as "unknown", not hidden
		});
		await seedBackupRun(sql, {
			deviceId: device.id,
			groupId: group.id,
			machineId: server.machineId,
			outcome: "success",
			bytesUploaded: 2048,
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		await expect(page.getByText(/repository stats/i)).toBeVisible();
		await expect(page.getByText("42")).toBeVisible();
		// bucket_bytes NULL → "unknown" shown, per the indicators rule.
		await expect(page.getByText(/bucket bytes:\s*unknown/i)).toBeVisible();
		// The "Observed" timestamp belongs to the kopia-reported repo stats, so
		// it sits under "Physical bytes", above the bucket figure.
		const statsPanel = page
			.getByRole("heading", { name: /repository stats/i })
			.locator("..");
		const panelText = await statsPanel.innerText();
		expect(panelText.indexOf("Observed")).toBeGreaterThan(
			panelText.indexOf("Physical bytes"),
		);
		expect(panelText.indexOf("Observed")).toBeLessThan(
			panelText.indexOf("Bucket bytes"),
		);
		await expect(page.getByText(/recent runs/i)).toBeVisible();
		await expect(page.getByText("success")).toBeVisible();
		// The run carries a server_id, so the table names which server it's from.
		const runs = page.getByRole("table").last();
		await expect(runs.getByText("stats-srv")).toBeVisible();
	});

	/// A run stays in the group's history after its box moves elsewhere, and
	/// still reads by the box's name rather than its id.
	/// spec: FLT#naming
	test("a run from a box that has left the group names the box", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "runs-left" });
		const elsewhere = await seedServerGroup(sql, { name: "runs-elsewhere" });
		const device = await seedDevice(sql);
		const moved = await seedMachine(sql, {
			name: "moved-box",
			groupId: elsewhere.id,
		});
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
			intervalSeconds: 3600,
		});
		await seedBackupRun(sql, {
			deviceId: device.id,
			groupId: group.id,
			machineId: moved.id,
			outcome: "success",
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		const runs = page.getByRole("table").last();
		await expect(runs.getByText("moved-box")).toBeVisible();
		await expect(runs.getByText(moved.id.slice(0, 8))).toHaveCount(0);
	});

	test("recent runs show a Canopy-measured duration and surface unreported restores", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "dur-group" });
		const device = await seedDevice(sql);
		const server = await seedServer(sql, {
			name: "dur-srv",
			groupId: group.id,
			deviceId: device.id,
		});
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
			intervalSeconds: 3600,
		});

		// A reported backup, plus the credential issuance that started it 5 minutes
		// before it reported → the row carries a 5m duration.
		await seedBackupRun(sql, {
			deviceId: device.id,
			groupId: group.id,
			machineId: server.machineId,
			outcome: "success",
		});
		await seedBackupCredentialIssuance(sql, {
			deviceId: device.id,
			groupId: group.id,
			purpose: "backup",
			issuedAgoSecs: 300,
		});
		// A restore whose credentials are still valid but which never reported →
		// shown as in progress (this is a manual `bestool canopy restore`).
		await seedBackupCredentialIssuance(sql, {
			deviceId: device.id,
			groupId: group.id,
			purpose: "restore",
			issuedAgoSecs: 30,
			ttlSecs: 3600,
		});
		// A restore whose credentials expired without a report → unknown outcome.
		await seedBackupCredentialIssuance(sql, {
			deviceId: device.id,
			groupId: group.id,
			type: "malaria-db",
			purpose: "restore",
			issuedAgoSecs: 7200,
			ttlSecs: 3600,
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		const runs = page.getByRole("table").last();
		// Duration column populated for the reported run.
		await expect(runs.getByText("5m")).toBeVisible();
		// The unreported restores surface with their inferred states.
		await expect(runs.getByText("in progress")).toBeVisible();
		await expect(runs.getByText("unknown")).toBeVisible();
		await expect(runs.getByText("restore").first()).toBeVisible();
	});

	test("monthly S3 traffic totals this month's runs with an egress-cost tooltip", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "s3-traffic-group" });
		const device = await seedDevice(sql);
		const server = await seedServer(sql, {
			name: "s3-traffic-srv",
			groupId: group.id,
			deviceId: device.id,
		});
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
			region: "ap-southeast-2",
			intervalSeconds: 3600,
		});
		// Two runs this month → summed (300 sent / 30 received, raw bytes).
		await seedBackupRun(sql, {
			deviceId: device.id,
			groupId: group.id,
			machineId: server.machineId,
			s3SentRawBytes: 100,
			s3SentPayloadBytes: 90,
			s3ReceivedRawBytes: 10,
			s3ReceivedPayloadBytes: 9,
		});
		await seedBackupRun(sql, {
			deviceId: device.id,
			groupId: group.id,
			machineId: server.machineId,
			s3SentRawBytes: 200,
			s3SentPayloadBytes: 180,
			s3ReceivedRawBytes: 20,
			s3ReceivedPayloadBytes: 18,
		});
		// Backdated 40 days → always outside the calendar month, must not count.
		await seedBackupRun(sql, {
			deviceId: device.id,
			groupId: group.id,
			machineId: server.machineId,
			s3SentRawBytes: 9999,
			s3ReceivedRawBytes: 9999,
			reportedAgoSecs: 40 * 24 * 3600,
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		// getByText resolves to the innermost <strong> label; the values live in
		// its parent Typography, so assert (and hover) on that.
		const stat = page.getByText(/S3 traffic \(this month\)/i).locator("..");
		await expect(stat).toBeVisible();
		// Raw-byte totals for the in-month runs only (9999s excluded).
		await expect(stat).toContainText("300 B sent / 30 B received");
		// The undercount caveat is visible without hovering.
		await expect(
			page.getByText(/device backup traffic only/i),
		).toBeVisible();

		// The tooltip prices egress by the config's region.
		await stat.hover();
		const tooltip = page.getByRole("tooltip");
		await expect(tooltip).toContainText("Uploads are free");
		await expect(tooltip).toContainText("$0.114/GB in ap-southeast-2");
		await expect(tooltip).toContainText("estimated $0.00");
		await expect(tooltip).not.toContainText("assumed");
	});

	test("bucket bytes shows an estimated monthly cost tooltip", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "cost-group" });
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
			region: "ap-southeast-2",
		});
		await seedBackupRepoStats(sql, {
			groupId: group.id,
			bucketBytes: 107374182400, // 100 GiB
			bucketBytesObservedAt: new Date(Date.now() - 86_400_000).toISOString(),
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		await expect(page.getByText(/bucket bytes:\s*100\.0 GiB/i)).toBeVisible();

		await page.getByText(/^100\.0 GiB$/).hover();
		const tooltip = page.getByRole("tooltip");
		await expect(tooltip).toContainText("$2.50/month");
		await expect(tooltip).toContainText("ap-southeast-2");
		// The bucket figure comes from CloudWatch on its own daily cadence, so
		// the tooltip carries its measurement time and says it updates less
		// often than the kopia-reported stats.
		await expect(tooltip).toContainText("From CloudWatch storage metrics");
		await expect(tooltip).toContainText("measured");
		await expect(tooltip).toContainText("about once a day");
	});

	test("recent run shows a truncated, copyable snapshot id", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "snap-group" });
		const device = await seedDevice(sql);
		const server = await seedServer(sql, {
			name: "snap-srv",
			groupId: group.id,
			deviceId: device.id,
		});
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
			intervalSeconds: 3600,
		});
		await seedBackupRun(sql, {
			deviceId: device.id,
			groupId: group.id,
			machineId: server.machineId,
			outcome: "success",
			bytesUploaded: 2048,
			snapshotId: "k0123456789abcdef0123",
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		const runs = page.getByRole("table").last();
		// Shown truncated (not the full opaque id), with a copy button.
		await expect(runs.getByText(/k0123456789/)).toBeVisible();
		await expect(runs.getByText("k0123456789abcdef0123")).toBeHidden();
		await expect(
			runs.getByRole("button", { name: /copy snapshot id/i }),
		).toBeVisible();
	});

	test("run with no reported size falls back to the inspected snapshot size", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "size-group" });
		const device = await seedDevice(sql);
		const server = await seedServer(sql, {
			name: "size-srv",
			groupId: group.id,
			deviceId: device.id,
		});
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
			intervalSeconds: 3600,
		});
		// The device reported no size, but repo inspection observed the snapshot's
		// logical size and backfilled it onto the run.
		await seedBackupRun(sql, {
			deviceId: device.id,
			groupId: group.id,
			machineId: server.machineId,
			outcome: "success",
			bytesUploaded: null,
			snapshotLogicalBytes: 4096,
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		const runs = page.getByRole("table").last();
		await expect(runs.getByText("4.0 KiB")).toBeVisible();
		// No S3 traffic reported and no snapshot id → Transfer and Snapshot cells
		// both fall back to "—", plus the empty Duration column.
		await expect(runs.getByRole("cell", { name: "—" })).toHaveCount(3);
	});

	test("failed run shows expandable error detail, no snapshot size, and no upload", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "err-group" });
		const device = await seedDevice(sql);
		const server = await seedServer(sql, {
			name: "err-srv",
			groupId: group.id,
			deviceId: device.id,
		});
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
			intervalSeconds: 3600,
		});
		await seedBackupRun(sql, {
			deviceId: device.id,
			groupId: group.id,
			machineId: server.machineId,
			outcome: "failure",
			error: "kopia: snapshot failed: disk quota exceeded",
			bytesUploaded: null,
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		const runs = page.getByRole("table").last();
		await expect(runs.getByText("err-srv")).toBeVisible();
		await expect(runs.getByText("failure")).toBeVisible();
		// A failed run has no size, no upload, and no snapshot → four "—" cells
		// (Snapshot size, Transfer, Snapshot, Duration), not "unknown".
		await expect(runs.getByRole("cell", { name: "—" })).toHaveCount(4);

		// Error detail is hidden until the row is expanded.
		await expect(page.getByText(/disk quota exceeded/i)).toBeHidden();
		await runs.getByRole("button", { name: /show details/i }).click();
		await expect(page.getByText(/disk quota exceeded/i)).toBeVisible();
	});

	test("run with S3 traffic but no reported size shows uploaded bytes, no snapshot size, and expandable traffic detail", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "s3-group" });
		const device = await seedDevice(sql);
		const server = await seedServer(sql, {
			name: "s3-srv",
			groupId: group.id,
			deviceId: device.id,
		});
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
			intervalSeconds: 3600,
		});
		// No explicit size (device-reported or inspected), but the proxy tallied
		// S3 traffic → the Transfer column shows the payload-sent figure directly,
		// while Snapshot size stays empty.
		await seedBackupRun(sql, {
			deviceId: device.id,
			groupId: group.id,
			machineId: server.machineId,
			outcome: "success",
			bytesUploaded: null,
			s3SentPayloadBytes: 2048, // 2.0 KiB → shown in the Transfer column
			s3SentRawBytes: 3072, // 3.0 KiB
			s3ReceivedPayloadBytes: 512, // 512 B
			s3ReceivedRawBytes: 1024, // 1.0 KiB
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		const runs = page.getByRole("table").last();
		await expect(runs.getByText("s3-srv")).toBeVisible();
		await expect(runs.getByText("2.0 KiB")).toBeVisible();
		// Snapshot size has nothing to show, nor does the (unseeded) snapshot id,
		// plus the empty Duration column.
		await expect(runs.getByRole("cell", { name: "—" })).toHaveCount(3);

		// The S3 traffic breakdown is hidden until the row is expanded. Scoped to
		// the runs table: the repo-stats panel has its own always-visible
		// "S3 traffic (this month)" stat that would otherwise match.
		await expect(runs.getByText(/s3 traffic/i)).toBeHidden();
		await runs.getByRole("button", { name: /show details/i }).click();
		await expect(runs.getByText(/s3 traffic/i)).toBeVisible();
		await expect(runs.getByText(/2\.0 KiB payload \/ 3\.0 KiB raw/i)).toBeVisible();
		await expect(runs.getByText(/512 B payload \/ 1\.0 KiB raw/i)).toBeVisible();
	});

	test("restore run shows its download in the Transfer column and the restored snapshot's size", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "restore-run-group" });
		const device = await seedDevice(sql);
		const server = await seedServer(sql, {
			name: "restore-run-srv",
			groupId: group.id,
			deviceId: device.id,
		});
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
			intervalSeconds: 3600,
		});
		// The backup that produced the snapshot, sized when it ran. 32.0 MiB is
		// seeded nowhere else, so seeing it on the restore row below proves the
		// size came from this run via the lookup.
		await seedBackupRun(sql, {
			deviceId: device.id,
			groupId: group.id,
			machineId: server.machineId,
			outcome: "success",
			bytesUploaded: 33554432, // 32.0 MiB snapshot
			snapshotId: "kfedcba9876543210fedc",
			reportedAgoSecs: 3600,
		});
		// A reported restore of that snapshot: bestool sends the restored-from
		// snapshot id and the proxy's traffic tallies (no bytes_uploaded — nothing
		// is uploaded), and no size of its own (snapshot_logical_bytes unset) —
		// the size resolves from the producing backup run, without waiting for an
		// inspection backfill.
		await seedBackupRun(sql, {
			deviceId: device.id,
			groupId: group.id,
			machineId: server.machineId,
			purpose: "restore",
			outcome: "success",
			bytesUploaded: null,
			snapshotId: "kfedcba9876543210fedc",
			s3SentPayloadBytes: 2048, // 2.0 KiB (kopia metadata chatter)
			s3SentRawBytes: 3072,
			s3ReceivedPayloadBytes: 8388608, // 8.0 MiB → shown in the Transfer column
			s3ReceivedRawBytes: 9437184,
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		const runs = page.getByRole("table").last();
		await expect(runs.getByRole("columnheader", { name: "Transfer" })).toBeVisible();
		// Pick the restore row by its exact Purpose cell — the server name also
		// contains "restore", so a bare hasText filter would match the backup row.
		const restoreRow = runs
			.getByRole("row")
			.filter({ has: page.getByRole("cell", { name: "restore", exact: true }) });
		// Transfer shows the download (received payload) in its own cell…
		await expect(restoreRow.getByRole("cell", { name: "8.0 MiB" })).toBeVisible();
		// …not the upload, which only appears in the collapsed S3-traffic detail.
		await expect(restoreRow.getByText("2.0 KiB")).toBeHidden();
		// Snapshot size shows the producing backup run's figure — the restore row
		// itself was seeded without one, so this pins the lookup, not the backfill.
		// The cell's accessible *name* is its tooltip (MUI Tooltip aria-labels the
		// wrapped span), so select by that and assert the figure as its text.
		await expect(
			restoreRow.getByRole("cell", {
				name: "Size of the snapshot the restore used",
			}),
		).toHaveText("32.0 MiB");
	});

	test("run with both a snapshot size and S3 traffic shows them as distinct columns", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "both-sizes-group" });
		const device = await seedDevice(sql);
		const server = await seedServer(sql, {
			name: "both-sizes-srv",
			groupId: group.id,
			deviceId: device.id,
		});
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
			intervalSeconds: 3600,
		});
		// The full snapshot is much larger than what kopia actually had to send,
		// since it dedupes against data the server already has.
		await seedBackupRun(sql, {
			deviceId: device.id,
			groupId: group.id,
			machineId: server.machineId,
			outcome: "success",
			bytesUploaded: 1048576, // 1.0 MiB snapshot size
			s3SentPayloadBytes: 2048, // 2.0 KiB actually sent
			s3SentRawBytes: 3072,
			s3ReceivedPayloadBytes: 512,
			s3ReceivedRawBytes: 1024,
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		const runs = page.getByRole("table").last();
		await expect(runs.getByText("both-sizes-srv")).toBeVisible();
		await expect(runs.getByText("1.0 MiB")).toBeVisible();
		await expect(runs.getByText("2.0 KiB")).toBeVisible();
	});

	test("backup-now writes a request row; cancel deletes it", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "now-group" });
		const device = await seedDevice(sql);
		const server = await seedServer(sql, {
			name: "now-srv",
			groupId: group.id,
			deviceId: device.id,
		});
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
			intervalSeconds: 3600,
		});
		// A declared type is what enables the button (and names which type runs).
		await seedServerBackupCapability(sql, {
			machineId: server.machineId,
			type: "tamanu-postgres",
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		await page.getByRole("button", { name: /backup now/i }).click();

		await expect(async () => {
			const rows = await sql.query<{ type: string }>(
				`SELECT type FROM backup_requests WHERE machine_id = $1 AND purpose = 'backup'`,
				[server.machineId],
			);
			expect(rows).toHaveLength(1);
			expect(rows[0]!.type).toBe("tamanu-postgres");
		}).toPass();

		await expect(page.getByText(/requested/i)).toBeVisible();

		await page.getByRole("button", { name: /^cancel$/i }).click();
		await expect(async () => {
			const rows = await sql.query(
				`SELECT 1 FROM backup_requests WHERE machine_id = $1`,
				[server.machineId],
			);
			expect(rows).toHaveLength(0);
		}).toPass();
	});

	test("a machine with no declared types has a disabled backup-now button", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "no-types-group" });
		await seedServer(sql, { name: "no-types-srv", groupId: group.id });
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		// The box appears in the "Back up now" panel, but its button is greyed
		// out because it has registered no backup types.
		await expect(page.getByText("no-types-srv")).toBeVisible();
		await expect(
			page.getByRole("button", { name: /backup now/i }),
		).toBeDisabled();
	});

	test("a machine declaring multiple types offers a backup-now per type", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "multi-type-group" });
		const server = await seedServer(sql, {
			name: "multi-srv",
			groupId: group.id,
		});
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
		});
		await seedServerBackupCapability(sql, {
			machineId: server.machineId,
			type: "tamanu-postgres",
		});
		await seedServerBackupCapability(sql, {
			machineId: server.machineId,
			type: "files",
			enabled: false,
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);

		// Both declared types are listed (the disabled-for-schedule one too, since
		// a one-off backup-now overrides the enabled gate), each with its own button.
		const panel = page
			.getByRole("heading", { name: /^machines$/i })
			.locator("..");
		await expect(panel.getByText("tamanu-postgres")).toBeVisible();
		await expect(panel.getByText("files")).toBeVisible();
		await expect(
			panel.getByRole("button", { name: /backup now/i }),
		).toHaveCount(2);
		// The disabled-for-schedule type is marked, so it's clear it still has a
		// button only for on-demand use.
		await expect(panel.getByText(/not scheduled/i)).toBeVisible();

		// Backing up the non-default type writes a request for exactly that type.
		// Scope to the files row so we click that type's button, not the other's.
		await panel
			.getByRole("row")
			.filter({ hasText: "files" })
			.getByRole("button", { name: /backup now/i })
			.click();
		await expect(async () => {
			const rows = await sql.query<{ type: string }>(
				`SELECT type FROM backup_requests WHERE machine_id = $1 AND purpose = 'backup'`,
				[server.machineId],
			);
			expect(rows).toHaveLength(1);
			expect(rows[0]!.type).toBe("files");
		}).toPass();
	});

	test("machines panel shows per-machine next-backup (a lagging box isn't masked)", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "lag-group" });
		const device = await seedDevice(sql);
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
			intervalSeconds: 7200, // 2h
		});
		const ahead = await seedServer(sql, { name: "srv-ahead", groupId: group.id });
		const behind = await seedServer(sql, {
			name: "srv-behind",
			groupId: group.id,
		});
		await seedServerBackupCapability(sql, { machineId: ahead.machineId });
		await seedServerBackupCapability(sql, { machineId: behind.machineId });
		// Only `ahead` has actually backed up.
		await seedBackupRun(sql, {
			deviceId: device.id,
			groupId: group.id,
			machineId: ahead.machineId,
			outcome: "success",
			bytesUploaded: 4096,
			snapshotId: "kdeadbeef0123cafe",
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		const panel = page
			.getByRole("heading", { name: /^machines$/i })
			.locator("..");
		await expect(panel.getByText("Next backup")).toBeVisible();

		// The ahead box's next backup is in the future; it has a snapshot.
		const aheadRow = panel.getByRole("row").filter({ hasText: "srv-ahead" });
		await expect(aheadRow.getByText(/^in /)).toBeVisible();
		await expect(aheadRow.getByText(/kdeadbeef/)).toBeVisible();

		// The behind box never backed up → due now (not masked by `ahead`).
		const behindRow = panel.getByRole("row").filter({ hasText: "srv-behind" });
		await expect(behindRow.getByText("due now")).toBeVisible();
		await expect(behindRow.getByText(/no snapshot yet/i)).toBeVisible();
	});

	test("backup page names the group and cross-links to/from the machine backup section", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "Linky Group" });
		const server = await seedServer(sql, {
			name: "linky-srv",
			groupId: group.id,
		});
		await seedServerBackupCapability(sql, { machineId: server.machineId });
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);

		// Header carries the group name + a back-link to the group page.
		await expect(
			page.getByRole("heading", { name: /linky group backups/i }),
		).toBeVisible();
		await expect(
			page.getByRole("link", { name: /back to linky group/i }),
		).toBeVisible();

		// The box in the "back up now" panel links to its page's backup section.
		// Backups are the box's, so the link goes to the machine, never to a
		// workload on it.
		await page.getByRole("link", { name: "linky-srv" }).click();
		await expect(page).toHaveURL(
			new RegExp(`/fleet/machines/${server.machineId}#backups$`),
		);

		// That backup section links back to the group's backup page.
		await page.getByRole("link", { name: /group backups/i }).click();
		await expect(page).toHaveURL(new RegExp(`/fleet/groups/${group.id}/backups$`));
	});

	test("provisioning with init error shows retry", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "failed-group" });
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "provisioning",
			lastInitError: "kopia repository create failed",
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		await expect(
			page.getByText(/kopia repository create failed/i),
		).toBeVisible();
		await expect(
			page.getByRole("button", { name: /retry repo creation/i }),
		).toBeVisible();
	});

	test("admin can delete (decommission) the config", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "del-group" });
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		// Header delete → confirm dialog → confirm.
		await page.getByRole("button", { name: /^delete$/i }).click();
		const dialog = page.getByRole("dialog");
		await expect(dialog).toBeVisible();
		await dialog.getByRole("button", { name: /^delete$/i }).click();

		// Config is gone → back to the zero-state.
		await expect(page.getByText(/backups not set up/i)).toBeVisible();
		const rows = await sql.query(
			`SELECT 1 FROM server_group_backup_config WHERE group_id = $1`,
			[group.id],
		);
		expect(rows).toHaveLength(0);
	});
});

test.describe("machine backup capabilities", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("machine with no capabilities shows the empty state", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "caps-empty-group" });
		const server = await seedServer(sql, {
			name: "caps-empty-srv",
			groupId: group.id,
		});

		await page.goto(`/fleet/machines/${server.machineId}`);
		await expect(
			page.getByText(/no backup types registered for this machine/i),
		).toBeVisible();
	});

	test("a capability shows its latest snapshot (id + size), copyable", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "snap-caps-group" });
		await seedServerGroupBackupConfig(sql, { groupId: group.id, status: "ready" });
		const device = await seedDevice(sql);
		const server = await seedServer(sql, {
			name: "snap-caps-srv",
			groupId: group.id,
			deviceId: device.id,
		});
		await seedServerBackupCapability(sql, {
			machineId: server.machineId,
			type: "tamanu-postgres",
		});
		await seedBackupRun(sql, {
			deviceId: device.id,
			groupId: group.id,
			machineId: server.machineId,
			outcome: "success",
			bytesUploaded: 2048,
			snapshotId: "k0123456789abcdef0123",
		});

		await page.goto(`/fleet/machines/${server.machineId}`);
		const backups = page.locator("#backups");
		await expect(backups.getByText(/k0123456789/)).toBeVisible();
		await expect(backups.getByText(/2\.0 KiB/)).toBeVisible();
		await expect(
			backups.getByRole("button", { name: /copy snapshot id/i }),
		).toBeVisible();
	});

	test("a capability with no successful backup shows 'no snapshot yet'", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "no-snap-group" });
		await seedServerGroupBackupConfig(sql, { groupId: group.id, status: "ready" });
		const server = await seedServer(sql, {
			name: "no-snap-srv",
			groupId: group.id,
		});
		await seedServerBackupCapability(sql, {
			machineId: server.machineId,
			type: "tamanu-postgres",
		});

		await page.goto(`/fleet/machines/${server.machineId}`);
		const backups = page.locator("#backups");
		await expect(backups.getByText(/no snapshot yet/i)).toBeVisible();
	});

	test("a recent issuance with no newer run shows 'backing up…'", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "inflight-group" });
		await seedServerGroupBackupConfig(sql, { groupId: group.id, status: "ready" });
		const device = await seedDevice(sql);
		const server = await seedServer(sql, {
			name: "inflight-srv",
			groupId: group.id,
			deviceId: device.id,
		});
		await seedServerBackupCapability(sql, { machineId: server.machineId });
		// Credentials issued 10 minutes ago, no run reported since → in flight.
		await seedBackupCredentialIssuance(sql, {
			deviceId: device.id,
			groupId: group.id,
			issuedAgoSecs: 600,
		});

		await page.goto(`/fleet/machines/${server.machineId}`);
		const backups = page.locator("#backups");
		await expect(backups.getByText(/backing up…/i)).toBeVisible();

		// A run reported after the issuance clears the in-flight state.
		await seedBackupRun(sql, {
			deviceId: device.id,
			groupId: group.id,
			machineId: server.machineId,
			outcome: "success",
			snapshotId: "kfreshsnap0001",
		});
		await page.reload();
		await expect(backups.getByText(/backing up…/i)).toBeHidden();
		await expect(backups.getByText(/kfreshsnap/)).toBeVisible();
	});

	test("with no group backup config, capabilities are collapsed behind a message", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "unconfigured-group" });
		const server = await seedServer(sql, {
			name: "unconfigured-srv",
			groupId: group.id,
		});
		// A declared capability, but the group has NO backup config.
		await seedServerBackupCapability(sql, {
			machineId: server.machineId,
			type: "tamanu-postgres",
		});

		await page.goto(`/fleet/machines/${server.machineId}`);
		const backups = page.locator("#backups");
		// The message explains the toggles are inert.
		await expect(backups.getByText(/aren't set up for this group/i)).toBeVisible();
		// The toggle is collapsed (not visible) until expanded.
		const toggle = backups.getByRole("switch", {
			name: /enable tamanu-postgres backups/i,
		});
		await expect(toggle).toBeHidden();
		// …but still reachable: expanding reveals it.
		await backups.getByRole("button", { name: /show backup types/i }).click();
		await expect(toggle).toBeVisible();
	});

	test("toggling a capability switch flips enabled in the DB", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "caps-group" });
		await seedServerGroupBackupConfig(sql, { groupId: group.id, status: "ready" });
		const server = await seedServer(sql, {
			name: "caps-srv",
			groupId: group.id,
		});
		await seedServerBackupCapability(sql, {
			machineId: server.machineId,
			type: "tamanu-postgres",
			enabled: false,
		});

		await page.goto(`/fleet/machines/${server.machineId}`);
		const toggle = page.getByRole("switch", {
			name: /enable tamanu-postgres backups/i,
		});
		await expect(toggle).not.toBeChecked();

		await toggle.click();

		await expect(async () => {
			const rows = await sql.query<{ enabled: boolean }>(
				`SELECT enabled FROM machine_backup_capabilities
				 WHERE machine_id = $1 AND type = 'tamanu-postgres'`,
				[server.machineId],
			);
			expect(rows[0]!.enabled).toBe(true);
		}).toPass();

		await expect(toggle).toBeChecked();
	});
});

test.describe("backups ready: repo maintenance panel", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("zero-state shows maintenance has never run", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "maint-empty" });
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		const panel = page.getByTestId("repo-maintenance");
		await expect(panel.getByText(/no maintenance has run yet/i)).toBeVisible();
	});

	test("a successful run shows Healthy, last-success time, duration, and reclaimed bytes", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "maint-ok" });
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
		});
		await seedBackupMaintenanceRun(sql, {
			groupId: group.id,
			kind: "full",
			outcome: "success",
			bytesReclaimed: 1048576, // 1.0 MiB
			finishedAgoSecs: 3600,
			durationSecs: 180, // renders as "3m"
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		const panel = page.getByTestId("repo-maintenance");
		await expect(panel.getByText("Healthy")).toBeVisible();
		await expect(panel.getByText(/last successful maintenance/i)).toBeVisible();
		// exact: the "Run full maintenance now" button also contains "full".
		await expect(panel.getByText("Full", { exact: true })).toBeVisible();
		// exact: the "Last successful maintenance" caption also contains "success".
		await expect(panel.getByText("success", { exact: true })).toBeVisible();
		await expect(
			panel.getByRole("cell", { name: "3m", exact: true }),
		).toBeVisible();
		await expect(panel.getByText("1.0 MiB")).toBeVisible();
	});

	test("a failed latest run shows the failure and expands to its error", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "maint-failed" });
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
		});
		// An older success, then a newer failure — the panel reads the latest.
		await seedBackupMaintenanceRun(sql, {
			groupId: group.id,
			kind: "full",
			outcome: "success",
			finishedAgoSecs: 7 * 86400,
		});
		await seedBackupMaintenanceRun(sql, {
			groupId: group.id,
			kind: "full",
			outcome: "failure",
			error: "kopia maintenance: connection refused",
			finishedAgoSecs: 3600,
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		const panel = page.getByTestId("repo-maintenance");
		await expect(panel.getByText(/last run failed/i)).toBeVisible();
		// Error detail is hidden until the failed row is expanded.
		await expect(page.getByText(/connection refused/i)).toBeHidden();
		await panel.getByRole("button", { name: /show error/i }).click();
		await expect(page.getByText(/connection refused/i)).toBeVisible();
	});

	test("an in-flight run renders as running", async ({ page, sql }) => {
		const group = await seedServerGroup(sql, { name: "maint-running" });
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
		});
		await seedBackupMaintenanceRun(sql, {
			groupId: group.id,
			kind: "quick",
			outcome: null, // still in flight
			finishedAgoSecs: 60,
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		const panel = page.getByTestId("repo-maintenance");
		// exact: the summary chip reads "Running" (capitalised); the row chip "running".
		await expect(panel.getByText("running", { exact: true })).toBeVisible();
		await expect(panel.getByText("Quick")).toBeVisible();
		// The unfinished run has no Finished, Duration, or Reclaimed value.
		await expect(
			panel.getByRole("cell", { name: "—", exact: true }),
		).toHaveCount(3);
	});

	test("an admin can queue and cancel an on-demand full maintenance run", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "maint-ondemand" });
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		const panel = page.getByTestId("repo-maintenance");

		await panel
			.getByRole("button", { name: /run full maintenance now/i })
			.click();

		// The pending request is echoed as a chip and persisted on the config row.
		await expect(panel.getByText(/full run queued/i)).toBeVisible();
		await expect
			.poll(async () => {
				const rows = await sql.query(
					`SELECT 1 FROM server_group_backup_config
					 WHERE group_id = $1 AND force_full_maintenance_at IS NOT NULL`,
					[group.id],
				);
				return rows.length;
			})
			.toBe(1);

		// Cancelling clears it and restores the request button.
		await panel.getByRole("button", { name: /^cancel$/i }).click();
		await expect(
			panel.getByRole("button", { name: /run full maintenance now/i }),
		).toBeVisible();
		await expect
			.poll(async () => {
				const rows = await sql.query(
					`SELECT 1 FROM server_group_backup_config
					 WHERE group_id = $1 AND force_full_maintenance_at IS NOT NULL`,
					[group.id],
				);
				return rows.length;
			})
			.toBe(0);
	});

	test("a full run in flight disables the request button and spins the indicator", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "maint-fullrunning" });
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
		});
		// A prior success (so the summary reads "Healthy", not "Running") plus a
		// current full run still in flight (no outcome yet).
		await seedBackupMaintenanceRun(sql, {
			groupId: group.id,
			kind: "full",
			outcome: "success",
			finishedAgoSecs: 7 * 86400,
		});
		await seedBackupMaintenanceRun(sql, {
			groupId: group.id,
			kind: "full",
			outcome: null,
			finishedAgoSecs: 30,
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		const panel = page.getByTestId("repo-maintenance");

		// Header shows the spinning "Running" indicator (a progressbar). exact:
		// the in-flight row chip reads lowercase "running".
		await expect(panel.getByText("Running", { exact: true })).toBeVisible();
		await expect(panel.getByRole("progressbar")).toBeVisible();
		// A new full run can't be usefully requested while one is running.
		await expect(
			panel.getByRole("button", { name: /run full maintenance now/i }),
		).toBeDisabled();
	});
});

test.describe("restore window", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("group backups page: allow then disable restores for a machine", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "restore-grp" });
		const server = await seedServer(sql, {
			name: "restore-srv",
			groupId: group.id,
		});
		await seedServerBackupCapability(sql, { machineId: server.machineId });
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
		});

		await page.goto(`/fleet/groups/${group.id}/backups`);
		const row = page.getByRole("row").filter({ hasText: "restore-srv" });

		// Opening the window persists a future expiry attributed to the operator.
		await row.getByRole("button", { name: /allow restores/i }).click();
		await expect(row.getByText(/restores allowed until/i)).toBeVisible();
		const opened = await sql.query<{
			until: string | null;
			by: string | null;
		}>(
			`SELECT restore_allowed_until AS until, restore_allowed_by AS by
			 FROM machines WHERE id = $1`,
			[server.machineId],
		);
		expect(opened[0]!.until).not.toBeNull();
		expect(opened[0]!.by).toBe("admin@localhost");

		// Disabling clears it again.
		await row.getByRole("button", { name: /^disable$/i }).click();
		await expect(
			row.getByRole("button", { name: /allow restores/i }),
		).toBeVisible();
		const closed = await sql.query<{ until: string | null }>(
			`SELECT restore_allowed_until AS until FROM machines WHERE id = $1`,
			[server.machineId],
		);
		expect(closed[0]!.until).toBeNull();
	});

	test("machine detail page: allow then disable restores", async ({
		page,
		sql,
	}) => {
		const group = await seedServerGroup(sql, { name: "restore-detail-grp" });
		const server = await seedServer(sql, {
			name: "restore-detail-srv",
			groupId: group.id,
		});
		await seedServerGroupBackupConfig(sql, {
			groupId: group.id,
			status: "ready",
		});

		await page.goto(`/fleet/machines/${server.machineId}`);
		const backups = page.locator("#backups");

		await backups.getByRole("button", { name: /allow restores/i }).click();
		await expect(
			backups.getByText(/restores are allowed for this machine until/i),
		).toBeVisible();
		const opened = await sql.query<{ until: string | null }>(
			`SELECT restore_allowed_until AS until FROM machines WHERE id = $1`,
			[server.machineId],
		);
		expect(opened[0]!.until).not.toBeNull();

		await backups.getByRole("button", { name: /^disable$/i }).click();
		await expect(
			backups.getByRole("button", { name: /allow restores/i }),
		).toBeVisible();
		const closed = await sql.query<{ until: string | null }>(
			`SELECT restore_allowed_until AS until FROM machines WHERE id = $1`,
			[server.machineId],
		);
		expect(closed[0]!.until).toBeNull();
	});
});
