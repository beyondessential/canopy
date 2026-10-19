import { describe, expect, it } from "vitest";
import { detailPairs, detailValue, jobLabel, recordPath } from "./audit";

describe("detailValue", () => {
	it("omits null and empty values", () => {
		for (const empty of [null, undefined, "", [], {}]) {
			expect(detailValue(empty)).toBeNull();
		}
	});

	it("keeps false and zero", () => {
		expect(detailValue(false)).toEqual({ text: "false", mono: false });
		expect(detailValue(0)).toEqual({ text: "0", mono: false });
	});

	it("renders an ISO timestamp in the viewer's locale", () => {
		const iso = "2026-10-19T04:08:11Z";
		const expected = new Intl.DateTimeFormat(undefined, {
			dateStyle: "medium",
			timeStyle: "short",
		}).format(new Date(iso));
		expect(detailValue(iso)).toEqual({ text: expected, mono: false });
	});

	it("shows nested values as compact JSON", () => {
		expect(detailValue({ a: 1, b: [2] })).toEqual({
			text: '{"a":1,"b":[2]}',
			mono: true,
		});
	});
});

describe("detailPairs", () => {
	it("turns underscores in keys into spaces", () => {
		expect(detailPairs({ access_key: "ASIA", gone: null })).toEqual([
			{ key: "access key", value: { text: "ASIA", mono: false } },
		]);
	});
});

describe("recordPath", () => {
	it("links by identifier, never by name", () => {
		expect(
			recordPath({ kind: "machine", id: "abc", name: "host 1/2" }),
		).toBe("/fleet/machines/abc");
	});

	it("gives no link to a kind with no page", () => {
		expect(recordPath({ kind: "credential", id: "x", name: "y" })).toBeNull();
	});
});

describe("jobLabel", () => {
	it("capitalises and spaces a job name", () => {
		expect(jobLabel("dns_records")).toBe("Dns records");
	});
});
