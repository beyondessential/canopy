import { expect, test } from "./test-fixtures";

function uniqueName(label: string): string {
	const id = Math.random().toString(36).slice(2, 10);
	return `e2e-${label}-${id}`;
}

test.describe("mcp tokens page", () => {
	test("renders tokens minted through the API", async ({ page, request }) => {
		const name = uniqueName("seed");
		const res = await request.post("/api/mcp_tokens/mint", {
			data: { name },
		});
		const minted = await res.json();
		expect(minted.secret).toMatch(/^canopy_mcp_/);

		try {
			await page.goto("/settings/mcp-tokens");
			const row = page.getByRole("row").filter({ hasText: name });
			await expect(row).toBeVisible();
			await expect(row.getByText("active")).toBeVisible();
			await expect(row.getByText("never")).toBeVisible();
		} finally {
			await request.post("/api/mcp_tokens/revoke", {
				data: { id: minted.token.id },
			});
		}
	});

	test("mint shows the secret once, then lists the token", async ({
		page,
		request,
	}) => {
		const name = uniqueName("mint");

		await page.goto("/settings/mcp-tokens");
		await page.getByLabel("Token name").fill(name);
		await page.getByRole("button", { name: "Mint token" }).click();

		const dialog = page.getByRole("dialog");
		await expect(dialog.getByText(`Token "${name}" minted`)).toBeVisible();
		const secret = await dialog.getByRole("textbox").inputValue();
		expect(secret).toMatch(/^canopy_mcp_/);
		await dialog.getByRole("button", { name: "Done" }).click();

		const row = page.getByRole("row").filter({ hasText: name });
		await expect(row).toBeVisible();
		// The secret is not re-displayed anywhere on the page.
		await expect(page.getByText(secret)).not.toBeVisible();

		// Clean up via the API so other tests see a quiet list.
		const list = await request.post("/api/mcp_tokens/list", { data: {} });
		const tokens = (await list.json()) as Array<{ id: string; name: string }>;
		const mine = tokens.find((t) => t.name === name);
		expect(mine).toBeTruthy();
		await request.post("/api/mcp_tokens/revoke", {
			data: { id: mine?.id },
		});
	});

	test("revoke flips the token status", async ({ page, request }) => {
		const name = uniqueName("revoke");
		const res = await request.post("/api/mcp_tokens/mint", {
			data: { name },
		});
		const minted = await res.json();

		await page.goto("/settings/mcp-tokens");
		const row = page.getByRole("row").filter({ hasText: name });
		await row.getByRole("button", { name: `revoke ${name}` }).click();

		const dialog = page.getByRole("dialog");
		await expect(dialog.getByText(`Revoke "${name}"?`)).toBeVisible();
		await dialog.getByRole("button", { name: "Revoke" }).click();

		await expect(row.getByText("revoked")).toBeVisible();
		await expect(
			row.getByRole("button", { name: `revoke ${name}` }),
		).not.toBeVisible();

		// Idempotence at the API level: revoking again still succeeds.
		const again = await request.post("/api/mcp_tokens/revoke", {
			data: { id: minted.token.id },
		});
		expect(again.ok()).toBeTruthy();
	});

	test("rejects an empty token name with an inline error", async ({
		page,
	}) => {
		await page.goto("/settings/mcp-tokens");
		await page.getByLabel("Token name").fill("   ");
		await page.getByRole("button", { name: "Mint token" }).click();
		await expect(page.getByText("Token name cannot be empty")).toBeVisible();
	});

	test("mints a token with the triage grant and lists its grants", async ({
		page,
		request,
	}) => {
		const name = uniqueName("triage");
		const plain = uniqueName("plain");

		await page.goto("/settings/mcp-tokens");
		await expect(page.getByLabel("Triage grant")).not.toBeChecked();

		await page.getByLabel("Token name").fill(plain);
		await page.getByRole("button", { name: "Mint token" }).click();
		await page.getByRole("dialog").getByRole("button", { name: "Done" }).click();

		await page.getByLabel("Token name").fill(name);
		await page.getByLabel("Triage grant").check();
		await page.getByRole("button", { name: "Mint token" }).click();
		await page.getByRole("dialog").getByRole("button", { name: "Done" }).click();

		await expect(page.getByLabel("Triage grant")).not.toBeChecked();
		await expect(
			page.getByRole("row").filter({ hasText: name }),
		).toContainText("triage");
		await expect(
			page.getByRole("row").filter({ hasText: plain }),
		).not.toContainText("triage");

		const list = await request.post("/api/mcp_tokens/list", { data: {} });
		const tokens = (await list.json()) as Array<{
			id: string;
			name: string;
			grants: string[];
		}>;
		for (const token of tokens.filter((t) => [name, plain].includes(t.name))) {
			expect(token.grants).toEqual(token.name === name ? ["triage"] : []);
			await request.post("/api/mcp_tokens/revoke", { data: { id: token.id } });
		}
	});

	test("mints a token with the notes grant and lists it", async ({
		page,
		request,
	}) => {
		const name = uniqueName("notes");

		await page.goto("/settings/mcp-tokens");
		await expect(page.getByLabel("Notes grant")).not.toBeChecked();
		await page.getByLabel("Token name").fill(name);
		await page.getByLabel("Notes grant").check();
		await page.getByRole("button", { name: "Mint token" }).click();
		await page.getByRole("dialog").getByRole("button", { name: "Done" }).click();

		await expect(
			page.getByRole("row").filter({ hasText: name }),
		).toContainText("notes");

		const list = await request.post("/api/mcp_tokens/list", { data: {} });
		const tokens = (await list.json()) as Array<{
			id: string;
			name: string;
			grants: string[];
		}>;
		const mine = tokens.find((t) => t.name === name);
		expect(mine?.grants).toEqual(["notes"]);
		await request.post("/api/mcp_tokens/revoke", { data: { id: mine?.id } });
	});
});
