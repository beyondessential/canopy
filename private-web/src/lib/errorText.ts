const ESCAPES: Record<string, string> = {
	n: "\n",
	r: "\r",
	t: "\t",
	'"': '"',
	"\\": "\\",
};

/// Runners report a database error that has been through JSON.stringify once or
/// twice, so the newlines arrive as \n or \\n and the quotes as \" or \\".
export function readableError(text: string) {
	let out = text;
	for (let pass = 0; pass < 3; pass += 1) {
		const next = out.replace(/\\(.)/g, (whole, char: string) => ESCAPES[char] ?? whole);
		if (next === out) break;
		out = next;
	}
	return out.trim();
}

/// A tooltip is a glance at the failure, with the dialog holding the rest.
export function errorPreview(text: string, lines = 6) {
	const all = readableError(text).split("\n");
	if (all.length <= lines) return all.join("\n");
	return `${all.slice(0, lines).join("\n")}\n…`;
}
