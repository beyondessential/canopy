import { describe, expect, it } from "vitest";
import { describeRange } from "./osvRange";

describe("describeRange", () => {
	it("closes a range at its fix", () => {
		expect(describeRange([{ introduced: "1.0.0" }, { fixed: "1.2.0" }])).toEqual([
			">= 1.0.0, < 1.2.0",
		]);
	});

	it("closes a range at its limit", () => {
		expect(describeRange([{ introduced: "1.0.0" }, { limit: "2.0.0" }])).toEqual([
			">= 1.0.0, < 2.0.0",
		]);
	});

	it("leaves a range with no end open", () => {
		expect(describeRange([{ introduced: "0" }])).toEqual(["all versions"]);
	});
});
