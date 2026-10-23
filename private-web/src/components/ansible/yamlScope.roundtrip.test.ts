import { describe, expect, it } from "vitest";
import scalars from "./fixtures/pyyaml-scalars.json";
import { diffScope, formatScope, parseScope, type ScopeVariable } from "./yamlScope";

function reread(value: unknown): unknown {
	const parsed = parseScope(formatScope([{ name: "v", value, secret: false }]));
	expect(parsed.problems).toEqual([]);
	return parsed.entries[0].value;
}

describe("a scope opened and saved unedited", () => {
	describe("keeps a string whatever it looks like", () => {
		it.each(scalars)("$text", ({ text }) => {
			expect(reread(text)).toStrictEqual(text);
		});
	});

	describe("keeps the value a plain scalar is read as", () => {
		it.each(scalars)("$text", ({ value }) => {
			expect(reread(value)).toStrictEqual(value);
		});
	});

	it("changes nothing", () => {
		const variables: ScopeVariable[] = [];
		scalars.forEach(({ text, value }, i) => {
			variables.push({ name: `text_${i}`, value: text, secret: false });
			variables.push({ name: `value_${i}`, value, secret: false });
		});
		variables.push(
			{ name: "list", value: scalars.map((row) => row.text), secret: false },
			{ name: "map", value: Object.fromEntries(scalars.map((row, i) => [`k${i}`, row.value])), secret: false },
			{ name: "token", secret: true },
		);

		const changes = diffScope(variables, parseScope(formatScope(variables)), null);
		expect(changes.set).toEqual([]);
		expect(changes.remove).toEqual([]);
	});
});
