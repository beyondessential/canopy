import type { CatalogueView } from "../../types";

/// The part of JSON Schema the catalogue uses, which is what an entry form is
/// shaped by. Anything it does not understand is entered as JSON.
export type Schema = {
	type?: string | string[];
	description?: string;
	default?: unknown;
	enum?: unknown[];
	format?: string;
	pattern?: string;
	minimum?: number;
	maximum?: number;
	minLength?: number;
	maxLength?: number;
	minItems?: number;
	maxItems?: number;
	properties?: Record<string, Schema>;
	required?: string[];
	items?: Schema;
	anyOf?: Schema[];
	oneOf?: Schema[];
};

export type CatalogueVariable = CatalogueView["variables"][number];
export type ScopeKind = "group" | "environment" | "machine";

/// The kind of value a schema describes, for choosing a field. Alternatives
/// that agree on a type, such as several string formats, are that type.
export function kindOf(
	schema: Schema,
): "boolean" | "integer" | "number" | "string" | "object" | "array" | "json" {
	const alternatives = schema.anyOf ?? schema.oneOf;
	const types = new Set<string>();
	const note = (s: Schema) => {
		for (const t of Array.isArray(s.type) ? s.type : s.type ? [s.type] : []) {
			if (t !== "null") types.add(t);
		}
	};
	note(schema);
	for (const alternative of alternatives ?? []) note(alternative);
	if (types.size !== 1) return "json";
	const [only] = types;
	switch (only) {
		case "boolean":
		case "integer":
		case "number":
		case "string":
		case "object":
		case "array":
			return only;
		default:
			return "json";
	}
}

/// A short account of a variable's type for the name's completion list.
export function describeType(variable: CatalogueVariable): string {
	const schema = variable.schema as Schema;
	const kind = kindOf(schema);
	const base =
		kind === "array"
			? `list of ${describeItems(schema.items)}`
			: kind === "json"
				? "any"
				: kind === "boolean"
					? "bool"
					: kind;
	const scope = variable.scopes?.length === 1 ? ` · ${variable.scopes[0]}` : "";
	const fallback =
		variable.default === null || variable.default === undefined
			? ""
			: ` · default ${JSON.stringify(variable.default)}`;
	return `${base}${scope}${fallback}`;
}

function describeItems(items: Schema | undefined): string {
	if (!items) return "values";
	const kind = kindOf(items);
	return kind === "object" ? "objects" : kind === "string" ? "strings" : `${kind}s`;
}

/// Whether a run acts on a machine: set by a switch on the machine, never as
/// a variable.
// spec: INV#managed-machines
export const MANAGED = "managed_by_ansible";

/// Whether a value says its machine is managed, as the plugin reads it.
export function truthy(value: unknown): boolean {
	if (typeof value === "boolean") return value;
	if (typeof value === "string")
		return ["yes", "true", "on", "1", "y", "t"].includes(value.toLowerCase());
	return value === 1;
}

/// Whether a catalogue variable is offered where a value is set: at a scope the
/// catalogue allows, and for application types the scope holds any of. A scope
/// holding nothing yet, such as a machine nothing is on, is offered every type.
export function offered(
	variable: CatalogueVariable,
	scope: ScopeKind,
	applicationTypes: ReadonlyArray<string>,
): boolean {
	if (variable.name === MANAGED) return false;
	if (variable.reserved) return false;
	if (variable.scopes && !variable.scopes.includes(scope)) return false;
	if (
		variable.applications &&
		applicationTypes.length > 0 &&
		!applicationTypes.some((type) => variable.applications?.includes(type))
	) {
		return false;
	}
	return true;
}

/// What breaks a schema's own constraints in a value, in the words the form
/// shows as it is typed. The server holds the value to the whole schema when
/// it is set; this is only what can be said before asking.
export function problems(schema: Schema, value: unknown, path = ""): string[] {
	const where = path === "" ? "" : `${path}: `;
	if (value === undefined || value === "") return [];
	if (schema.enum && !schema.enum.some((option) => option === value)) {
		return [`${where}one of ${schema.enum.map((o) => JSON.stringify(o)).join(", ")}`];
	}
	switch (kindOf(schema)) {
		case "boolean":
			return typeof value === "boolean" ? [] : [`${where}true or false`];
		case "integer":
		case "number": {
			if (typeof value !== "number" || Number.isNaN(value)) {
				return [`${where}a number`];
			}
			if (kindOf(schema) === "integer" && !Number.isInteger(value)) {
				return [`${where}a whole number`];
			}
			if (schema.minimum !== undefined && value < schema.minimum) {
				return [`${where}at least ${schema.minimum}`];
			}
			if (schema.maximum !== undefined && value > schema.maximum) {
				return [`${where}at most ${schema.maximum}`];
			}
			return [];
		}
		case "string": {
			if (typeof value !== "string") return [`${where}text`];
			const out: string[] = [];
			if (schema.minLength !== undefined && value.length < schema.minLength) {
				out.push(`${where}${value.length} of ${schema.minLength} characters`);
			}
			if (schema.maxLength !== undefined && value.length > schema.maxLength) {
				out.push(`${where}at most ${schema.maxLength} characters`);
			}
			if (schema.pattern !== undefined) {
				try {
					if (!new RegExp(schema.pattern, "u").test(value)) {
						out.push(`${where}does not match ${schema.pattern}`);
					}
				} catch {
					// A pattern the browser cannot read is left to the server.
				}
			}
			return out;
		}
		case "object": {
			if (typeof value !== "object" || value === null || Array.isArray(value)) {
				return [`${where}an object`];
			}
			const record = value as Record<string, unknown>;
			const out: string[] = [];
			for (const name of schema.required ?? []) {
				if (record[name] === undefined || record[name] === "") {
					out.push(`${path === "" ? "" : `${path}.`}${name} is required`);
				}
			}
			for (const [name, property] of Object.entries(schema.properties ?? {})) {
				out.push(
					...problems(property, record[name], path === "" ? name : `${path}.${name}`),
				);
			}
			return out;
		}
		case "array": {
			if (!Array.isArray(value)) return [`${where}a list`];
			const out: string[] = [];
			if (schema.minItems !== undefined && value.length < schema.minItems) {
				out.push(`${where}at least ${schema.minItems} items`);
			}
			if (schema.maxItems !== undefined && value.length > schema.maxItems) {
				out.push(`${where}at most ${schema.maxItems} items`);
			}
			if (schema.items) {
				value.forEach((item, index) => {
					out.push(...problems(schema.items as Schema, item, `${path}[${index}]`));
				});
			}
			return out;
		}
		default:
			return [];
	}
}

/// Whether `schema` requires `name`, for marking its field.
export function requires(schema: Schema, name: string): boolean {
	return schema.required?.includes(name) ?? false;
}
