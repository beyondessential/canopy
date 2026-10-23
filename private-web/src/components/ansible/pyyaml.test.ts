import { describe, expect, it } from "vitest";
import { resolvePlain } from "./pyyaml";

// The same cases as the server's `a_value_is_what_ansible_reads_it_as`.
describe("resolvePlain", () => {
	it("reads booleans and nulls as PyYAML does", () => {
		for (const yes of ["yes", "Yes", "true", "on", "ON"]) expect(resolvePlain(yes)).toBe(true);
		for (const no of ["no", "NO", "false", "off"]) expect(resolvePlain(no)).toBe(false);
		for (const nothing of ["", "~", "null", "NULL"]) expect(resolvePlain(nothing)).toBeNull();
		expect(resolvePlain("y")).toBe("y");
	});

	it("reads integers in every base PyYAML does", () => {
		expect(resolvePlain("0644")).toBe(420);
		expect(resolvePlain("1_000")).toBe(1000);
		expect(resolvePlain("0x1F")).toBe(31);
		expect(resolvePlain("0b101")).toBe(5);
		expect(resolvePlain("-12")).toBe(-12);
		expect(resolvePlain("1:30")).toBe(90);
		expect(resolvePlain("02:30")).toBe("02:30");
		expect(resolvePlain("12345678901234567890123")).toBe("12345678901234567890123");
		expect(resolvePlain("9007199254740991")).toBe(9007199254740991);
		expect(resolvePlain("9007199254740993")).toBe("9007199254740993");
	});

	it("reads floats only where PyYAML does", () => {
		expect(resolvePlain("2.5")).toBe(2.5);
		expect(resolvePlain("-.5")).toBe(-0.5);
		expect(resolvePlain("1:30.5")).toBe(90.5);
		expect(resolvePlain("1e3")).toBe("1e3");
		expect(resolvePlain(".inf")).toBe(".inf");
	});

	it("leaves anything else as text", () => {
		expect(resolvePlain("Pacific/Auckland")).toBe("Pacific/Auckland");
		expect(resolvePlain("v2.64.3")).toBe("v2.64.3");
		expect(resolvePlain("2.64.3")).toBe("2.64.3");
		expect(resolvePlain("1:30:99")).toBe("1:30:99");
		expect(resolvePlain("1:60")).toBe("1:60");
	});
});
