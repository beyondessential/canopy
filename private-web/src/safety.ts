// The safety-mode ladder, as the interface sees it.
//
// Spec: `.workhorse/specs/private-server/safety-modes.md` (id `SAFE`).
//
// The server decides every graded request itself; nothing here is a security
// boundary. What it is for is presentation: knowing the mode a control needs
// lets the interface block it and say which mode it wants, rather than letting
// the operator find out by being refused.

import type { DangerReason } from "./safety-modes";

/** A rung of the ladder: the mode a session is in, or one a control requires. */
export type SafetyMode = "read-only" | "write" | "danger";

/** A mode above read-only: one that has a stripe, and one a raise can be to. */
export type RaisedMode = Exclude<SafetyMode, "read-only">;

/** Low to high. The index is the comparison, for every reading of the ladder. */
export const LADDER: readonly SafetyMode[] = ["read-only", "write", "danger"];

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

/**
 * What an operator is told about each reason a handler is danger, in the order
 * the SAFE spec lists them. Each completes "it …", so a blocked control's raise
 * reads "it acts directly on servers and invalidates credentials".
 */
const DANGER_REASON_WORDING: Record<DangerReason, string> = {
	irreversible: "cannot be undone",
	fleet: "acts directly on servers",
	unprotects: "removes a protection",
	issues: "issues credentials",
	invalidates: "invalidates credentials",
};

/** "a", "a and b", "a, b and c". */
export function joinWithAnd(items: readonly string[]): string {
	if (items.length <= 1) return items.join("");
	return `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

/**
 * Why a danger control needs danger, as the operator is told it: "it acts
 * directly on servers and invalidates credentials". Reasons are worded in the
 * order of {@link DANGER_REASON_WORDING}, whatever order they are given in.
 */
export function dangerReasonsSentence(reasons: readonly DangerReason[]): string {
	const ordered = (Object.keys(DANGER_REASON_WORDING) as DangerReason[]).filter(
		(reason) => reasons.includes(reason),
	);
	return `it ${joinWithAnd(ordered.map((reason) => DANGER_REASON_WORDING[reason]))}`;
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

/** The problem type a mode refusal carries. */
const MODE_REFUSAL = "safety-mode-too-low";

/** Whether a problem-details body is a mode refusal. */
function isModeRefusal(detail: unknown): boolean {
	if (!detail || typeof detail !== "object") return false;
	const type = (detail as { type?: unknown }).type;
	return typeof type === "string" && type.endsWith(MODE_REFUSAL);
}

/**
 * Tell the provider a request was refused for its mode, so the indicator drops
 * back to read-only. Called from the API layer, which sees every refusal.
 */
export function noteRefusal(detail: unknown): void {
	if (isModeRefusal(detail)) onRaiseLapsed?.();
}
