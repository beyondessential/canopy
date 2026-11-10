// Config for the P6 design loop's capture rig, kept out of private-web/e2e so
// the harness is never mistaken for a test. Run from private-web, so Playwright
// and the fixture's workspace paths resolve:
//
//   cd private-web && env -u PORT npx playwright test \
//     --config ../.workhorse/plans/p6/design/capture/playwright.config.ts
//
// Exports a plain object rather than calling defineConfig: this directory has no
// node_modules above it, so the config itself can import nothing.

export default {
	testDir: ".",
	reporter: [["list"] as const],
	timeout: 300_000,
	expect: { timeout: 10_000 },
	workers: 1,
	use: {
		screenshot: "off" as const,
		trace: "off" as const,
		video: "off" as const,
		viewport: { width: 1440, height: 1000 },
	},
	projects: [{ name: "chromium", use: { browserName: "chromium" as const } }],
};
