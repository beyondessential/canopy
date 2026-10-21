import { describe, expect, it } from "vitest";
import type { CatalogueView } from "../../types";
import { checkScope, diffScope, formatScope, parseScope } from "./yamlScope";

const catalogue = {
	variables: [
		{
			name: "elastic_agent_enabled",
			description: null,
			schema: { type: "boolean" },
			default: null,
			secret: false,
			scopes: null,
			applications: null,
			reserved: false,
		},
		{
			name: "salt",
			description: null,
			schema: { type: "string" },
			default: null,
			secret: true,
			scopes: null,
			applications: null,
			reserved: false,
		},
		{
			name: "ansible_host",
			description: null,
			schema: { type: "string" },
			default: null,
			secret: false,
			scopes: ["machine"],
			applications: null,
			reserved: false,
		},
		{
			name: "canopy_intent",
			description: null,
			schema: { type: "string" },
			default: null,
			secret: false,
			scopes: null,
			applications: null,
			reserved: true,
		},
	],
	playbooks: [],
	tags: {},
	source_commit: null,
	read_at: "2026-01-01T00:00:00Z",
	problem: null,
	problem_detail: null,
} as unknown as CatalogueView;

describe("formatScope", () => {
	it("writes a variable a line and a secret as a tag", () => {
		expect(
			formatScope([
				{ name: "tamanu_version", value: "v2.65.0", secret: false },
				{ name: "salt", secret: true },
				{ name: "elastic_agent_enabled", value: true, secret: false },
			]),
		).toBe(
			"elastic_agent_enabled: true\nsalt: !secret\ntamanu_version: v2.65.0",
		);
	});
});

describe("parseScope", () => {
	it("reads values as an inventory file would", () => {
		const { entries, problems } = parseScope("a: yes\nb: no\nc: 3\nd: x y\n");
		expect(problems).toEqual([]);
		expect(entries.map((e) => [e.name, e.value])).toEqual([
			["a", true],
			["b", false],
			["c", 3],
			["d", "x y"],
		]);
	});

	it("reads a scalar as PyYAML does, at any depth, so it is stored as an import would store it", () => {
		const { entries } = parseScope("a: 1:30\nb: 0644\nc:\n  d: [1:30, yes, 1e3]\n");
		expect(entries.map((e) => [e.name, e.value])).toEqual([
			["a", 90],
			["b", 420],
			["c", { d: [90, true, "1e3"] }],
		]);
	});

	it("reads a secret kept and a secret retyped", () => {
		const { entries } = parseScope("salt: !secret\ntoken: !secret abc\n");
		expect(entries[0]).toMatchObject({ secret: true, kept: true });
		expect(entries[1]).toMatchObject({ secret: true, kept: false, value: "abc" });
	});

	it("names the line a fault is on", () => {
		const { problems } = parseScope("a: 1\nb: [1\n");
		expect(problems[0].line).toBeGreaterThanOrEqual(2);
	});

	it("refuses anything but a mapping and a name set twice", () => {
		expect(parseScope("- a\n- b\n").problems).toHaveLength(1);
		expect(parseScope("a: 1\na: 2\n").problems).toHaveLength(1);
	});

	it("reads an object value and the line its name is on", () => {
		const { entries } = parseScope("a: 1\niam:\n  access_key_id: AKIA\n  secret: x\n");
		expect(entries[1]).toMatchObject({
			name: "iam",
			line: 2,
			value: { access_key_id: "AKIA", secret: "x" },
		});
	});
});

describe("diffScope", () => {
	const before = [
		{ name: "elastic_agent_enabled", value: true, secret: false },
		{ name: "salt", secret: true },
		{ name: "old", value: 1, secret: false },
	];

	it("sets only what changed and removes what is gone", () => {
		const parsed = parseScope("elastic_agent_enabled: false\nsalt: !secret\nnew: 2\n");
		expect(diffScope(before, parsed, catalogue)).toEqual({
			set: [
				{ name: "elastic_agent_enabled", value: false, secret: false },
				{ name: "new", value: 2, secret: false },
			],
			remove: ["old"],
		});
	});

	it("leaves what is unchanged and a kept secret alone", () => {
		const parsed = parseScope(
			"elastic_agent_enabled: yes\nsalt: !secret\nold: 1\n",
		);
		expect(diffScope(before, parsed, catalogue)).toEqual({ set: [], remove: [] });
	});

	it("sets a retyped secret, and one the catalogue calls secret, as secret", () => {
		const parsed = parseScope("elastic_agent_enabled: true\nsalt: !secret pepper\nold: 1\n");
		expect(diffScope(before, parsed, catalogue).set).toEqual([
			{ name: "salt", value: "pepper", secret: true },
		]);
		const plain = parseScope("salt: pepper\n");
		expect(diffScope([], plain, catalogue).set).toEqual([
			{ name: "salt", value: "pepper", secret: true },
		]);
	});
});

describe("checkScope", () => {
	const check = (text: string, scope: "group" | "environment" | "machine" = "environment") =>
		checkScope(parseScope(text), [{ name: "salt", secret: true }], scope, [], catalogue);

	it("accepts what the catalogue allows", () => {
		expect(check("elastic_agent_enabled: true\nsalt: !secret\nfree: [1]\n")).toEqual([]);
	});

	it("names the line of a wrong type, a reserved name and a scope", () => {
		const out = check("a: 1\nelastic_agent_enabled: maybe\ncanopy_intent: x\nansible_host: h\n");
		expect(out.map((p) => p.line)).toEqual([2, 3, 4]);
		expect(out[0].message).toContain("true or false");
		expect(out[1].message).toContain("a run for itself");
		expect(out[2].message).toContain("cannot be set at environment");
		expect(check("ansible_host: h\n", "machine")).toEqual([]);
	});

	it("refuses a name that is not usable and a secret with nothing to keep", () => {
		expect(check("'bad name': 1\n")[0].message).toContain("not a usable variable name");
		expect(check("other: !secret\n")[0].message).toContain("no secret value to keep");
	});
});
