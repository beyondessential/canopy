import { useCallback, useEffect, useRef, useState } from "react";

import type { ApiBody, ApiFn, ApiModule, ApiResponse } from "./types";

export class ApiError extends Error {
	readonly status: number;
	readonly detail: unknown;

	constructor(status: number, message: string, detail: unknown) {
		super(message);
		this.name = "ApiError";
		this.status = status;
		this.detail = detail;
	}
}

// `M` and `F` are inferred from the positional args and constrain the
// (module, fn) pair against the generated `paths` interface. `T` defaults to
// the response type for that path, so most call sites don't need to spell it
// out — they only need to override when narrowing or shaping locally.
export async function callApi<
	M extends ApiModule,
	F extends ApiFn<M>,
	T = ApiResponse<M, F>,
>(
	module: M,
	fn: F,
	params: ApiBody<M, F> | Record<string, unknown> = {},
	signal?: AbortSignal,
): Promise<T> {
	const response = await fetch(`/api/${module}/${fn}`, {
		method: "POST",
		headers: { "content-type": "application/json" },
		body: JSON.stringify(params),
		signal,
	});

	return (await answered(response, module, fn)) as T;
}

async function answered(
	response: Response,
	module: string,
	fn: string,
): Promise<unknown> {
	if (response.ok) return await response.json();

	let detail: unknown = null;
	try {
		detail = await response.json();
	} catch {
		detail = await response.text().catch(() => null);
	}
	// Surface the problem-details title (and detail line, if present)
	// in the thrown error's message so action.error?.message in the UI
	// shows the actual server-side cause, not just the HTTP status.
	let extra = "";
	if (
		detail &&
		typeof detail === "object" &&
		"title" in detail &&
		typeof (detail as { title?: unknown }).title === "string"
	) {
		extra = `: ${(detail as { title: string }).title}`;
	}
	throw new ApiError(
		response.status,
		`server fn ${module}.${fn} failed: ${response.status}${extra}`,
		detail,
	);
}

// An endpoint whose body is the bytes themselves: everything it is told about
// them travels in the query string.
export async function uploadApi<T>(
	module: string,
	fn: string,
	query: Record<string, string>,
	body: Blob,
): Promise<T> {
	const response = await fetch(
		`/api/${module}/${fn}?${new URLSearchParams(query)}`,
		{
			method: "POST",
			headers: {
				"content-type": body.type || "application/octet-stream",
				// A raw body is a content type a form can send, so this is what
				// makes the browser preflight the request and keeps a
				// cross-origin page off the endpoint.
				"x-canopy-upload": "1",
			},
			body,
		},
	);

	return (await answered(response, module, fn)) as T;
}

export type ApiState<T> =
	| { status: "idle" }
	| { status: "loading" }
	| { status: "ok"; data: T }
	| { status: "error"; error: Error };

/**
 * A stable string for "which entity is this query about": the module, the
 * function, and the params by value.
 *
 * Deliberately *not* the `deps` array. Callers legitimately put a refetch
 * nonce in `deps` (`[id, tick]`, bumped to force a reload of the same
 * entity), and treating that as a new entity would blank the page — and
 * unmount anything with local state inside it — on every manual refresh.
 */
function queryIdentity(
	module: string,
	fn: string,
	params: Record<string, unknown>,
): string {
	const stable = Object.keys(params)
		.sort()
		.map((k) => [k, params[k]]);
	return JSON.stringify([module, fn, stable]);
}

export function useApi<
	M extends ApiModule,
	F extends ApiFn<M>,
	T = ApiResponse<M, F>,
>(
	module: M,
	fn: F,
	params: Record<string, unknown> = {},
	deps: ReadonlyArray<unknown> = [],
	/** `skip` leaves the hook idle without calling the endpoint, for when the
	 * params aren't available yet (a filter the caller doesn't have). Hooks
	 * can't be called conditionally, so the condition has to live here. */
	options: { skip?: boolean } = {},
): ApiState<T> & { reload: () => void } {
	const [state, setState] = useState<ApiState<T>>({ status: "idle" });
	const tick = useRef(0);
	// Identity of the query whose data is currently on screen, so a refetch
	// can be told apart from a switch to a different entity.
	const shownIdentity = useRef<string | null>(null);
	const skip = options.skip ?? false;

	const run = useCallback(() => {
		if (skip) {
			// Bump the tick so an in-flight response from before the skip
			// can't land and present stale data as current.
			tick.current++;
			setState({ status: "idle" });
			return;
		}
		const myTick = ++tick.current;
		const controller = new AbortController();
		// Keep prior data on screen during background refetches so the UI
		// doesn't collapse to a loading placeholder on every reload tick —
		// but only when it's still the *same* query. Detail routes reuse the
		// mounted component across /applications/A → /applications/B, so holding on to
		// prior data across an identity change renders A's name, health and
		// checks under B's URL, with no loading indicator to say otherwise.
		const identity = queryIdentity(module, fn, params);
		const sameQuery = shownIdentity.current === identity;
		setState((prev) =>
			prev.status === "ok" && sameQuery ? prev : { status: "loading" },
		);
		callApi<M, F, T>(module, fn, params, controller.signal)
			.then((data) => {
				if (tick.current === myTick) {
					shownIdentity.current = identity;
					setState({ status: "ok", data });
				}
			})
			.catch((error: unknown) => {
				if (controller.signal.aborted) return;
				if (tick.current !== myTick) return;
				setState({
					status: "error",
					error: error instanceof Error ? error : new Error(String(error)),
				});
			});
		return () => controller.abort();
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [...deps, skip]);

	useEffect(() => run(), [run]);

	return { ...state, reload: run };
}

/**
 * Hook for write/mutation server fns. Returns a `call` function that
 * performs the request and a `pending` / `error` state for the UI.
 * The caller decides what to do with the result (e.g. refetch a
 * `useApi` resource).
 */
export function useApiAction<
	M extends ApiModule,
	F extends ApiFn<M>,
	T = ApiResponse<M, F>,
>(
	module: M,
	fn: F,
): {
	call: (params?: Record<string, unknown>) => Promise<T>;
	pending: boolean;
	error: Error | null;
	reset: () => void;
} {
	return useApiCall<[Record<string, unknown>?], T>(
		useCallback(
			(params = {}) => callApi<M, F, T>(module, fn, params),
			[module, fn],
		),
	);
}

/**
 * `useApiAction` for an endpoint whose body is the bytes themselves.
 */
export function useApiUpload<T>(
	module: string,
	fn: string,
): {
	call: (query: Record<string, string>, body: Blob) => Promise<T>;
	pending: boolean;
	error: Error | null;
	reset: () => void;
} {
	return useApiCall<[Record<string, string>, Blob], T>(
		useCallback(
			(query, body) => uploadApi<T>(module, fn, query, body),
			[module, fn],
		),
	);
}

/**
 * The pending/error bookkeeping both write hooks share, so the two cannot
 * differ on what they report or on telling the rest of the page that
 * something changed.
 */
function useApiCall<A extends unknown[], T>(
	request: (...args: A) => Promise<T>,
): {
	call: (...args: A) => Promise<T>;
	pending: boolean;
	error: Error | null;
	reset: () => void;
} {
	const [pending, setPending] = useState(false);
	const [error, setError] = useState<Error | null>(null);

	const call = useCallback(
		async (...args: A): Promise<T> => {
			setPending(true);
			setError(null);
			try {
				const result = await request(...args);
				// Broadcast so global, page-agnostic queries (e.g. the open-
				// incidents nav badge) can refetch without the caller having
				// to know they exist. Listeners hook via useReloadInterval.
				document.dispatchEvent(new Event("canopy-data-changed"));
				return result;
			} catch (err) {
				const e = err instanceof Error ? err : new Error(String(err));
				setError(e);
				throw e;
			} finally {
				setPending(false);
			}
		},
		[request],
	);

	const reset = useCallback(() => setError(null), []);

	return { call, pending, error, reset };
}
