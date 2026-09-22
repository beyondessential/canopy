// Shared Playwright test object. Every spec gets:
// - `stack`: a worker-scoped private-server + Vite + per-worker DB.
// - `baseURL`: wired to the stack's Vite URL (so `page.goto("/foo")`
//   just works).
// - `sql`: a worker-scoped pg client against the same per-worker DB,
//   used by the seed helpers in `seed.ts`.
// - `page`: a page whose safety-mode session starts raised to danger, unless
//   the spec opts out with `test.use({ safetyMode: "read-only" })`.

import { test as base, expect } from "@playwright/test";

import { startStack, type StackHandle } from "./fixture";
import { connect, type Sql } from "./seed";

type Fixtures = {
	stack: StackHandle;
	sql: Sql;
};

type Options = {
	/**
	 * The mode each page's session starts in (see the SAFE spec).
	 *
	 * An operator's session starts read-only, and the interface blocks every
	 * control above that. A spec about backups or groups is not about safety
	 * modes, so by default the fixture raises each session to danger the way an
	 * operator would, through the real raise endpoint, before the page sees it.
	 * Specs that exercise the modes themselves opt out and start read-only.
	 */
	safetyMode: "danger" | "read-only";
};

export const test = base.extend<Options, Fixtures>({
	safetyMode: ["danger", { option: true }],
	page: async ({ page, safetyMode }, use) => {
		if (safetyMode === "danger") {
			// Answer the page's first request for a session with that session
			// already raised. The raise is real: the server records it, and the
			// page carries the same session identifier from then on.
			await page.route("**/api/safety/session", async (route) => {
				try {
					const minted = await route.fetch();
					const session = (await minted.json()) as { id: string };
					const raised = await page.request.post("/api/safety/raise", {
						headers: { "x-canopy-session": session.id },
						data: { mode: "danger" },
					});
					await route.fulfill({ response: raised });
				} catch (error) {
					// A navigation cancels the request mid-flight and disposes its
					// response; the page asks again, and that request is raised in
					// turn. Anything else is a real failure.
					if (!/disposed|closed|cancel|abort|ended/i.test(String(error))) {
						throw error;
					}
				}
			});
		}
		await use(page);
		// A session request can still be in flight as the test ends; let it go
		// rather than report it against whatever the test was about.
		await page.unrouteAll({ behavior: "ignoreErrors" });
	},
	stack: [
		async ({}, use) => {
			const handle = await startStack();
			try {
				await use(handle);
			} finally {
				await handle.stop();
			}
		},
		{ scope: "worker" },
	],
	baseURL: async ({ stack }, use) => {
		await use(stack.baseUrl);
	},
	sql: [
		async ({ stack }, use) => {
			const sql = await connect(stack.databaseUrl);
			try {
				await use(sql);
			} finally {
				await sql.end();
			}
		},
		{ scope: "worker" },
	],
});

export { expect };
