import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, expect, it, vi } from "vitest";
import type { ServerInfo, ServerRank } from "../types";
import MigrationTestsSection from "./MigrationTestsSection";

vi.mock("../hooks/useSafetyMode", () => ({
	useSafetyMode: () => ({ mode: "danger", busy: false }),
}));

const GROUP = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";

function server(id: string, name: string, rank: ServerRank | null): ServerInfo {
	return { id, name, rank, type: "tamanu-central" } as ServerInfo;
}

function serve(verdicts: unknown[]) {
	vi.stubGlobal(
		"fetch",
		vi.fn(
			async () =>
				new Response(JSON.stringify(verdicts), {
					status: 200,
					headers: { "content-type": "application/json" },
				}),
		),
	);
}

afterEach(() => {
	vi.unstubAllGlobals();
});

it("lists rows by environment, then problems first, then by name", async () => {
	const servers = [
		server("1", "kamaka demo", "demo"),
		server("2", "drifting", "production"),
		server("3", "kamaka clone", "clone"),
		server("4", "kamaka", "production"),
		server("5", "aardvark", null),
	];
	const verdicts = [
		{ server_id: "1", target_version: "2.60.0", verdict: "failed", latest: null, set_up: true },
		{ server_id: "2", target_version: "2.60.0", verdict: "passed", latest: null, set_up: true },
		{ server_id: "3", target_version: "2.60.0", verdict: "nottested", latest: null, set_up: true },
		{ server_id: "4", target_version: "2.60.0", verdict: "failed", latest: null, set_up: true },
		{ server_id: "5", target_version: "2.60.0", verdict: "failed", latest: null, set_up: true },
	];
	serve(verdicts);

	render(
		<MemoryRouter>
			<MigrationTestsSection groupId={GROUP} servers={servers} />
		</MemoryRouter>,
	);

	const rows = await screen.findAllByTestId("migration-test-row");
	expect(rows.map((row) => row.querySelector("a")?.textContent)).toEqual([
		"kamaka",
		"drifting",
		"kamaka clone",
		"kamaka demo",
		"aardvark",
	]);
});

it("gives a row only to servers set up for testing, and counts the rest", async () => {
	const servers = [
		server("1", "kamaka", "production"),
		server("2", "kamaka facility", "production"),
		server("3", "kamaka clone", "clone"),
	];
	serve([
		{ server_id: "1", target_version: "2.60.0", verdict: "passed", latest: null, set_up: true },
		{ server_id: "2", target_version: "2.60.0", verdict: "nottested", latest: null, set_up: false },
		{ server_id: "3", target_version: "2.61.0", verdict: "nottested", latest: null, set_up: false },
	]);

	render(
		<MemoryRouter>
			<MigrationTestsSection groupId={GROUP} servers={servers} />
		</MemoryRouter>,
	);

	const rows = await screen.findAllByTestId("migration-test-row");
	expect(rows.map((row) => row.querySelector("a")?.textContent)).toEqual(["kamaka"]);
	expect(screen.getByText("to 2.60.0")).toBeTruthy();
	expect(screen.queryByText("Upgrading to")).toBeNull();
	expect(
		screen.getByTestId("migration-tests-not-set-up").textContent,
	).toContain("2 other servers are not set up for testing");
});
