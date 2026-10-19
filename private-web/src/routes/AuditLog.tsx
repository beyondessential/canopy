import RefreshIcon from "@mui/icons-material/Refresh";
import {
	Alert,
	Box,
	Button,
	Chip,
	Divider,
	IconButton,
	LinearProgress,
	Link as MuiLink,
	MenuItem,
	Paper,
	Stack,
	Table,
	TableBody,
	TableCell,
	TableHead,
	TableRow,
	TextField,
	Typography,
} from "@mui/material";
import { useEffect, useRef, useState } from "react";
import { Link as RouterLink } from "react-router-dom";
import { callApi, useApi } from "../api";
import ActorAvatar from "../components/ActorAvatar";
import TimeAgo from "../components/TimeAgo";
import { useIsAdmin } from "../hooks/useIsAdmin";
import { usePageTitle } from "../hooks/usePageTitle";
import {
	actorKey,
	actorLabel,
	detailPairs,
	recordPath,
} from "../lib/audit";
import type { AuditActor, AuditEntry, AuditFilters, AuditRecord } from "../types";

const PAGE = 50;
const RECORD_DEBOUNCE_MS = 300;
const HOUR_MS = 60 * 60 * 1000;

const RANGES: Array<{ value: string; label: string; ms: number | null }> = [
	{ value: "24h", label: "Last 24 hours", ms: 24 * HOUR_MS },
	{ value: "7d", label: "Last 7 days", ms: 7 * 24 * HOUR_MS },
	{ value: "30d", label: "Last 30 days", ms: 30 * 24 * HOUR_MS },
	{ value: "90d", label: "Last 90 days", ms: null },
];

// What the action filter holds: the two groupings, or `action:<name>`.
const IMPORTANT = "important";
const ALL = "all";
const ACTION_PREFIX = "action:";

export default function AuditLog() {
	usePageTitle("Audit log");
	const isAdmin = useIsAdmin();
	if (isAdmin === false) return <Alert severity="info">Administrators only.</Alert>;
	if (isAdmin === undefined) return <LinearProgress />;
	return <AuditLogPage />;
}

function AuditLogPage() {
	const filters = useApi("audit", "filters");
	const [actor, setActor] = useState("");
	const [action, setAction] = useState(IMPORTANT);
	const [range, setRange] = useState("90d");
	const [recordText, setRecordText] = useState("");
	const [record, setRecord] = useState("");
	const [tick, setTick] = useState(0);

	const [entries, setEntries] = useState<AuditEntry[]>([]);
	const [loading, setLoading] = useState(true);
	const [error, setError] = useState<Error | null>(null);
	const [full, setFull] = useState(false);
	const [loadingMore, setLoadingMore] = useState(false);
	// Bumped whenever the query changes, so a page of the old query that lands
	// late is dropped rather than shown under the new one.
	const generation = useRef(0);

	useEffect(() => {
		const id = window.setTimeout(
			() => setRecord(recordText.trim()),
			RECORD_DEBOUNCE_MS,
		);
		return () => window.clearTimeout(id);
	}, [recordText]);

	const query = (before?: string): Record<string, unknown> => {
		const [kind, ...rest] = actor.split(":");
		const ms = RANGES.find((r) => r.value === range)?.ms ?? null;
		return {
			actor_kind: actor ? kind : undefined,
			actor_id: actor ? rest.join(":") : undefined,
			action: action.startsWith(ACTION_PREFIX)
				? action.slice(ACTION_PREFIX.length)
				: undefined,
			include_routine: action === ALL,
			record: record || undefined,
			since: ms === null ? undefined : new Date(Date.now() - ms).toISOString(),
			before,
			limit: PAGE,
		};
	};

	useEffect(() => {
		const mine = ++generation.current;
		const controller = new AbortController();
		setLoading(true);
		setLoadingMore(false);
		callApi<"audit", "list", AuditEntry[]>(
			"audit",
			"list",
			query(),
			controller.signal,
		)
			.then((page) => {
				if (generation.current !== mine) return;
				setEntries(page);
				setFull(page.length >= PAGE);
				setError(null);
				setLoading(false);
			})
			.catch((err: unknown) => {
				if (controller.signal.aborted || generation.current !== mine) return;
				setError(err instanceof Error ? err : new Error(String(err)));
				setLoading(false);
			});
		return () => controller.abort();
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [actor, action, range, record, tick]);

	const loadMore = async () => {
		const last = entries[entries.length - 1];
		if (!last) return;
		const mine = generation.current;
		setLoadingMore(true);
		try {
			const page = await callApi<"audit", "list", AuditEntry[]>(
				"audit",
				"list",
				query(last.recorded_at),
			);
			if (generation.current !== mine) return;
			setEntries((prev) => [...prev, ...page]);
			setFull(page.length >= PAGE);
		} catch (err) {
			if (generation.current !== mine) return;
			setError(err instanceof Error ? err : new Error(String(err)));
		} finally {
			if (generation.current === mine) setLoadingMore(false);
		}
	};

	const options = filters.status === "ok" ? filters.data : null;

	return (
		<Stack spacing={3}>
			<FilterBar
				options={options}
				actor={actor}
				setActor={setActor}
				action={action}
				setAction={setAction}
				range={range}
				setRange={setRange}
				recordText={recordText}
				setRecordText={setRecordText}
				onRefresh={() => setTick((t) => t + 1)}
			/>

			{error ? (
				<Alert severity="error">{error.message}</Alert>
			) : loading && entries.length === 0 ? (
				<LinearProgress />
			) : entries.length === 0 ? (
				<Alert severity="info">No entries.</Alert>
			) : (
				<>
					{loading && <LinearProgress />}
					<Paper variant="outlined">
						<Table size="small">
							<TableHead>
								<TableRow>
									<TableCell>When</TableCell>
									<TableCell>Actor</TableCell>
									<TableCell>Action</TableCell>
									<TableCell>Acted on</TableCell>
									<TableCell>Detail</TableCell>
								</TableRow>
							</TableHead>
							<TableBody>
								{entries.map((entry) => (
									<EntryRow key={entry.id} entry={entry} />
								))}
							</TableBody>
						</Table>
					</Paper>
					{full && (
						<Box>
							<Button
								variant="outlined"
								onClick={loadMore}
								disabled={loadingMore}
							>
								{loadingMore ? "Loading…" : "Load more"}
							</Button>
						</Box>
					)}
				</>
			)}
		</Stack>
	);
}

function FilterBar({
	options,
	actor,
	setActor,
	action,
	setAction,
	range,
	setRange,
	recordText,
	setRecordText,
	onRefresh,
}: {
	options: AuditFilters | null;
	actor: string;
	setActor: (v: string) => void;
	action: string;
	setAction: (v: string) => void;
	range: string;
	setRange: (v: string) => void;
	recordText: string;
	setRecordText: (v: string) => void;
	onRefresh: () => void;
}) {
	const actors = options?.actors ?? [];
	const actions = options?.actions ?? [];
	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<Stack
				direction="row"
				spacing={2}
				sx={{ alignItems: "center", flexWrap: "wrap" }}
				useFlexGap
			>
				<TextField
					select
					size="small"
					label="Actor"
					value={actor}
					onChange={(e) => setActor(e.target.value)}
					sx={{ minWidth: 200 }}
					slotProps={{
						select: {
							displayEmpty: true,
							renderValue: (v) => {
								const picked = actors.find((a) => actorKey(a) === v);
								return picked ? actorLabel(picked).name : "Anyone";
							},
						},
					}}
				>
					<MenuItem value="">Anyone</MenuItem>
					{actors.map((a) => (
						<MenuItem key={actorKey(a)} value={actorKey(a)}>
							<ActorOption actor={a} />
						</MenuItem>
					))}
				</TextField>
				<TextField
					select
					size="small"
					label="Action"
					value={action}
					onChange={(e) => setAction(e.target.value)}
					sx={{ minWidth: 240 }}
					slotProps={{
						select: {
							renderValue: (v) => {
								if (v === IMPORTANT) return "Important actions";
								if (v === ALL) return "All actions";
								const picked = actions.find(
									(a) => `${ACTION_PREFIX}${a.name}` === v,
								);
								return picked?.label ?? String(v);
							},
						},
					}}
				>
					<MenuItem value={IMPORTANT}>Important actions</MenuItem>
					<MenuItem value={ALL}>All actions</MenuItem>
					<Divider />
					{actions.map((a) => (
						<MenuItem key={a.name} value={`${ACTION_PREFIX}${a.name}`}>
							{a.label}
							{a.tier === "routine" && (
								<Typography
									variant="caption"
									color="text.secondary"
									sx={{ ml: 1 }}
								>
									routine
								</Typography>
							)}
						</MenuItem>
					))}
				</TextField>
				<TextField
					select
					size="small"
					label="Time range"
					value={range}
					onChange={(e) => setRange(e.target.value)}
					sx={{ minWidth: 160 }}
				>
					{RANGES.map((r) => (
						<MenuItem key={r.value} value={r.value}>
							{r.label}
						</MenuItem>
					))}
				</TextField>
				<TextField
					size="small"
					label="Acted on"
					value={recordText}
					onChange={(e) => setRecordText(e.target.value)}
					sx={{ minWidth: 200 }}
				/>
				<Box sx={{ ml: "auto" }}>
					<IconButton aria-label="Refresh" size="small" onClick={onRefresh}>
						<RefreshIcon fontSize="small" />
					</IconButton>
				</Box>
			</Stack>
		</Paper>
	);
}

function ActorOption({ actor }: { actor: AuditActor }) {
	const { name, caption } = actorLabel(actor);
	return (
		<>
			{name}
			{caption && (
				<Typography variant="caption" color="text.secondary" sx={{ ml: 1 }}>
					{caption}
				</Typography>
			)}
		</>
	);
}

function EntryRow({ entry }: { entry: AuditEntry }) {
	const pairs = detailPairs(entry.detail);
	return (
		<TableRow hover>
			<TableCell sx={{ whiteSpace: "nowrap" }}>
				<TimeAgo timestamp={entry.recorded_at} />
			</TableCell>
			<TableCell>
				<ActorCell actor={entry.actor} />
			</TableCell>
			<TableCell sx={{ whiteSpace: "nowrap" }}>
				{entry.action_label}
				{entry.status === "failed" && (
					<Chip
						size="small"
						variant="outlined"
						color="error"
						label="failed"
						sx={{ ml: 1 }}
					/>
				)}
				{entry.status === "unconfirmed" && (
					<Chip
						size="small"
						variant="outlined"
						color="warning"
						label="unconfirmed"
						sx={{ ml: 1 }}
					/>
				)}
			</TableCell>
			<TableCell>
				<Pairs>
					{entry.records.map((r) => (
						<Pair key={`${r.kind}:${r.id}`} label={r.kind.replace(/_/g, " ")}>
							<RecordName record={r} />
						</Pair>
					))}
				</Pairs>
			</TableCell>
			<TableCell>
				<Pairs>
					{pairs.map(({ key, value }) => (
						<Pair key={key} label={key}>
							<Box
								component="span"
								title={value.text}
								sx={{
									fontFamily: value.mono ? "monospace" : undefined,
									display: "inline-block",
									maxWidth: "24ch",
									overflow: "hidden",
									textOverflow: "ellipsis",
									verticalAlign: "bottom",
								}}
							>
								{value.text}
							</Box>
						</Pair>
					))}
				</Pairs>
			</TableCell>
		</TableRow>
	);
}

function ActorCell({ actor }: { actor: AuditActor }) {
	const { name, caption } = actorLabel(actor);
	return (
		<Box>
			<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
				<ActorAvatar actor={actor} />
				{actor.kind === "device" ? (
					<MuiLink
						component={RouterLink}
						to={`/devices/${encodeURIComponent(actor.id)}`}
						underline="hover"
					>
						{name}
					</MuiLink>
				) : (
					<span>{name}</span>
				)}
			</Stack>
			{caption && (
				<Typography
					variant="caption"
					color="text.secondary"
					sx={{ display: "block", pl: 4, whiteSpace: "nowrap" }}
				>
					{caption}
				</Typography>
			)}
		</Box>
	);
}

function RecordName({ record }: { record: AuditRecord }) {
	const name = record.name || record.id;
	const to = recordPath(record);
	if (!to) return <>{name}</>;
	return (
		<MuiLink component={RouterLink} to={to} underline="hover">
			{name}
		</MuiLink>
	);
}

function Pairs({ children }: { children: React.ReactNode }) {
	return (
		<Box
			sx={{
				display: "flex",
				flexWrap: "wrap",
				alignItems: "center",
				columnGap: 2,
				rowGap: 0.25,
			}}
		>
			{children}
		</Box>
	);
}

function Pair({
	label,
	children,
}: {
	label: string;
	children: React.ReactNode;
}) {
	return (
		<Box
			sx={{
				display: "inline-flex",
				alignItems: "baseline",
				gap: 0.75,
				whiteSpace: "nowrap",
			}}
		>
			<Typography variant="caption" color="text.secondary">
				{label}
			</Typography>
			{children}
		</Box>
	);
}
