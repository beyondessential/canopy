import { Button, Dialog, DialogActions, DialogContent, DialogTitle, Typography } from "@mui/material";
import {
	type ReactNode,
	createContext,
	useCallback,
	useContext,
	useEffect,
	useMemo,
	useRef,
	useState,
} from "react";
import { flushSync } from "react-dom";
import { callApi } from "../api";
import { RaiseDialog, type RaiseRequest } from "../components/RaiseDialog";
import {
	RAISE_DURATION_MS,
	type SafetyMode,
	permits,
	publishSession,
	setRaiseLapsedHandler,
} from "../safety";

/** What the server says about the caller's session. */
interface SessionState {
	id: string;
	mode: SafetyMode;
	raise_expires_at?: string | null;
}

/**
 * How a request for a raise came out: the mode now `permits` the action, whether
 * it already did or the raise has landed; the operator `declined`, or the raise
 * failed; or nothing was asked, because another request is `already-asking`.
 */
export type RaiseOutcome = "permits" | "declined" | "already-asking";

export interface SafetyStatus {
	/** The mode the session is in. Read-only until the operator raises it. */
	mode: SafetyMode;
	/**
	 * Raise to a mode, without asking. Resolves whether the raise was made; when
	 * it was not, the operator has already been told their mode is unchanged.
	 */
	raise: (mode: SafetyMode) => Promise<boolean>;
	/**
	 * Return to read-only at once, without waiting for the countdown. Resolves
	 * whether it was done, like {@link raise}.
	 */
	lower: () => Promise<boolean>;
	/**
	 * Ask the operator to confirm a raise, and make it. Resolves `permits` once
	 * the raise has landed and the new mode has rendered, so a control that was
	 * blocked is already usable (see {@link RaiseOutcome}).
	 */
	requestRaise: (request: RaiseRequest) => Promise<RaiseOutcome>;
	/** True while a raise or lower is in flight. */
	busy: boolean;
}

const SafetyContext = createContext<SafetyStatus | null>(null);

// The time left is its own context because it changes every second while a
// raise is live. Every graded control on the page reads the mode; only the
// indicator reads this, so only the indicator re-renders on the tick.
const RemainingContext = createContext<number | null>(null);

/**
 * The operator's safety-mode session.
 *
 * Mounted once near the router root. The session identifier is held here and
 * published to the API layer, which puts it on every request; it is never
 * persisted, so a reloaded page asks for a fresh session and comes back
 * read-only.
 *
 * Nothing here is a security boundary — the server decides every graded request
 * itself. What this is for is telling the operator which mode they are in and
 * how long they have left.
 */
export function SafetyModeProvider({ children }: { children: ReactNode }) {
	const [session, setSession] = useState<SessionState | null>(null);
	const [busy, setBusy] = useState(false);
	// Drives the countdown. A raise is time-limited, so the displayed remainder
	// has to move on its own rather than only when something else re-renders.
	const [now, setNow] = useState(() => Date.now());

	// Set synchronously on adoption, ahead of the re-render, so an answer that
	// arrives late can tell a session has been adopted since it was asked for.
	const adopted = useRef(false);
	const adopt = useCallback((next: SessionState) => {
		adopted.current = true;
		setSession(next);
		publishSession(next.id);
	}, []);

	// Ask for a session as soon as the app connects, so one always exists and a
	// later raise modifies the one already there. The mode control is usable
	// before this answers, and a raise made in that gap brings its own session,
	// so a late answer never replaces one adopted since.
	useEffect(() => {
		let live = true;
		callApi("safety", "session", {})
			.then((next) => {
				if (live && !adopted.current) adopt(next as SessionState);
			})
			.catch(() => {
				// A client with no session is read-only, which is where it starts
				// anyway, so a failure here costs nothing until the operator raises.
			});
		return () => {
			live = false;
		};
	}, [adopt]);

	const expiresAt = useMemo(() => {
		if (!session?.raise_expires_at) return null;
		const at = Date.parse(session.raise_expires_at);
		return Number.isNaN(at) ? null : at;
	}, [session?.raise_expires_at]);

	// The stored mode is what the server last said; a raise that has run out
	// reads as read-only here so the indicator and the server agree. So does a
	// raise carrying no time this client can read: doubt resolves downwards,
	// which is how the server reads a raise with no expiry too.
	const raised = !!session && session.mode !== "read-only";
	const lapsed = raised && (expiresAt === null || now >= expiresAt);
	const mode: SafetyMode = !raised || lapsed ? "read-only" : session.mode;
	// Clamped to the length of a raise: the server sets the expiry from its own
	// clock, so a client running a little behind would otherwise open the
	// countdown above ten minutes.
	const remainingMs =
		mode === "read-only" || expiresAt === null
			? null
			: Math.min(RAISE_DURATION_MS, Math.max(0, expiresAt - now));

	// Tick only while something is counting down.
	useEffect(() => {
		if (remainingMs === null) return;
		const id = window.setInterval(() => setNow(Date.now()), 1000);
		return () => window.clearInterval(id);
	}, [remainingMs === null]);

	// The server refusing a request for its mode is the authoritative word that
	// the raise is over, whatever the countdown thinks.
	const sessionRef = useRef(session);
	sessionRef.current = session;
	useEffect(() => {
		setRaiseLapsedHandler(() => {
			const held = sessionRef.current;
			if (held) setSession({ ...held, mode: "read-only", raise_expires_at: null });
		});
		return () => setRaiseLapsedHandler(null);
	}, []);

	const [failed, setFailed] = useState(false);

	// Committed synchronously, so whoever awaits a change of mode finds the
	// controls already drawn for it.
	const change = useCallback(
		async (ask: () => Promise<unknown>) => {
			setBusy(true);
			setFailed(false);
			try {
				const next = (await ask()) as SessionState;
				flushSync(() => adopt(next));
				return true;
			} catch {
				setFailed(true);
				return false;
			} finally {
				setBusy(false);
			}
		},
		[adopt],
	);

	const raise = useCallback(
		(to: SafetyMode) => change(() => callApi("safety", "raise", { mode: to })),
		[change],
	);

	const lower = useCallback(
		() => change(() => callApi("safety", "lower", {})),
		[change],
	);

	// One request at a time: it is held from being asked until the raise has
	// landed, so a second activation in that time does nothing further.
	const modeRef = useRef(mode);
	modeRef.current = mode;
	const requesting = useRef(false);
	const [asked, setAsked] = useState<{
		request: RaiseRequest;
		settle: (raised: boolean) => void;
	} | null>(null);

	const requestRaise = useCallback((request: RaiseRequest) => {
		if (permits(modeRef.current, request.mode)) {
			return Promise.resolve<RaiseOutcome>("permits");
		}
		if (requesting.current) return Promise.resolve<RaiseOutcome>("already-asking");
		requesting.current = true;
		return new Promise<RaiseOutcome>((resolve) => {
			setAsked({
				request,
				settle: (raised) => {
					requesting.current = false;
					resolve(raised ? "permits" : "declined");
				},
			});
		});
	}, []);

	const confirmAsked = () => {
		if (!asked) return;
		const { request, settle } = asked;
		setAsked(null);
		raise(request.mode).then(settle);
	};
	const cancelAsked = () => {
		asked?.settle(false);
		setAsked(null);
	};

	const value = useMemo<SafetyStatus>(
		() => ({ mode, raise, lower, requestRaise, busy }),
		[mode, raise, lower, requestRaise, busy],
	);

	return (
		<SafetyContext.Provider value={value}>
			<RemainingContext.Provider value={remainingMs}>
				{children}
			</RemainingContext.Provider>
			<RaiseDialog
				request={asked?.request ?? null}
				onConfirm={confirmAsked}
				onCancel={cancelAsked}
			/>
			<Dialog open={failed} onClose={() => setFailed(false)}>
				<DialogTitle>Mode unchanged</DialogTitle>
				<DialogContent>
					<Typography color="text.secondary">Could not change mode.</Typography>
				</DialogContent>
				<DialogActions>
					<Button onClick={() => setFailed(false)}>Close</Button>
				</DialogActions>
			</Dialog>
		</SafetyContext.Provider>
	);
}

/** The operator's current safety mode and the controls that change it. */
export function useSafetyMode(): SafetyStatus {
	const ctx = useContext(SafetyContext);
	if (!ctx) {
		throw new Error("useSafetyMode must be used inside <SafetyModeProvider>");
	}
	return ctx;
}

/**
 * Milliseconds left on the current raise, or null while read-only.
 *
 * Changes every second while a raise is live, so read it only where it is
 * shown: a component reading this re-renders on every tick.
 */
export function useRemainingMs(): number | null {
	return useContext(RemainingContext);
}
