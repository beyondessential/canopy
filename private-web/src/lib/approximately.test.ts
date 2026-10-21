import { describe, expect, it } from "vitest";
import { approximately } from "./approximately";

describe("approximately", () => {
	it("keeps small counts exact", () => {
		expect(approximately(0)).toBe("0");
		expect(approximately(7)).toBe("7");
		expect(approximately(43)).toBe("43");
	});

	it("rounds to two significant figures", () => {
		expect(approximately(437)).toBe("440");
		expect(approximately(1234)).toBe("1.2K");
		expect(approximately(56_789)).toBe("57K");
		expect(approximately(1_234_567)).toBe("1.2M");
	});
});
