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

// The session identifier every request carries.
//
// `callApi` is a bare function called from outside React, so the provider
// publishes the identifier here rather than threading it through every call
// site. It lives in memory only and is deliberately not persisted: a reloaded
// page has no identifier, asks for a fresh session, and comes back read-only.
let sessionId: string | null = null;

/** Publish the session the provider holds, so requests can carry it. */
export function publishSession(id: string | null): void {
	sessionId = id;
}

/** The header a request carries its session in, or nothing before there is one. */
export function sessionHeaders(): Record<string, string> {
	return sessionId ? { [SESSION_HEADER]: sessionId } : {};
}

// What to do when the server says a raise has lapsed. The provider registers
// this so client and server agree again without a reload.
let onRaiseLapsed: (() => void) | null = null;

/** Register the provider's handler for a mode refusal. */
export function setRaiseLapsedHandler(handler: (() => void) | null): void {
	onRaiseLapsed = handler;
}

/** The problem types the two refusals carry. */
const MODE_REFUSAL = "safety-mode-too-low";
const PERMISSION_REFUSAL = "danger-not-permitted";

/** Which refusal a problem-details body is, if either. */
export function refusalOf(detail: unknown): "mode" | "permission" | null {
	if (!detail || typeof detail !== "object") return null;
	const type = (detail as { type?: unknown }).type;
	if (typeof type !== "string") return null;
	if (type.endsWith(MODE_REFUSAL)) return "mode";
	if (type.endsWith(PERMISSION_REFUSAL)) return "permission";
	return null;
}

/**
 * Tell the provider a request was refused for its mode, so the indicator drops
 * back to read-only. Called from the API layer, which sees every refusal.
 */
export function noteRefusal(detail: unknown): void {
	if (refusalOf(detail) === "mode") onRaiseLapsed?.();
}
