import { describe, expect, it } from "vitest";
import {
	healthcheckPath,
	healthcheckSettingsPath,
	namespaceFromSegment,
	namespaceSegment,
	qualifiedCheckName,
	type NamespaceRef,
} from "../types";

const FLAT: NamespaceRef = { subject: null, application_type: null };
const MACHINE: NamespaceRef = { subject: "machine", application_type: null };
const CENTRAL: NamespaceRef = {
	subject: "application",
	application_type: "tamanu-central",
};

// Check names are arbitrary strings a device chose, not restricted to
// URL-safe characters. A name carrying `/`, `?`, `#`, or `%` routes to the
// wrong page — or nowhere — unless every link builder encodes it, which is
// what these two helpers exist to guarantee.
describe("healthcheck link builders", () => {
	const awkward: [string, string][] = [
		["a/b", "a%2Fb"],
		["what?", "what%3F"],
		["frag#ment", "frag%23ment"],
		["100%", "100%25"],
		["with space", "with%20space"],
	];

	it.each(awkward)("encodes %j in the affected-servers path", (raw, encoded) => {
		expect(healthcheckPath("alertd", FLAT, raw)).toBe(`/healthchecks/alertd/-/${encoded}`);
	});

	it.each(awkward)("encodes %j in the settings path", (raw, encoded) => {
		expect(healthcheckSettingsPath("alertd", FLAT, raw)).toBe(
			`/settings/healthchecks/alertd/-/${encoded}`,
		);
	});

	it("encodes the source too", () => {
		expect(healthcheckPath("a/b", FLAT, "disk")).toBe("/healthchecks/a%2Fb/-/disk");
		expect(healthcheckSettingsPath("a/b", FLAT, "disk")).toBe(
			"/settings/healthchecks/a%2Fb/-/disk",
		);
	});

	it("leaves an ordinary name alone", () => {
		expect(healthcheckPath("alertd", FLAT, "disk_free")).toBe(
			"/healthchecks/alertd/-/disk_free",
		);
	});

	it("addresses each namespace separately", () => {
		expect(healthcheckPath("alertd", MACHINE, "disk_free")).toBe(
			"/healthchecks/alertd/machine/disk_free",
		);
		expect(healthcheckPath("alertd", CENTRAL, "version")).toBe(
			"/healthchecks/alertd/application.tamanu-central/version",
		);
	});
});

describe("namespace segments", () => {
	it.each([
		[FLAT, "-"],
		[MACHINE, "machine"],
		[CENTRAL, "application.tamanu-central"],
	])("round-trips %j", (namespace, segment) => {
		expect(namespaceSegment(namespace)).toBe(segment);
		expect(namespaceFromSegment(segment)).toEqual(namespace);
	});

	// The application type set is open, so a group can report a type
	// called `machine`. The subject leads the segment precisely so that type
	// is not read as the box.
	it("tells an application type called machine from the box", () => {
		const type: NamespaceRef = { subject: "application", application_type: "machine" };
		expect(namespaceSegment(type)).toBe("application.machine");
		expect(namespaceFromSegment("application.machine")).toEqual(type);
		expect(namespaceFromSegment("machine")).toEqual(MACHINE);
	});

	it("refuses a segment that names no namespace", () => {
		expect(namespaceFromSegment("")).toBeNull();
		expect(namespaceFromSegment("application")).toBeNull();
		expect(namespaceFromSegment("application.")).toBeNull();
		expect(namespaceFromSegment("nonsense")).toBeNull();
	});
});

describe("qualified names", () => {
	it("qualifies an application type's check and leaves the rest bare", () => {
		expect(qualifiedCheckName(CENTRAL, "version")).toBe("tamanu-central:version");
		expect(qualifiedCheckName(MACHINE, "disk_free")).toBe("disk_free");
		expect(qualifiedCheckName(FLAT, "reachability")).toBe("reachability");
		expect(qualifiedCheckName(undefined, "reachability")).toBe("reachability");
	});
});
