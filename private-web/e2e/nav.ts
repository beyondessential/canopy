// The app's sidebar navigation, for tests that move around by it.

import type { Page } from "@playwright/test";

/** The sidebar's list of sections and their pages. */
export function sidebar(page: Page) {
	return page.getByRole("navigation", { name: "Main" });
}

/** A page listed beneath a section in the sidebar, by its exact name. */
export function sidebarLink(page: Page, name: string) {
	return sidebar(page).getByRole("link", { name, exact: true });
}
