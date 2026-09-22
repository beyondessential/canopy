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
import { callApi } from "../api";
import {
	RAISE_DURATION_MS,
	type SafetyMode,
	publishSession,
	setRaiseLapsedHandler,
} from "../safety";

/** What the server says about the caller's session. */
interface SessionState {
	id: string;
	mode: SafetyMode;
	raise_expires_at?: string | null;
}

export interface SafetyStatus {
	/** The mode the session is in. Read-only until the operator raises it. */
	mode: SafetyMode;
	/** Milliseconds left on the current raise, or null while read-only. */
	remainingMs: number | null;
	/** Raise to a mode. Rejects if the operator lacks the danger permission. */
	raise: (mode: SafetyMode) => Promise<void>;
	/** Return to read-only at once, without waiting for the countdown. */
	lower: () => Promise<void>;
	/** True while a raise or lower is in flight. */
	busy: boolean;
}

const SafetyContext = createContext<SafetyStatus | null>(null);

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
	// reads as read-only here so the indicator and the server agree.
	const lapsed = expiresAt !== null && now >= expiresAt;
	const mode: SafetyMode =
		!session || lapsed ? "read-only" : (session.mode ?? "read-only");
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

	const raise = useCallback(
		async (to: SafetyMode) => {
			setBusy(true);
			try {
				adopt((await callApi("safety", "raise", { mode: to })) as SessionState);
			} finally {
				setBusy(false);
			}
		},
		[adopt],
	);

	const lower = useCallback(async () => {
		setBusy(true);
		try {
			adopt((await callApi("safety", "lower", {})) as SessionState);
		} finally {
			setBusy(false);
		}
	}, [adopt]);

	const value = useMemo<SafetyStatus>(
		() => ({ mode, remainingMs, raise, lower, busy }),
		[mode, remainingMs, raise, lower, busy],
	);

	return (
		<SafetyContext.Provider value={value}>{children}</SafetyContext.Provider>
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
