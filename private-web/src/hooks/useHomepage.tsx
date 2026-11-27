import {
	type ReactNode,
	createContext,
	useCallback,
	useContext,
	useMemo,
	useState,
} from "react";
import { callApi, useApi } from "../api";

/** Where the root goes when neither a choice nor the policy names a page. */
export const FALLBACK_HOMEPAGE = "/status";

export interface HomepageState {
	/** The page the person has chosen, `null` where they have chosen none, or
	 * `undefined` until the answer arrives. */
	chosen: string | null | undefined;
	/** The default the tailnet policy gives them, as for `chosen`. */
	default: string | null | undefined;
	/** The page the root opens, `undefined` until the answer arrives. */
	effective: string | undefined;
	/** The answer could not be fetched. */
	error: Error | null;
	/** Choose `path` as the homepage. */
	set: (path: string) => Promise<void>;
	/** Clear the choice, so the root opens the default again. */
	clear: () => Promise<void>;
}

const HomepageContext = createContext<HomepageState | null>(null);

/** One fetch of the caller's homepage, shared by the root and the toggle so a
 * change through the toggle is what the root opens next. spec: HOME */
export function HomepageProvider({ children }: { children: ReactNode }) {
	const answer = useApi("homepage", "get");
	// A choice made this session, overriding the fetched one once made.
	const [chosenHere, setChosenHere] = useState<string | null | undefined>(
		undefined,
	);

	const set = useCallback(async (path: string) => {
		await callApi("homepage", "set", { path });
		setChosenHere(path);
	}, []);
	const clear = useCallback(async () => {
		await callApi("homepage", "clear", {});
		setChosenHere(null);
	}, []);

	const value = useMemo<HomepageState>(() => {
		const loaded = answer.status === "ok" ? answer.data : undefined;
		const chosen =
			chosenHere !== undefined
				? chosenHere
				: loaded && (loaded.chosen ?? null);
		const fallback = loaded && (loaded.default ?? null);
		return {
			chosen,
			default: fallback,
			effective: loaded
				? (chosen ?? fallback ?? FALLBACK_HOMEPAGE)
				: undefined,
			error: answer.status === "error" ? answer.error : null,
			set,
			clear,
		};
	}, [answer, chosenHere, set, clear]);

	return (
		<HomepageContext.Provider value={value}>
			{children}
		</HomepageContext.Provider>
	);
}

export function useHomepage(): HomepageState {
	const ctx = useContext(HomepageContext);
	if (!ctx) throw new Error("useHomepage must be used inside <HomepageProvider>");
	return ctx;
}
