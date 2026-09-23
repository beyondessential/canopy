import { expect, test } from "./test-fixtures";
import {
	resetSeededTables,
	seedCluster,
	seedIssue,
	seedServerGroup,
} from "./seed";

/// A registered cluster's page: its health, reachability and checks, and the
/// applications it hosts.
///
/// spec: K8S
test.describe("a cluster's page", () => {
	test.beforeEach(async ({ sql }) => {
		await resetSeededTables(sql);
	});

	test("presents the cluster's health, threshold, checks and applications", async ({
		page,
		sql,
	}) => {
		const cluster = await seedCluster(sql, { name: "ops-main" });
		await seedIssue(sql, {
			clusterId: cluster.id,
			source: "kubernetes",
			ref: "node-pools",
			message: "gpu: nodes launched but never registered",
		});
		const group = await seedServerGroup(sql, { name: "Harbour" });
		await sql.query(
			`INSERT INTO applications (type, name, rank, group_id, kubernetes_cluster_id)
			 VALUES ('tamanu-central', 'central', 'demo', $1, $2)`,
			[group.id, cluster.id],
		);

		await page.goto(`/fleet/clusters/${cluster.id}`);

		await expect(
			page.getByRole("heading", { name: "ops-main", level: 1 }),
		).toBeVisible();
		await expect(page.getByText("Unhealthy")).toBeVisible();
		await expect(page.getByText("Unreachable after")).toBeVisible();
		await expect(page.getByText("5m", { exact: true })).toBeVisible();
		await expect(page.getByRole("link", { name: "node-pools" })).toBeVisible();
		// A cluster's issue is the cluster's, not one of Canopy's own.
		await expect(page.getByText("Canopy: node-pools")).toHaveCount(0);
		await expect(page.getByText("reachability", { exact: true })).toBeVisible();

		const hosted = page.getByTestId("applications-on-cluster");
		await expect(hosted.getByText("Applications (1)")).toBeVisible();
		await expect(hosted.getByText("Harbour")).toBeVisible();
		await expect(hosted.getByText("central")).toBeVisible();
	});

	test("the registry links each registered cluster to its page", async ({
		page,
		sql,
	}) => {
		const cluster = await seedCluster(sql, { name: "ops-linked" });

		await page.goto("/settings/clusters");
		await page.getByRole("link", { name: "ops-linked" }).click();

		await expect(page).toHaveURL(new RegExp(`/fleet/clusters/${cluster.id}$`));
		await expect(
			page.getByRole("heading", { name: "ops-linked", level: 1 }),
		).toBeVisible();
	});

	/// A cluster belongs to no group, so its checks are silenced against the
	/// cluster alone.
	///
	/// spec: CHK#silences-follow-the-event
	test("a cluster's check is silenced on the cluster, with no group scope offered", async ({
		page,
		sql,
	}) => {
		const cluster = await seedCluster(sql, { name: "ops-quiet" });
		await seedIssue(sql, {
			clusterId: cluster.id,
			source: "kubernetes",
			ref: "workloads-running",
			message: "62% of the cluster's workload is ready",
		});

		await page.goto(`/fleet/clusters/${cluster.id}`);

		await page.getByRole("button", { name: "Silence workloads-running" }).click();
		await expect(
			page.getByRole("button", { name: "For this cluster" }),
		).toBeVisible();
		await expect(
			page.getByRole("button", { name: "For this group" }),
		).toHaveCount(0);

		await page.getByRole("button", { name: "For this cluster" }).click();

		await expect(
			page.getByRole("heading", { name: /Silenced refs/ }),
		).toBeVisible();
		await expect(
			page.getByRole("code").filter({ hasText: "kubernetes/workloads-running" }),
		).toBeVisible();
		await expect(page.getByText("Healthy")).toBeVisible();
	});

	test("an admin changes how long the cluster may go unheard", async ({
		page,
		sql,
	}) => {
		const cluster = await seedCluster(sql, { name: "ops-edit" });

		await page.goto(`/fleet/clusters/${cluster.id}`);
		await page.getByRole("link", { name: "Edit" }).click();

		await page.getByLabel("minutes").fill("20");
		await page.getByRole("button", { name: "Save" }).click();

		await expect(page).toHaveURL(new RegExp(`/fleet/clusters/${cluster.id}$`));
		await expect(page.getByText("20m", { exact: true })).toBeVisible();
	});

	test("a non-admin reads the page but is offered no edit", async ({
		page,
		sql,
	}) => {
		const cluster = await seedCluster(sql, { name: "ops-readonly" });
		await page.route("**/api/commons/is_current_user_admin", (route) =>
			route.fulfill({
				status: 200,
				contentType: "application/json",
				body: "false",
			}),
		);

		await page.goto(`/fleet/clusters/${cluster.id}`);

		await expect(
			page.getByRole("heading", { name: "ops-readonly", level: 1 }),
		).toBeVisible();
		await expect(page.getByRole("link", { name: "Edit" })).toHaveCount(0);
	});
});
