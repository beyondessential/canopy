import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, expect, it, vi } from "vitest";
import type { ServerInfo, ServerRank } from "../types";
import MigrationTestsSection from "./MigrationTestsSection";

const GROUP = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";

function server(id: string, name: string, rank: ServerRank | null): ServerInfo {
	return { id, name, rank, type: "tamanu-central" } as ServerInfo;
}

afterEach(() => {
	vi.unstubAllGlobals();
});

it("lists rows by environment, then by name", async () => {
	const servers = [
		server("1", "kamaka demo", "demo"),
		server("2", "drifting", "production"),
		server("3", "kamaka clone", "clone"),
		server("4", "kamaka", "production"),
		server("5", "aardvark", null),
	];
	const verdicts = [
		{ server_id: "1", target_version: "2.60.0", verdict: "failed", latest: null },
		{ server_id: "2", target_version: "2.60.0", verdict: "passed", latest: null },
		{ server_id: "3", target_version: "2.60.0", verdict: "nottested", latest: null },
		{ server_id: "4", target_version: "2.60.0", verdict: "passed", latest: null },
		{ server_id: "5", target_version: "2.60.0", verdict: "failed", latest: null },
	];
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

	render(
		<MemoryRouter>
			<MigrationTestsSection groupId={GROUP} servers={servers} />
		</MemoryRouter>,
	);

	const rows = await screen.findAllByTestId("migration-test-row");
	expect(rows.map((row) => row.querySelector("a")?.textContent)).toEqual([
		"drifting",
		"kamaka",
		"kamaka clone",
		"kamaka demo",
		"aardvark",
	]);
});
