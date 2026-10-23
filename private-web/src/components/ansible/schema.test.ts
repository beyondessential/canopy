import { describe, expect, it } from "vitest";
import type { CatalogueView } from "../../types";
import {
	type CatalogueVariable,
	type Schema,
	type ScopeKind,
	describeType,
	kindOf,
	offered,
	problems,
} from "./schema";
import { checkScope, parseScope } from "./yamlScope";

const variable = (over: Partial<CatalogueVariable>): CatalogueVariable => ({
	name: "v",
	description: null,
	schema: { type: "string" },
	default: null,
	secret: false,
	scopes: null,
	applications: null,
	reserved: false,
	...over,
});

type Kind = ReturnType<typeof kindOf>;

type KindRow = {
	schema: Schema;
	accepted: unknown;
	rejected: unknown;
	message: string | null;
	label: string;
};

const kinds = {
	boolean: {
		schema: { type: "boolean" },
		accepted: true,
		rejected: "yes",
		message: "true or false",
		label: "bool",
	},
	integer: {
		schema: { type: "integer" },
		accepted: 3,
		rejected: 1.5,
		message: "a whole number",
		label: "integer",
	},
	number: {
		schema: { type: "number" },
		accepted: 1.5,
		rejected: "x",
		message: "a number",
		label: "number",
	},
	string: {
		schema: { type: "string" },
		accepted: "a",
		rejected: 1,
		message: "text",
		label: "string",
	},
	object: {
		schema: { type: "object" },
		accepted: { a: 1 },
		rejected: [],
		message: "an object",
		label: "object",
	},
	array: {
		schema: { type: "array" },
		accepted: [1],
		rejected: "x",
		message: "a list",
		label: "list of values",
	},
	json: {
		schema: {},
		accepted: { anything: [1, "x"] },
		rejected: { anything: [1, "x"] },
		message: null,
		label: "any",
	},
} satisfies Record<Kind, KindRow>;

describe("each kind of schema", () => {
	it.each(Object.entries(kinds))("%s is read, held to and described as itself", (kind, row) => {
		expect(kindOf(row.schema)).toBe(kind);
		expect(problems(row.schema, row.accepted)).toEqual([]);
		expect(problems(row.schema, row.rejected)).toEqual(row.message === null ? [] : [row.message]);
		expect(describeType(variable({ schema: row.schema }))).toBe(row.label);
	});
});

describe("kindOf", () => {
	it("reads alternatives agreeing on a type as that type", () => {
		expect(
			kindOf({
				type: "string",
				anyOf: [{ format: "hostname" }, { format: "ipv4" }],
			}),
		).toBe("string");
		expect(kindOf({ anyOf: [{ type: "string" }, { type: "null" }] })).toBe(
			"string",
		);
	});

	it("falls back to JSON where it cannot tell", () => {
		expect(kindOf({ type: ["string", "integer"] })).toBe("json");
		expect(kindOf({ anyOf: [{ type: "string" }, { type: "integer" }] })).toBe(
			"json",
		);
	});
});

describe("describeType", () => {
	it("names the type, a lone scope and the default", () => {
		expect(
			describeType(
				variable({
					schema: { type: "boolean" },
					default: true,
				}),
			),
		).toBe("bool · default true");
		expect(
			describeType(
				variable({
					schema: { type: "object" },
					scopes: ["machine"],
				}),
			),
		).toBe("object · machine");
		expect(
			describeType(
				variable({ schema: { type: "array", items: { type: "string" } } }),
			),
		).toBe("list of strings");
	});
});

describe("offered", () => {
	it("hides a reserved variable", () => {
		expect(offered(variable({ reserved: true }), "machine", [])).toBe(false);
	});

	it("hides one for application types the scope holds none of", () => {
		const v = variable({ applications: ["tamanu-central"] });
		expect(offered(v, "machine", ["tamanu-facility"])).toBe(false);
		expect(offered(v, "machine", ["tamanu-central", "mysql"])).toBe(true);
	});

	it("offers every type to a scope with nothing on it", () => {
		expect(
			offered(variable({ applications: ["tamanu-central"] }), "machine", []),
		).toBe(true);
	});
});

type ApiScope = NonNullable<CatalogueVariable["scopes"]>[number];

const scopes: Record<ScopeKind, true> = {
	group: true,
	environment: true,
	machine: true,
};
const apiScopes: Record<ApiScope, true> = scopes;
void apiScopes;

const scopeKinds = Object.keys(scopes) as ScopeKind[];

describe("scopes", () => {
	const pairs = scopeKinds.flatMap((s) => scopeKinds.map((t) => [s, t] as const));

	it.each(pairs)("a variable limited to %s is offered at %s only if they are the same", (s, t) => {
		expect(offered(variable({ scopes: [s] }), t, [])).toBe(s === t);
	});

	const catalogue = {
		variables: [variable({ name: "ansible_host", scopes: ["machine"] })],
		playbooks: [],
		tags: {},
		source_commit: null,
		read_at: "2026-01-01T00:00:00Z",
		problem: null,
		problem_detail: null,
	} as unknown as CatalogueView;

	it.each(scopeKinds)("a machine-only variable set at %s is refused unless it is a machine", (s) => {
		const out = checkScope(parseScope("ansible_host: h\n"), [], s, [], catalogue);
		expect(out.map((p) => p.message)).toEqual(
			s === "machine" ? [] : [`ansible_host cannot be set at ${s}`],
		);
	});
});

describe("problems", () => {
	const iam: Schema = {
		type: "object",
		required: ["access_key_id", "secret_access_key"],
		properties: {
			access_key_id: { type: "string" },
			secret_access_key: { type: "string", minLength: 40 },
		},
	};

	it("accepts what matches and says nothing of what is not yet typed", () => {
		expect(problems({ type: "integer", minimum: 1 }, 2)).toEqual([]);
		expect(problems({ type: "string", minLength: 3 }, "")).toEqual([]);
	});

	type Constraint = Exclude<
		keyof Schema,
		"type" | "description" | "default" | "format" | "properties" | "items" | "anyOf" | "oneOf"
	>;

	// `format` is declared on Schema but never enforced here, so it has no row.
	const constraints = {
		enum: { schema: { type: "string", enum: ["a", "b"] }, bad: "c", message: 'one of "a", "b"' },
		pattern: {
			schema: { type: "string", pattern: "^a+$" },
			bad: "b",
			message: "does not match ^a+$",
		},
		minimum: { schema: { type: "integer", minimum: 2 }, bad: 1, message: "at least 2" },
		maximum: { schema: { type: "integer", maximum: 10 }, bad: 11, message: "at most 10" },
		minLength: {
			schema: { type: "string", minLength: 3 },
			bad: "ab",
			message: "2 of 3 characters",
		},
		maxLength: {
			schema: { type: "string", maxLength: 2 },
			bad: "abc",
			message: "at most 2 characters",
		},
		minItems: { schema: { type: "array", minItems: 2 }, bad: [1], message: "at least 2 items" },
		maxItems: {
			schema: { type: "array", maxItems: 2 },
			bad: [1, 2, 3],
			message: "at most 2 items",
		},
		required: {
			schema: { type: "object", required: ["a"] },
			bad: {},
			message: "a is required",
		},
	} satisfies Record<Constraint, { schema: Schema; bad: unknown; message: string }>;

	it.each(Object.entries(constraints))("names a broken %s", (_keyword, row) => {
		expect(problems(row.schema, row.bad)).toEqual([row.message]);
	});

	it("names what an object is missing and where a property is wrong", () => {
		expect(problems(iam, { access_key_id: "AKIA" })).toEqual([
			"secret_access_key is required",
		]);
		expect(
			problems(iam, { access_key_id: "AKIA", secret_access_key: "short" }),
		).toEqual(["secret_access_key: 5 of 40 characters"]);
	});

	it("holds each item of a list to the item schema", () => {
		expect(
			problems(
				{ type: "array", items: { type: "integer" }, maxItems: 2 },
				[1, "x", 3],
			),
		).toEqual(["at most 2 items", "[1]: a number"]);
	});
});
