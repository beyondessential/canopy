import { expect, test } from "./test-fixtures";

function uniqueName(label: string): string {
	const id = Math.random().toString(36).slice(2, 10);
	return `e2e-${label}-${id}`;
}

test.describe("kubernetes clusters settings page", () => {
	test("registering mints a credential and leaves a draft awaiting the relay", async ({
		page,
	}) => {
		const name = uniqueName("register");

		await page.goto("/settings/clusters");
		await page.getByRole("button", { name: "Register cluster" }).click();

		const dialog = page.getByRole("dialog");
		await dialog.getByLabel("Cluster name").fill(name);
		await dialog.getByRole("button", { name: "Register" }).click();

		// The relay credential is shown once, and the wizard waits for the relay
		// to connect — nothing has answered, so it does not register.
		await expect(
			dialog.getByText(/shown once/i),
		).toBeVisible();
		await expect(
			dialog.getByRole("button", { name: "Download key file" }),
		).toBeVisible();
		await expect(
			dialog.getByText(/waiting for the relay to connect/i),
		).toBeVisible();

		await dialog.getByRole("button", { name: "Close" }).click();

		// The unfinished registration is kept as a draft.
		const row = page.getByRole("row").filter({ hasText: name });
		await expect(row).toBeVisible();
		await expect(row.getByText("awaiting relay")).toBeVisible();
	});

	test("a registered cluster shows as answering", async ({
		page,
		request,
		sql,
	}) => {
		const name = uniqueName("registered");

		// Register through the API, then simulate the relay having answered by
		// stamping last_answered_at, and confirm — the flow the wizard drives.
		const res = await request.post("/api/kubernetes_clusters/register", {
			data: { name },
		});
		const started = await res.json();
		await sql.query(
			"UPDATE kubernetes_clusters SET last_answered_at = NOW() WHERE id = $1",
			[started.cluster.id],
		);
		await request.post("/api/kubernetes_clusters/confirm", {
			data: { id: started.cluster.id },
		});

		await page.goto("/settings/clusters");
		const row = page.getByRole("row").filter({ hasText: name });
		await expect(row).toBeVisible();
		await expect(row.getByText("answering")).toBeVisible();
	});

	test("a draft can be removed", async ({ page, request }) => {
		const name = uniqueName("remove");
		await request.post("/api/kubernetes_clusters/register", {
			data: { name },
		});

		await page.goto("/settings/clusters");
		const row = page.getByRole("row").filter({ hasText: name });
		await expect(row).toBeVisible();
		await row.getByRole("button", { name: `remove ${name}` }).click();

		const dialog = page.getByRole("dialog");
		await dialog.getByRole("button", { name: "Remove" }).click();
		await expect(row).not.toBeVisible();
	});

	test("rejects an empty cluster name", async ({ page }) => {
		await page.goto("/settings/clusters");
		await page.getByRole("button", { name: "Register cluster" }).click();
		const dialog = page.getByRole("dialog");
		await dialog.getByLabel("Cluster name").fill("   ");
		await dialog.getByRole("button", { name: "Register" }).click();
		await expect(dialog.getByText("A cluster needs a name")).toBeVisible();
	});
});
