import {
	LineCounter,
	Scalar,
	isAlias,
	isMap,
	isScalar,
	isSeq,
	parseDocument,
	stringify,
} from "yaml";
import { resolvePlain } from "./pyyaml";
import type { CatalogueView } from "../../types";
import { MANAGED, type Schema, type ScopeKind, offered, problems } from "./schema";

/** The longest variable name the server accepts. */
const NAME_LIMIT = 253;

/// A variable as a scope holds it.
export type ScopeVariable = {
	name: string;
	/** `undefined` for a secret, whose value is never read here. */
	value?: unknown;
	secret: boolean;
};

export type LineProblem = { line: number; message: string };

export type Parsed = {
	entries: Array<{
		name: string;
		line: number;
		value: unknown;
		/** Tagged `!secret`. */
		secret: boolean;
		/** A secret with no value after its tag, which keeps the one it has. */
		kept: boolean;
	}>;
	problems: LineProblem[];
};

export type Changes = {
	set: Array<{ name: string; value: unknown; secret: boolean }>;
	remove: string[];
};

const SECRET_TAG = "!secret";

/// A node's value, with every plain scalar read as PyYAML reads it, which is
/// how an inventory file is read, and not as the parser's own 1.1 mode does.
function resolved(node: unknown, document: ReturnType<typeof parseDocument>): unknown {
	if (isAlias(node)) return resolved(node.resolve(document), document);
	if (isScalar(node)) {
		return node.type === Scalar.PLAIN && !node.tag
			? resolvePlain(String(node.source ?? node.value ?? ""))
			: node.value;
	}
	if (isSeq(node)) return node.items.map((item) => resolved(item, document));
	if (isMap(node)) {
		return Object.fromEntries(
			node.items.map((pair) => [
				isScalar(pair.key) ? String(pair.key.value) : String(pair.key),
				resolved(pair.value, document),
			]),
		);
	}
	return null;
}

/// A scope as text: one `name: value` per variable, YAML values, a secret as
/// `name: !secret` so its value is never shown.
export function formatScope(variables: ReadonlyArray<ScopeVariable>): string {
	return [...variables]
		.sort((a, b) => a.name.localeCompare(b.name))
		.map((variable) =>
			variable.secret
				? `${variable.name}: ${SECRET_TAG}`
				: stringify({ [variable.name]: variable.value }, { version: "1.1" }).trimEnd(),
		)
		.join("\n");
}

/// Read a scope's text. A value is read as it would be in an inventory file, so
/// YAML 1.1: `yes` and `no` are booleans.
export function parseScope(text: string): Parsed {
	const lines = new LineCounter();
	const document = parseDocument(text, {
		version: "1.1",
		lineCounter: lines,
		uniqueKeys: true,
		customTags: [
			{
				tag: SECRET_TAG,
				resolve: (value: string) => new Scalar(value === "" ? null : value),
				identify: () => false,
			},
		],
	});
	const found: Parsed = { entries: [], problems: [] };
	for (const error of document.errors) {
		found.problems.push({
			line: error.linePos?.[0].line ?? 1,
			message: error.message.split("\n")[0],
		});
	}
	if (found.problems.length > 0) return found;

	const root = document.contents;
	if (root === null) return found;
	if (!isMap(root)) {
		found.problems.push({
			line: 1,
			message: "One variable per line, as name: value",
		});
		return found;
	}
	for (const pair of root.items) {
		// The key as written: a bare `.` would otherwise be read as a number.
		const key = isScalar(pair.key)
			? (pair.key.source ?? String(pair.key.value))
			: String(pair.key);
		const line = isScalar(pair.key) && pair.key.range
			? lines.linePos(pair.key.range[0]).line
			: 1;
		const value = pair.value;
		const secret = isScalar(value) && value.tag === SECRET_TAG;
		const plain = secret && isScalar(value) ? value.value : resolved(value, document);
		found.entries.push({
			name: key,
			line,
			value: secret ? (plain ?? undefined) : plain,
			secret,
			kept: secret && (plain === null || plain === undefined),
		});
	}
	return found;
}

/// What saving would change: the variables set or replaced, and those removed.
/// A secret left as `!secret` keeps its value, and a variable the catalogue
/// calls secret is set as one however it was typed.
export function diffScope(
	before: ReadonlyArray<ScopeVariable>,
	parsed: Parsed,
	catalogue: CatalogueView | null,
): Changes {
	const held = new Map(before.map((variable) => [variable.name, variable]));
	const changes: Changes = { set: [], remove: [] };
	const seen = new Set<string>();
	for (const entry of parsed.entries) {
		seen.add(entry.name);
		const described = catalogue?.variables.find((v) => v.name === entry.name);
		const secret = entry.secret || (described?.secret ?? false);
		const previous = held.get(entry.name);
		if (entry.kept && previous?.secret) continue;
		if (
			previous &&
			!previous.secret &&
			!secret &&
			JSON.stringify(previous.value) === JSON.stringify(entry.value)
		) {
			continue;
		}
		changes.set.push({ name: entry.name, value: entry.value, secret });
	}
	for (const variable of before) {
		if (!seen.has(variable.name)) changes.remove.push(variable.name);
	}
	return changes;
}

/// What stops the text being saved, line by line: what does not parse, a name
/// that is not usable, a secret with no value to keep, and what the catalogue
/// refuses at this scope.
export function checkScope(
	parsed: Parsed,
	before: ReadonlyArray<ScopeVariable>,
	scope: ScopeKind,
	applicationTypes: ReadonlyArray<string>,
	catalogue: CatalogueView | null,
): LineProblem[] {
	const out = [...parsed.problems];
	const held = new Set(before.filter((v) => v.secret).map((v) => v.name));
	for (const entry of parsed.entries) {
		if (
			!/^[-._a-zA-Z0-9]+$/.test(entry.name) ||
			entry.name === "." ||
			entry.name === ".." ||
			entry.name.length > NAME_LIMIT
		) {
			out.push({
				line: entry.line,
				message: `${entry.name} is not a usable variable name`,
			});
			continue;
		}
		if (entry.name === MANAGED) {
			out.push({
				line: entry.line,
				message: `${entry.name} is set by the machine's switch`,
			});
			continue;
		}
		if (entry.kept && !held.has(entry.name)) {
			out.push({
				line: entry.line,
				message: `${entry.name} has no secret value to keep`,
			});
		}
		const described = catalogue?.variables.find((v) => v.name === entry.name);
		if (!described) continue;
		if (entry.secret && !described.secret) {
			out.push({
				line: entry.line,
				message: `${entry.name} is not secret in the catalogue, so it is set as a plain value`,
			});
		}
		if (described.reserved) {
			out.push({
				line: entry.line,
				message: `${entry.name} is set by a run for itself`,
			});
		} else if (!offered({ ...described, applications: null }, scope, applicationTypes)) {
			out.push({
				line: entry.line,
				message: `${entry.name} cannot be set at ${scope}`,
			});
		}
		if (!entry.kept && entry.value !== undefined) {
			for (const message of problems(described.schema as Schema, entry.value)) {
				out.push({ line: entry.line, message: `${entry.name}: ${message}` });
			}
		}
	}
	return out;
}
