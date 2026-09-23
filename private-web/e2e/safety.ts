// Driving the operator's safety mode from a test.
//
// Spec: `.workhorse/specs/private-server/safety-modes.md` (id `SAFE`).
//
// The e2e stack runs a debug binary with no tailnet headers, so the server
// treats every request as the development identity and skips the mode check.
// The interface does not: it holds a real read-only session and blocks controls
// above it. A test that reaches for a write- or danger-graded control therefore
// has to raise first, exactly as an operator does.

import { type Page, expect } from "@playwright/test";

/** The mode pill in the app bar, which also reports the current mode. */
export function modeControl(page: Page) {
	return page.getByRole("button", { name: /read-only|write|danger/i }).first();
}

/** Raise the session through the interface, confirming when danger asks. */
export async function raiseTo(page: Page, mode: "write" | "danger") {
	await modeControl(page).click();
	await page.getByRole("menuitem", { name: new RegExp(mode, "i") }).click();
	if (mode === "danger") {
		await page.getByRole("button", { name: "Enter danger mode" }).click();
		// Wait for the dialog to actually go: while it is closing its backdrop
		// still takes the pointer, so a following click lands on nothing.
		await expect(page.getByText("Enter danger mode?")).toHaveCount(0);
	}
	await expect(modeControl(page)).toContainText(new RegExp(mode, "i"));
}

/** Lower back to read-only from the same control. */
export async function lower(page: Page) {
	await modeControl(page).click();
	await page.getByRole("menuitem", { name: /read-only/i }).click();
	await expect(modeControl(page)).toContainText(/read-only/i);
}
