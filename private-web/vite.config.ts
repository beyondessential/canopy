/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Defaults match `just watch-private-api`. The e2e fixture overrides this to
// point at the per-run private-server it spawns on a dynamic port.
const PRIVATE_SERVER = process.env.VITE_PROXY_TARGET ?? "http://127.0.0.1:8081";

export default defineConfig({
	plugins: [react()],
	// Each e2e worker runs a dev server of its own, and servers sharing one
	// dependency cache rewrite each other's pre-bundled modules, which a page
	// already loading then fails to fetch. The fixture gives each its own.
	cacheDir: process.env.VITE_CACHE_DIR ?? "node_modules/.vite",
	build: {
		outDir: "dist",
		emptyOutDir: true,
	},
	server: {
		port: 8090,
		strictPort: true,
		proxy: {
			"/api": PRIVATE_SERVER,
		},
	},
	test: {
		environment: "jsdom",
		include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
		globals: true,
	},
});
