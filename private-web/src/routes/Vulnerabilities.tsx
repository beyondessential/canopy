import {
	Alert,
	Box,
	Button,
	FormControlLabel,
	IconButton,
	InputAdornment,
	LinearProgress,
	Link as MuiLink,
	MenuItem,
	Paper,
	Stack,
	Switch,
	Table,
	TableBody,
	TableCell,
	TableHead,
	TableRow,
	TableSortLabel,
	TextField,
	Typography,
} from "@mui/material";
import ClearIcon from "@mui/icons-material/Clear";
import RefreshIcon from "@mui/icons-material/Refresh";
import { type ReactNode, useEffect, useState } from "react";
import { Link as RouterLink, useNavigate, useSearchParams } from "react-router-dom";
import { useApi } from "../api";
import { SeverityChip, StatusChip } from "../components/VulnerabilityChips";
import { usePageTitle } from "../hooks/usePageTitle";
import {
	VULNERABILITY_BASIS_LABEL,
	VULNERABILITY_SCOPE_LABEL,
	VULNERABILITY_SEVERITY_LABEL,
	VULNERABILITY_STATUS_LABEL,
	type VulnerabilityFacetCount,
	type VulnerabilityFacets,
	type VulnerabilityItem,
	type VulnerabilitySortColumn,
	type VulnerabilityStatus,
} from "../types";

export function vulnerabilityPath(item: Pick<VulnerabilityItem, "source" | "id">) {
	return `/vulnerabilities/${item.source}/${encodeURIComponent(item.id)}`;
}

const STATUSES = Object.keys(VULNERABILITY_STATUS_LABEL) as VulnerabilityStatus[];
const SEVERITIES = ["critical", "high", "medium", "low", "unrated"] as const;
const SUGGESTION_LABEL: Record<string, string> = {
	yes: "Has suggestion",
	no: "No suggestion",
};

/** The sort the list shows when none is chosen. */
const DEFAULT_SORT: { column: VulnerabilitySortColumn; descending: boolean } = {
	column: "severity",
	descending: true,
};

type Column = {
	label: string;
	sort: VulnerabilitySortColumn;
};

const COLUMNS: Column[] = [
	{ label: "Severity", sort: "severity" },
	{ label: "Vulnerability", sort: "summary" },
	{ label: "Package", sort: "package" },
	{ label: "Source", sort: "source" },
	{ label: "Scope", sort: "scope" },
	{ label: "Status", sort: "status" },
	{ label: "Machines", sort: "machines" },
];

/** A debounced copy of `value`. */
function useDebounced<T>(value: T, ms: number): T {
	const [debounced, setDebounced] = useState(value);
	useEffect(() => {
		const timer = setTimeout(() => setDebounced(value), ms);
		return () => clearTimeout(timer);
	}, [value, ms]);
	return debounced;
}

export default function Vulnerabilities() {
	usePageTitle("Vulnerabilities");

	// Everything the list shows lives in the URL, and the defaults stay out of it.
	const [params, setParams] = useSearchParams();
	const source = params.get("source") ?? "";
	const status = params.get("status") ?? "open";
	const scope = params.get("scope") ?? "";
	const severity = params.get("severity") ?? "";
	const suggestion = params.get("suggestion") ?? "";
	const includeClosed = params.get("closed") === "1";
	const search = params.get("q") ?? "";
	const sortColumn = (params.get("sort") as VulnerabilitySortColumn | null) ?? null;
	const descending = params.get("dir") === "desc";

	const updateParams = (changes: Record<string, string | null>) => {
		setParams(
			(prev) => {
				const next = new URLSearchParams(prev);
				for (const [key, value] of Object.entries(changes)) {
					if (value === null || value === "") next.delete(key);
					else next.set(key, value);
				}
				return next;
			},
			{ replace: true },
		);
	};

	// The search box types ahead of the URL, which follows once typing pauses.
	const [typed, setTyped] = useState(search);
	const debounced = useDebounced(typed, 250);
	useEffect(() => {
		if (debounced.trim() !== search) updateParams({ q: debounced.trim() });
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [debounced]);
	// A term that arrives from the address, rather than from typing, is shown,
	// and not written back over by what was typed before.
	useEffect(() => {
		if (search !== typed.trim() && search !== debounced.trim()) setTyped(search);
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [search]);

	const queryKey = [
		source,
		status,
		scope,
		severity,
		suggestion,
		includeClosed,
		search,
		sortColumn,
		descending,
	].join("|");
	// Pages are walked with the cursors the server gave, kept with the query
	// they belong to: a different query has no stack, in the same render that
	// it changes in, so it is never fetched with another query's cursor.
	const [paging, setPaging] = useState<{ key: string; stack: string[] }>({
		key: queryKey,
		stack: [],
	});
	const stack = paging.key === queryKey ? paging.stack : [];
	const cursor = stack.at(-1) ?? null;

	const list = useApi(
		"vulnerabilities",
		"list",
		{
			source: source || null,
			status: status === "all" ? null : status,
			scope: scope || null,
			severity: severity || null,
			has_suggestion: suggestion === "" ? null : suggestion === "yes",
			include_closed: includeClosed,
			search: search || null,
			sort: sortColumn ? { column: sortColumn, descending } : null,
			cursor,
		},
		[queryKey, cursor],
	);

	const sortBy = (column: VulnerabilitySortColumn) => {
		const active = sortColumn ?? DEFAULT_SORT.column;
		const activeDescending = sortColumn ? descending : DEFAULT_SORT.descending;
		updateParams({
			sort: column,
			dir: column === active && !activeDescending ? "desc" : column === active ? "asc" : null,
		});
	};

	// Only a query's first page carries its facets, so they are kept while the
	// later pages are walked and until the next first page replaces them.
	const [facets, setFacets] = useState<VulnerabilityFacets | null>(null);
	const delivered = list.status === "ok" ? list.data.facets : null;
	useEffect(() => {
		if (delivered) setFacets(delivered);
	}, [delivered]);

	return (
		<Stack spacing={3}>
			<Typography variant="h4" component="h1">
				Vulnerabilities
			</Typography>

			<Paper variant="outlined" sx={{ p: 2 }}>
				<Stack
					direction="row"
					spacing={2}
					sx={{ alignItems: "center", flexWrap: "wrap" }}
					useFlexGap
				>
					<FormControlLabel
						control={
							<Switch
								size="small"
								checked={includeClosed}
								onChange={(e) => updateParams({ closed: e.target.checked ? "1" : null })}
							/>
						}
						label="Show closed"
					/>
					<TextField
						size="small"
						label="Search"
						value={typed}
						onChange={(e) => setTyped(e.target.value)}
						sx={{ minWidth: 240 }}
						slotProps={{
							input: {
								endAdornment: typed ? (
									<InputAdornment position="end">
										<IconButton
											size="small"
											aria-label="Clear search"
											onClick={() => {
												setTyped("");
												updateParams({ q: null });
											}}
										>
											<ClearIcon fontSize="small" />
										</IconButton>
									</InputAdornment>
								) : null,
							},
						}}
					/>
					<Facet
						label="Source"
						value={source}
						onChange={(value) => updateParams({ source: value })}
						choices={facets?.source ?? []}
						labelOf={(value) => value}
					total={facets?.totals.source ?? null}
					/>
					<Facet
						label="Status"
						value={status}
						onChange={(value) => updateParams({ status: value === "open" ? null : value })}
						choices={sortedStatuses(facets?.status ?? [])}
						labelOf={(value) => VULNERABILITY_STATUS_LABEL[value as VulnerabilityStatus] ?? value}
						allValue="all"
					total={facets?.totals.status ?? null}
					/>
					<Facet
						label="Scope"
						value={scope}
						onChange={(value) => updateParams({ scope: value })}
						choices={facets?.scope ?? []}
						labelOf={(value) => VULNERABILITY_SCOPE_LABEL[value] ?? value}
					total={facets?.totals.scope ?? null}
					/>
					<Facet
						label="Severity"
						value={severity}
						onChange={(value) => updateParams({ severity: value })}
						choices={sortedSeverities(facets?.severity ?? [])}
						labelOf={(value) =>
							VULNERABILITY_SEVERITY_LABEL[value as keyof typeof VULNERABILITY_SEVERITY_LABEL] ??
							value
						}
					total={facets?.totals.severity ?? null}
					/>
					<Facet
						label="Suggestion"
						value={suggestion}
						onChange={(value) => updateParams({ suggestion: value })}
						choices={facets?.suggestion ?? []}
						labelOf={(value) => SUGGESTION_LABEL[value] ?? value}
					total={facets?.totals.suggestion ?? null}
					/>
					<Box sx={{ ml: "auto", display: "flex", alignItems: "center", gap: 1 }}>
						{list.status === "ok" && (
							<Typography variant="body2" color="text.secondary">
								<b>{list.data.total}</b> {list.data.total === 1 ? "result" : "results"}
							</Typography>
						)}
						<IconButton aria-label="Refresh" size="small" onClick={list.reload}>
							<RefreshIcon fontSize="small" />
						</IconButton>
					</Box>
				</Stack>
			</Paper>

			{list.status === "loading" || list.status === "idle" ? (
				<LinearProgress />
			) : list.status === "error" ? (
				<Alert severity="error">{list.error.message}</Alert>
			) : list.data.items.length === 0 ? (
				<Alert severity="info">No vulnerabilities match the filters.</Alert>
			) : (
				<Paper variant="outlined">
					<Table size="small">
						<TableHead>
							<TableRow>
								{COLUMNS.map((column) => {
									const active = (sortColumn ?? DEFAULT_SORT.column) === column.sort;
									const activeDescending = sortColumn ? descending : DEFAULT_SORT.descending;
									return (
										<TableCell
											key={column.sort}
											sortDirection={active ? (activeDescending ? "desc" : "asc") : false}
										>
											<TableSortLabel
												active={active}
												direction={active && activeDescending ? "desc" : "asc"}
												onClick={() => sortBy(column.sort)}
											>
												{column.label}
											</TableSortLabel>
										</TableCell>
									);
								})}
							</TableRow>
						</TableHead>
						<TableBody>
							{list.data.items.map((item) => (
								<ItemRow key={`${item.source}/${item.id}`} item={item} search={search} />
							))}
						</TableBody>
					</Table>
					{(stack.length > 0 || list.data.next_cursor) && (
						<Stack
							direction="row"
							spacing={1}
							sx={{ p: 1, justifyContent: "flex-end", borderTop: 1, borderColor: "divider" }}
						>
							<Button
								size="small"
								disabled={stack.length === 0}
								onClick={() => setPaging({ key: queryKey, stack: stack.slice(0, -1) })}
							>
								Previous
							</Button>
							<Button
								size="small"
								disabled={!list.data.next_cursor}
								onClick={() => {
									const next = list.data.next_cursor;
									if (next) setPaging({ key: queryKey, stack: [...stack, next] });
								}}
							>
								Next
							</Button>
						</Stack>
					)}
				</Paper>
			)}
		</Stack>
	);
}

function sortedStatuses(counts: VulnerabilityFacetCount[]): VulnerabilityFacetCount[] {
	return [...counts].sort(
		(a, b) =>
			STATUSES.indexOf(a.value as VulnerabilityStatus) -
			STATUSES.indexOf(b.value as VulnerabilityStatus),
	);
}

function sortedSeverities(counts: VulnerabilityFacetCount[]): VulnerabilityFacetCount[] {
	return [...counts].sort(
		(a, b) =>
			SEVERITIES.indexOf(a.value as (typeof SEVERITIES)[number]) -
			SEVERITIES.indexOf(b.value as (typeof SEVERITIES)[number]),
	);
}

/** A filter: a select whose choices say how many items each would give. */
function Facet({
	label,
	value,
	onChange,
	choices,
	labelOf,
	allValue = "",
	total,
}: {
	label: string;
	value: string;
	onChange: (value: string) => void;
	choices: VulnerabilityFacetCount[];
	labelOf: (value: string) => string;
	allValue?: string;
	/** How many items the filter gives with none of its choices picked, which
	 * is not the sum of the choices where an item falls in several. */
	total: number | null;
}) {
	// What is chosen stays offered while the counts are not yet in.
	const offered = choices.some((choice) => choice.value === value) || value === allValue
		? choices
		: [...choices, { value, count: 0 }];
	const option = (key: string, text: ReactNode, count: number | null) => (
		<MenuItem key={key} value={key}>
			<Box sx={{ display: "flex", width: "100%", justifyContent: "space-between", gap: 2 }}>
				<span>{text}</span>
				{count !== null && (
					<Typography component="span" variant="body2" color="text.secondary">
						{count}
					</Typography>
				)}
			</Box>
		</MenuItem>
	);
	return (
		<TextField
			select
			size="small"
			label={label}
			value={value === "" ? allValue : value}
			onChange={(e) => onChange(e.target.value)}
			sx={{ minWidth: 170 }}
			slotProps={{
				select: {
					renderValue: (selected) => {
						const key = String(selected);
						return key === allValue ? "All" : labelOf(key);
					},
				},
			}}
		>
			{option(allValue, "All", total)}
			{offered.map((choice) => option(choice.value, labelOf(choice.value), choice.count))}
		</TextField>
	);
}

/** `text` with the parts the search matched marked. */
function Marked({ text, search }: { text: string; search: string }) {
	const terms = search
		.split(/\s+/)
		.map((term) => term.trim())
		.filter((term) => term.length > 1);
	if (terms.length === 0) return <>{text}</>;
	const escaped = terms.map((term) => term.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
	const pattern = new RegExp(`(${escaped.join("|")})`, "gi");
	const lower = new Set(terms.map((term) => term.toLowerCase()));
	return (
		<>
			{text.split(pattern).map((part, index) =>
				lower.has(part.toLowerCase()) ? (
					<Box
						component="mark"
						key={index}
						sx={{ bgcolor: "warning.light", color: "inherit", borderRadius: "2px" }}
					>
						{part}
					</Box>
				) : (
					<span key={index}>{part}</span>
				),
			)}
		</>
	);
}

function ItemRow({ item, search }: { item: VulnerabilityItem; search: string }) {
	const navigate = useNavigate();
	const to = vulnerabilityPath(item);
	const shown = item.packages.slice(0, 3);
	return (
		<TableRow
			hover
			onClick={() => navigate(to)}
			sx={{
				cursor: "pointer",
				"&:last-child td": { borderBottom: 0 },
				...(item.closed && { opacity: 0.7 }),
			}}
		>
			<TableCell>
				<SeverityChip severity={item.severity} />
			</TableCell>
			<TableCell>
				<MuiLink
					component={RouterLink}
					to={to}
					underline="hover"
					color="inherit"
					onClick={(e) => e.stopPropagation()}
				>
					<Marked text={item.summary || item.id} search={search} />
				</MuiLink>
				<Typography
					variant="caption"
					component="div"
					color="text.secondary"
					sx={{ fontFamily: "monospace" }}
				>
					<Marked text={item.id} search={search} />
				</Typography>
			</TableCell>
			<TableCell sx={{ fontFamily: "monospace" }}>
				{shown.map((pkg) => (
					<Box key={`${pkg.name}/${pkg.version}`}>
						<Marked text={pkg.name} search={search} /> {pkg.version}
						{pkg.min_fix_version && (
							<Typography
								variant="caption"
								component="div"
								color="text.secondary"
								sx={{ fontFamily: "inherit" }}
							>
								fix in {pkg.min_fix_version}
							</Typography>
						)}
					</Box>
				))}
				{item.packages.length > shown.length && (
					<Typography variant="caption" color="text.secondary">
						+{item.packages.length - shown.length} more
					</Typography>
				)}
			</TableCell>
			<TableCell>
				<Marked text={item.source} search={search} />
			</TableCell>
			<TableCell>
				{item.scopes.map((scope) => VULNERABILITY_SCOPE_LABEL[scope]?.toLowerCase() ?? scope).join(", ")}
			</TableCell>
			<TableCell>
				<StatusChip status={item.status} />
				{item.closed && <Note>Closed</Note>}
				{item.lapse && item.status === "open" && <Note>Decision lapsed</Note>}
				{item.withdrawn && item.status === "open" && <Note>Withdrawn</Note>}
				{item.edited && <Note>Edited</Note>}
				{item.status === "open" && item.suggestion && (
					<>
						<Note>
							Suggested: {VULNERABILITY_STATUS_LABEL[item.suggestion.status].toLowerCase()}
						</Note>
						<Note>{VULNERABILITY_BASIS_LABEL[item.suggestion.basis]}</Note>
					</>
				)}
			</TableCell>
			<TableCell>
				{item.source === "linux" && item.machines > 0
					? `${item.machines} ${item.machines === 1 ? "machine" : "machines"}`
					: null}
			</TableCell>
		</TableRow>
	);
}

function Note({ children }: { children: ReactNode }) {
	return (
		<Typography variant="caption" component="div" color="text.secondary">
			{children}
		</Typography>
	);
}

