import { expect, test } from "./test-fixtures";

async function callApi(
	request: { post: (url: string, opts: { data: unknown }) => Promise<unknown> },
	module: string,
	fn: string,
	params: Record<string, unknown> = {},
) {
	await request.post(`/api/${module}/${fn}`, {
		data: params,
	});
}

function uniqueEmail(label: string): string {
	const id = Math.random().toString(36).slice(2, 10);
	return `e2e-${label}-${id}@example.invalid`;
}

test.describe("admins page", () => {
	test("renders existing admins from the API", async ({ page, request }) => {
		const seeded = uniqueEmail("seed");
		await callApi(request, "admins", "add", { email: seeded });

		try {
			await page.goto("/settings/admins");
			await expect(page.getByText(seeded)).toBeVisible();
		} finally {
			await callApi(request, "admins", "delete", { email: seeded });
		}
	});

	test("delete button removes the admin", async ({ page, request }) => {
		const seeded = uniqueEmail("del");
		await callApi(request, "admins", "add", { email: seeded });

		await page.goto("/settings/admins");
		await expect(page.getByText(seeded)).toBeVisible();

		await page.getByRole("button", { name: `delete ${seeded}` }).click();
		await expect(page.getByText(seeded)).not.toBeVisible();
	});

	test("add form creates an admin", async ({ page, request }) => {
		const fresh = uniqueEmail("add");

		await page.goto("/settings/admins");
		await page.getByLabel("Email").fill(fresh);
		await page.getByRole("button", { name: "Add admin" }).click();

		try {
			await expect(page.getByText(fresh)).toBeVisible();
		} finally {
			await callApi(request, "admins", "delete", { email: fresh });
		}
	});

	test("rejects empty email with an inline error", async ({ page }) => {
		await page.goto("/settings/admins");
		// Browser email validation will block submit when empty;
		// circumvent with a space-only value to hit our own check.
		await page.getByLabel("Email").fill("   ");
		await page.getByRole("button", { name: "Add admin" }).click();
		await expect(page.getByText("Email cannot be empty")).toBeVisible();
	});
});
