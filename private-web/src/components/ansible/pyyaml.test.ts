import { describe, expect, it } from "vitest";
import scalars from "./fixtures/pyyaml-scalars.json";
import { resolvePlain } from "./pyyaml";

// The fixture is shared with the server's `plain_scalars_are_read_as_pyyaml_reads_them`.
describe("resolvePlain", () => {
	it.each(scalars)("reads $text as $value", ({ text, value }) => {
		expect(resolvePlain(text)).toStrictEqual(value);
	});
});
