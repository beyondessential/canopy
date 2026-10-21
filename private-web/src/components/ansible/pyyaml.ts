/// What a plain (unquoted, untagged) YAML scalar is read as by PyYAML, which is
/// how Ansible reads an inventory file. This is the same rule as the server's
/// inventory reader (`fns/ansible/inventory_file.rs`), so a value typed in the
/// text editor is stored as the value an import of the same text would store.
// spec: ANSIMP#reading-the-file

const INT_DECIMAL = /^[-+]?(0|[1-9][0-9_]*)$/;
const INT_BINARY = /^[-+]?0b[01_]+$/;
const INT_OCTAL = /^[-+]?0[0-7_]+$/;
const INT_HEX = /^[-+]?0x[0-9a-fA-F_]+$/;
const INT_SEXAGESIMAL = /^[-+]?[1-9][0-9_]*(:[0-5]?[0-9])+$/;
const FLOAT =
	/^[-+]?([0-9][0-9_]*)\.[0-9_]*([eE][-+][0-9]+)?$|^[-+]?\.[0-9_]+([eE][-+][0-9]+)?$/;
const FLOAT_SEXAGESIMAL = /^[-+]?[0-9][0-9_]*(:[0-5]?[0-9])+\.[0-9_]*$/;
const FLOAT_SPECIAL = /^([-+]?\.(inf|Inf|INF)|\.(nan|NaN|NAN))$/;

const TRUE = new Set(["yes", "Yes", "YES", "true", "True", "TRUE", "on", "On", "ON"]);
const FALSE = new Set(["no", "No", "NO", "false", "False", "FALSE", "off", "Off", "OFF"]);
const NULL = new Set(["", "~", "null", "Null", "NULL"]);

const digits = (text: string) => text.replace(/^[-+]/, "").replaceAll("_", "");

function integer(value: bigint, text: string): number | string {
	// Beyond what JSON carries exactly, the text it was.
	return value >= BigInt(Number.MIN_SAFE_INTEGER) && value <= BigInt(Number.MAX_SAFE_INTEGER)
		? Number(value)
		: text;
}

export function resolvePlain(text: string): unknown {
	if (NULL.has(text)) return null;
	if (TRUE.has(text)) return true;
	if (FALSE.has(text)) return false;
	if (text === "=") return text;

	const negative = text.startsWith("-");
	const signed = (n: bigint) => (negative ? -n : n);

	if (INT_DECIMAL.test(text)) return integer(signed(BigInt(digits(text))), text);
	if (INT_BINARY.test(text)) {
		return integer(signed(BigInt(`0b${digits(text).slice(2)}`)), text);
	}
	if (INT_HEX.test(text)) {
		return integer(signed(BigInt(`0x${digits(text).slice(2)}`)), text);
	}
	if (INT_OCTAL.test(text)) {
		return integer(signed(BigInt(`0o${digits(text).slice(1)}`)), text);
	}
	if (INT_SEXAGESIMAL.test(text)) {
		const total = digits(text)
			.split(":")
			.reduce((sum, part) => sum * 60n + BigInt(part), 0n);
		return integer(signed(total), text);
	}
	if (FLOAT_SPECIAL.test(text)) return text;
	if (FLOAT.test(text)) {
		const value = Number(digits(text));
		return Number.isFinite(value) ? (negative ? -value : value) : text;
	}
	if (FLOAT_SEXAGESIMAL.test(text)) {
		const clean = digits(text);
		const dot = clean.indexOf(".");
		const whole = clean
			.slice(0, dot)
			.split(":")
			.reduce((sum, part) => sum * 60 + Number(part), 0);
		const value = Number(`${whole}${clean.slice(dot)}`);
		return Number.isFinite(value) ? (negative ? -value : value) : text;
	}
	return text;
}
