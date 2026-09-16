import { describe, expect, it } from "vitest";
import { errorPreview, readableError } from "./errorText";

describe("readableError", () => {
	it("turns a doubly escaped newline into a newline", () => {
		expect(readableError("reason,\\\\n        NEW.created_at")).toBe(
			"reason,\n        NEW.created_at",
		);
	});

	it("turns a singly escaped newline into a newline", () => {
		expect(readableError("first\\nsecond")).toBe("first\nsecond");
	});

	it("unwraps escaped quotes", () => {
		expect(readableError('SELECT 1 FROM \\\\"public\\\\".\\\\"users\\\\"')).toBe(
			'SELECT 1 FROM "public"."users"',
		);
	});

	it("leaves text that carries no escapes alone", () => {
		expect(readableError("permission denied for table users")).toBe(
			"permission denied for table users",
		);
	});
});

describe("errorPreview", () => {
	it("keeps a short error whole", () => {
		expect(errorPreview("one\\ntwo")).toBe("one\ntwo");
	});

	it("marks where a long error was cut", () => {
		expect(errorPreview("a\\nb\\nc\\nd", 2)).toBe("a\nb\n…");
	});
});
