import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import TimeLeft, { describeTimeLeft } from "./TimeLeft";

const NOW = Date.parse("2026-10-09T00:00:00Z");
const after = (seconds: number) => NOW + seconds * 1000;

describe("describeTimeLeft", () => {
	it("floors in whole days wherever it is shown", () => {
		expect(describeTimeLeft(after(89 * 86400 + 86399), NOW)).toBe(
			"expires in 89 days",
		);
		expect(describeTimeLeft(after(86400), NOW)).toBe("expires in 1 day");
	});

	it("reads in hours under a day and minutes under an hour", () => {
		expect(describeTimeLeft(after(23 * 3600 + 3599), NOW)).toBe(
			"expires in 23 hours",
		);
		expect(describeTimeLeft(after(3600), NOW)).toBe("expires in 1 hour");
		expect(describeTimeLeft(after(59 * 60 + 59), NOW)).toBe(
			"expires in 59 minutes",
		);
		expect(describeTimeLeft(after(30), NOW)).toBe("expires in under a minute");
	});

	it("says how long ago once it has expired", () => {
		expect(describeTimeLeft(after(-3 * 86400 - 5), NOW)).toBe(
			"expired 3 days ago",
		);
	});
});

describe("TimeLeft", () => {
	it("carries the certificate's state, so its colour follows the state chip", () => {
		const at = new Date(Date.now() + 10 * 86400_000).toISOString();
		render(<TimeLeft notAfter={at} risk="critical" />);
		expect(screen.getByText(/expires in/).getAttribute("data-risk")).toBe(
			"critical",
		);
	});
});
