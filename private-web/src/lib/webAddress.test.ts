import { describe, expect, it } from "vitest";
import { isWebAddress } from "./webAddress";

describe("isWebAddress", () => {
	it("is a web address over http or https", () => {
		expect(isWebAddress("https://central.example/")).toBe(true);
		expect(isWebAddress("HTTP://central.example")).toBe(true);
	});

	it("is not one for any other scheme, or none", () => {
		for (const address of ["javascript://x%0Aalert(1)", "data:text/html,x", "central.example", "", null, undefined]) {
			expect(isWebAddress(address)).toBe(false);
		}
	});
});
