/** The ranges an OSV event list describes, one phrase per range. */
export function describeRange(
	events: Array<{
		introduced?: string | null;
		fixed?: string | null;
		last_affected?: string | null;
		limit?: string | null;
	}>,
): string[] {
	const out: string[] = [];
	let from: string | null = null;
	const open = (introduced: string | null) =>
		!introduced || introduced === "0" ? null : `>= ${introduced}`;
	const close = (bound: string) => {
		const lower = open(from);
		out.push(lower ? `${lower}, ${bound}` : bound);
		from = null;
	};
	let pending = false;
	for (const event of events) {
		if (event.introduced != null) {
			if (pending) out.push(open(from) ?? "all versions");
			from = event.introduced;
			pending = true;
		} else if (event.fixed != null) {
			close(`< ${event.fixed}`);
			pending = false;
		} else if (event.last_affected != null) {
			close(`<= ${event.last_affected}`);
			pending = false;
		} else if (event.limit != null) {
			close(`< ${event.limit}`);
			pending = false;
		}
	}
	if (pending) out.push(open(from) ?? "all versions");
	return out;
}
