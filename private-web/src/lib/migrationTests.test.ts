import { describe, expect, it } from "vitest";
import { formatDuration } from "./migrationTests";

describe("formatDuration", () => {
	it("shows a truncated zero as under a second", () => {
		expect(formatDuration(0)).toBe("<1s");
	});

	it("keeps whole seconds, minutes and hours", () => {
		expect(formatDuration(1)).toBe("1s");
		expect(formatDuration(59)).toBe("59s");
		expect(formatDuration(90)).toBe("2m");
		expect(formatDuration(5400)).toBe("1.5h");
	});
});
