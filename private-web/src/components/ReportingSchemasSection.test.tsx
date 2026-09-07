import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import ReportingSchemasSection from "./ReportingSchemasSection";

type Pair = {
	group_id: string;
	version_id: string;
	version: string;
	state: "awaiting" | "built" | "failed";
	error?: string | null;
	requested: boolean;
};

const GROUP = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";

function pair(over: Partial<Pair> = {}): Pair {
	return {
		group_id: GROUP,
		version_id: "11111111-1111-1111-1111-111111111111",
		version: "2.60.0",
		state: "awaiting",
		error: null,
		requested: false,
		...over,
	};
}

/// Answer `for_group` with `pairs`, and `build` with either a 200 or a
/// ProblemDetails the component is expected to surface.
function stubApi(pairs: Pair[], build: { status: number; body?: unknown } = { status: 200 }) {
	const calls: { url: string; body: unknown }[] = [];
	const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
		const url = typeof input === "string" ? input : input.toString();
		calls.push({ url, body: init?.body ? JSON.parse(String(init.body)) : undefined });

		if (url.includes("reporting_schemas/build")) {
			return new Response(JSON.stringify(build.body ?? {}), {
				status: build.status,
				headers: { "content-type": "application/json" },
			});
		}
		return new Response(JSON.stringify(pairs), {
			status: 200,
			headers: { "content-type": "application/json" },
		});
	});

	vi.stubGlobal("fetch", fetch);
	return calls;
}

afterEach(() => {
	vi.unstubAllGlobals();
});

describe("a pair's state reads off the chip", () => {
	it("shows a built pair as built", async () => {
		stubApi([pair({ state: "built" })]);
		render(<ReportingSchemasSection groupId={GROUP} />);

		expect(await screen.findByText("Built")).toBeTruthy();
	});

	it("shows an unbuilt pair as awaiting, which is not a failure", async () => {
		stubApi([pair()]);
		render(<ReportingSchemasSection groupId={GROUP} />);

		expect(await screen.findByText("Awaiting build")).toBeTruthy();
		expect(screen.queryByText("Failed")).toBeNull();
	});

	it("carries the builder's own error on the failed chip", async () => {
		stubApi([pair({ state: "failed", error: "views did not compile" })]);
		render(<ReportingSchemasSection groupId={GROUP} />);

		fireEvent.mouseOver(await screen.findByText("Failed"));
		expect(await screen.findByText("views did not compile")).toBeTruthy();
	});

	it("falls back where the build reported no description", async () => {
		stubApi([pair({ state: "failed", error: null })]);
		render(<ReportingSchemasSection groupId={GROUP} />);

		fireEvent.mouseOver(await screen.findByText("Failed"));
		expect(await screen.findByText("the build failed")).toBeTruthy();
	});
});

describe("asking for a build", () => {
	it("offers a first build on an unbuilt pair and a rebuild on a settled one", async () => {
		stubApi([
			pair({ version_id: "1", version: "2.59.0", state: "awaiting" }),
			pair({ version_id: "2", version: "2.60.0", state: "built" }),
			pair({ version_id: "3", version: "2.61.0", state: "failed" }),
		]);
		render(<ReportingSchemasSection groupId={GROUP} />);

		expect(await screen.findByText("Build sooner")).toBeTruthy();
		expect(screen.getAllByText("Build again")).toHaveLength(2);
	});

	it("names the pair rather than the group's latest version", async () => {
		const calls = stubApi([pair({ version_id: "abc", version: "2.59.0" })]);
		render(<ReportingSchemasSection groupId={GROUP} />);

		fireEvent.click(await screen.findByText("Build sooner"));

		await waitFor(() => {
			const ask = calls.find((c) => c.url.includes("reporting_schemas/build"));
			expect(ask?.body).toEqual({ group_id: GROUP, version_id: "abc" });
		});
	});

	it("replaces the control once an ask is recorded, so it is not asked twice", async () => {
		stubApi([pair({ requested: true })]);
		render(<ReportingSchemasSection groupId={GROUP} />);

		expect(await screen.findByText("Build asked for")).toBeTruthy();
		expect(screen.queryByText("Build sooner")).toBeNull();
	});

	it("surfaces a refused ask rather than looking like it worked", async () => {
		stubApi([pair()], {
			status: 403,
			body: { title: "insufficient permissions: admin role required" },
		});
		render(<ReportingSchemasSection groupId={GROUP} />);

		fireEvent.click(await screen.findByText("Build sooner"));

		expect(await screen.findByText(/insufficient permissions/)).toBeTruthy();
	});
});

describe("a group with nothing to build", () => {
	// Two different reasons reach the same empty answer: no builder is declared
	// for the group, or nothing in it reports a published version. Naming both
	// is what stops an operator reading an absent builder as a backlog.
	it("names both reasons rather than showing an empty table", async () => {
		stubApi([]);
		render(<ReportingSchemasSection groupId={GROUP} />);

		const empty = await screen.findByText(/nothing to build for this group/i);
		expect(empty.textContent).toMatch(/no builder is declared/i);
		expect(empty.textContent).toMatch(/reports a published version/i);
		expect(screen.queryByText("Version")).toBeNull();
	});
});
