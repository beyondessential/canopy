import { describe, expect, it } from "vitest";
import {
	type CatalogueVariable,
	type Schema,
	describeType,
	kindOf,
	offered,
	problems,
} from "./schema";

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
		for (const type of ["string", "integer", "number", "boolean", "array", "object"]) {
			expect(kindOf({ type })).toBe(type);
		}
		expect(kindOf({})).toBe("json");
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
	it("hides a reserved variable and one limited to other scopes", () => {
		expect(offered(variable({ reserved: true }), "machine", [])).toBe(false);
		expect(offered(variable({ scopes: ["machine"] }), "environment", [])).toBe(
			false,
		);
		expect(offered(variable({ scopes: ["machine"] }), "machine", [])).toBe(true);
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

	it("names the type, the bounds and the pattern", () => {
		expect(problems({ type: "integer" }, 1.5)).toEqual(["a whole number"]);
		expect(problems({ type: "integer", maximum: 10 }, 11)).toEqual(["at most 10"]);
		expect(problems({ type: "string", pattern: "^a+$" }, "b")).toEqual([
			"does not match ^a+$",
		]);
		expect(problems({ type: "string", enum: ["a", "b"] }, "c")).toEqual([
			'one of "a", "b"',
		]);
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
