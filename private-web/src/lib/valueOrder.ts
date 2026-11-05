/// Ordering for a set of values an operator asked to see sorted by value
/// rather than by how many servers report each one.
///
/// Everything the fleet view groups on is compared as its rendered text, so
/// the shape of the set is all there is to go on: whole numbers sort as
/// numbers, dotted values as versions (component by component, so `2.10`
/// sits above `2.9`), and anything else as text.

export type Comparator = (a: string, b: string) => number;

const INTEGER = /^-?\d+$/;
const UNSIGNED_INTEGER = /^\d+$/;
const DOTTED = /^\d+(?:\.\d+)+(?:[-+][0-9A-Za-z.-]+)?$/;

/// Pick the comparator that suits a field's values.
///
/// A set mixing dotted values with whole numbers reads as versions: `9.6`
/// against `16` is a PostgreSQL major, not a decimal. That costs decimal
/// data the distinction between `1.5` and `1.25`, which read as versions and
/// so order the other way round; the leading component still dominates, and
/// a fleet's dotted values are overwhelmingly versions.
export function valueComparator(values: readonly string[]): Comparator {
	if (values.length === 0) return textCompare;
	if (values.every((v) => INTEGER.test(v))) return numericCompare;
	if (
		values.some((v) => DOTTED.test(v)) &&
		values.every((v) => DOTTED.test(v) || UNSIGNED_INTEGER.test(v))
	) {
		return versionCompare;
	}
	if (values.every((v) => v.trim() !== "" && Number.isFinite(Number(v)))) {
		return numericCompare;
	}
	return textCompare;
}

function numericCompare(a: string, b: string): number {
	return Number(a) - Number(b) || a.localeCompare(b);
}

/// Compare dotted versions component by component, a missing component
/// counting as zero, so `2.10` beats `2.9` and `16` beats `9.6`. A trailing
/// `-rc1`/`+build` breaks a tie between equal cores, the bare version
/// sorting above its own prereleases as semver has it.
export function versionCompare(a: string, b: string): number {
	const [aCore, aTail] = splitTail(a);
	const [bCore, bTail] = splitTail(b);
	const aParts = aCore.split(".");
	const bParts = bCore.split(".");
	for (let i = 0; i < Math.max(aParts.length, bParts.length); i++) {
		const diff = Number(aParts[i] ?? 0) - Number(bParts[i] ?? 0);
		if (diff !== 0) return diff;
	}
	if (aTail === bTail) return 0;
	if (aTail === "") return 1;
	if (bTail === "") return -1;
	return aTail.localeCompare(bTail);
}

function splitTail(version: string): [string, string] {
	const at = version.search(/[-+]/);
	return at === -1
		? [version, ""]
		: [version.slice(0, at), version.slice(at + 1)];
}

function textCompare(a: string, b: string): number {
	return a.localeCompare(b);
}
