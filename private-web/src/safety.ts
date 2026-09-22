// The safety-mode ladder, as the interface sees it.
//
// Spec: `.workhorse/specs/private-server/safety-modes.md` (id `SAFE`).
//
// The server decides every graded request itself; nothing here is a security
// boundary. What it is for is presentation: knowing the mode a control needs
// lets the interface block it and say which mode it wants, rather than letting
// the operator find out by being refused.

/** A rung of the ladder: the mode a session is in, or one a control requires. */
export type SafetyMode = "read-only" | "write" | "danger";

/** Low to high. The index is the comparison. */
const LADDER: readonly SafetyMode[] = ["read-only", "write", "danger"];

/** Whether a session in `held` may use something requiring `required`. */
export function permits(held: SafetyMode, required: SafetyMode): boolean {
	return LADDER.indexOf(held) >= LADDER.indexOf(required);
}

/** How long a raise lasts, in milliseconds. The countdown opens here. */
export const RAISE_DURATION_MS = 10 * 60 * 1000;

/** The header each request carries its session identifier in. */
export const SESSION_HEADER = "x-canopy-session";

/** The mode's name as it is shown to an operator. */
export function modeLabel(mode: SafetyMode): string {
	switch (mode) {
		case "read-only":
			return "Read-only";
		case "write":
			return "Write";
		case "danger":
			return "Danger";
	}
}
