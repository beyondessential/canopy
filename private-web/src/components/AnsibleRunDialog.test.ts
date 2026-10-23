import { describe, expect, it } from "vitest";
import { buildCommand, shellArg, typeLabel } from "./AnsibleRunDialog";

const base = {
	vcs: "git" as const,
	origin: "https://canopy.example.ts.net",
	leaseId: "7c1e9a52-40b3-4d7e-9f0a-2e6b51c8d913",
	playbook: "tamanu-install.yml",
	limit: "",
	tags: [],
	skipTags: [],
	extra: "",
};

describe("buildCommand", () => {
	it("hands the lease to the run and leaves the checkout out of what is copied", () => {
		const { checkout, copied } = buildCommand(base);
		expect(checkout).toBe("cd <infra>");
		expect(copied).toBe(
			[
				"git switch main && git pull --ff-only",
				"cd ansible",
				"CANOPY_URL=https://canopy.example.ts.net CANOPY_LEASE=7c1e9a52-40b3-4d7e-9f0a-2e6b51c8d913 uv run ansible-playbook -i inventory/canopy.yml tamanu-install.yml",
			].join("\n"),
		);
	});

	it("updates with jj where chosen", () => {
		const { copied } = buildCommand({ ...base, vcs: "jj" });
		expect(copied.split("\n")[0]).toBe("jj git fetch && jj new main@origin");
	});

	it("adds the limit, the tags and the extra arguments, quoting what a shell would split", () => {
		const { copied } = buildCommand({
			...base,
			limit: "facility:!example-facility-b",
			tags: ["app", "base"],
			skipTags: ["tailscale"],
			extra: "-vv --check",
		});
		expect(copied).toContain("--limit 'facility:!example-facility-b'");
		expect(copied).toContain("--tags app,base --skip-tags tailscale -vv --check");
	});
});

describe("shellArg", () => {
	it("leaves a plain word and quotes the rest", () => {
		expect(shellArg("tamanu-install.yml")).toBe("tamanu-install.yml");
		expect(shellArg("a b")).toBe("'a b'");
		expect(shellArg("it's")).toBe("'it'\\''s'");
	});
});

describe("typeLabel", () => {
	it("reads a type as words", () => {
		expect(typeLabel("tamanu-central-api")).toBe("Tamanu central api");
	});
});
