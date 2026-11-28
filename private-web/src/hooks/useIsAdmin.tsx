import {
	type ReactNode,
	createContext,
	useContext,
	useEffect,
	useMemo,
	useState,
} from "react";
import { useApi } from "../api";
import type { AccessLevel } from "../types";
import { useReloadInterval } from "./useReloadInterval";

/** How often to re-probe once we have an answer. */
const REPROBE_MS = 5 * 60_000;
/** How fast to retry while we have no answer at all. */
const RETRY_MS = 3_000;

export interface AdminStatus {
	/** The caller's access level once the probe has answered: `null` for a
	 * caller bearing no tailnet identity, `undefined` while the level is still
	 * unknown (no probe has ever succeeded). */
	level: AccessLevel | null | undefined;
	/** `true` if confirmed admin, `false` if confirmed below admin, `undefined`
	 * while the level is still unknown. */
	isAdmin: boolean | undefined;
	/** The probe has answered at least once this session. While this is false,
	 * `level` and `isAdmin` are `undefined` and controls beyond reading are
	 * hidden. */
	resolved: boolean;
	/** The most recent probe failed. Only interesting when `resolved` is false:
	 * once we have an answer we keep it and ride out transient failures. */
	error: Error | null;
	/** Re-probe now. Wired to the retry button in the unresolved banner. */
	reload: () => void;
}

// `null` (not `undefined`) so the hooks can tell "no provider mounted" apart
// from "provider mounted, status not yet known" and shout about the former
// instead of silently reporting non-admin.
const AdminContext = createContext<AdminStatus | null>(null);

/** Single source of truth for the caller's access level. Mounted once near the
 * router root; every consumer reads the same value through
 * {@link useAccessLevel} or {@link useIsAdmin} so the whole page agrees. Never
 * probe `commons.current_user_level`
 * directly from a component — per-component probes have their own lifecycle
 * and their own failure modes, which is how one half of a page ends up in
 * admin mode while the other half isn't. */
export function AdminProvider({ children }: { children: ReactNode }) {
	// Re-probe periodically and when the tab regains focus. The provider
	// mounts once and lives for the whole SPA session, so without this a
	// single failed probe at initial load would hide every admin control
	// until a manual hard reload.
	const tick = useReloadInterval(REPROBE_MS);
	const probe = useApi("commons", "current_user_level", {}, [tick]);
	const { reload } = probe;
	// `null` is an answer (no tailnet identity), so "no answer" is `undefined`.
	const answer = probe.status === "ok" ? probe.data : undefined;
	const error = probe.status === "error" ? probe.error : null;

	// Last confirmed answer. Kept sticky across background-refetch errors so a
	// transient blip (e.g. the tailnet control plane momentarily unreachable)
	// never yanks admin controls out from under an admin mid-session; only the
	// first-ever load shows `undefined`.
	const [known, setKnown] = useState<AccessLevel | null | undefined>(
		undefined,
	);
	useEffect(() => {
		if (answer !== undefined) setKnown(answer);
	}, [answer]);

	// Until we've learned the status at least once, retry a failed probe
	// quickly so the session self-heals without a hard reload. The deps are all
	// stable values, not the per-render `probe` object — keying off that would
	// let any re-render of the tree above us clear the pending timer before it
	// ever fires. Reloading flips the status to `loading` first, so this
	// re-arms itself on each successive failure.
	useEffect(() => {
		if (!error || known !== undefined) return;
		const id = window.setTimeout(reload, RETRY_MS);
		return () => window.clearTimeout(id);
	}, [error, known, reload]);

	const value = useMemo<AdminStatus>(
		() => ({
			level: known,
			isAdmin: known === undefined ? undefined : known === "admin",
			resolved: known !== undefined,
			error,
			reload,
		}),
		[known, error, reload],
	);

	return (
		<AdminContext.Provider value={value}>{children}</AdminContext.Provider>
	);
}

/** Full admin status, including whether the probe has resolved and why not.
 * Most callers want {@link useIsAdmin}; this is for UI that needs to explain
 * the unknown state rather than just hide behind it. */
export function useAdminStatus(): AdminStatus {
	const ctx = useContext(AdminContext);
	if (!ctx) {
		throw new Error("useAdminStatus must be used inside <AdminProvider>");
	}
	return ctx;
}

/** The caller's access level once the probe has resolved, `null` for a caller
 * with no tailnet identity, or `undefined` while the level is unknown. */
export function useAccessLevel(): AccessLevel | null | undefined {
	return useAdminStatus().level;
}

/** Returns `true`/`false` once the probe has resolved, or `undefined` while the
 * status is unknown. Callers should `=== true` to gate admin-only UI: better to
 * briefly hide buttons from an admin than to briefly offer them to a non-admin.
 * When the status is unknown because the probe is failing, `AdminProbeBanner`
 * tells the user why the controls aren't there. */
export function useIsAdmin(): boolean | undefined {
	return useAdminStatus().isAdmin;
}
