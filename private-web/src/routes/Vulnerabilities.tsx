import {
	Alert,
	Box,
	FormControlLabel,
	IconButton,
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
	TextField,
	Tooltip,
	Typography,
} from "@mui/material";
import RefreshIcon from "@mui/icons-material/Refresh";
import { Link as RouterLink, useNavigate, useSearchParams } from "react-router-dom";
import { useApi } from "../api";
import {
	SeverityChip,
	SourceChip,
	StatusChip,
} from "../components/VulnerabilityChips";
import { usePageTitle } from "../hooks/usePageTitle";
import {
	VULNERABILITY_SOURCE_LABEL,
	VULNERABILITY_STATUS_LABEL,
	type VulnerabilityItem,
	type VulnerabilitySource,
	type VulnerabilityStatus,
} from "../types";

const STATUSES = Object.keys(VULNERABILITY_STATUS_LABEL) as VulnerabilityStatus[];
const SOURCES = Object.keys(VULNERABILITY_SOURCE_LABEL) as VulnerabilitySource[];

export function vulnerabilityPath(item: Pick<VulnerabilityItem, "source" | "id">) {
	return `/vulnerabilities/${item.source}/${encodeURIComponent(item.id)}`;
}

export default function Vulnerabilities() {
	usePageTitle("Vulnerabilities");

	// Filters live in the URL, and the defaults stay out of it.
	const [params, setParams] = useSearchParams();
	const source = (params.get("source") as VulnerabilitySource | null) ?? "";
	const statusParam = params.get("status");
	const status = statusParam ?? "open";
	const includeUncounted = params.get("showAll") === "1";

	const updateParam = (key: string, value: string | null) => {
		setParams(
			(prev) => {
				const next = new URLSearchParams(prev);
				if (value === null || value === "") next.delete(key);
				else next.set(key, value);
				return next;
			},
			{ replace: true },
		);
	};

	const list = useApi(
		"vulnerabilities",
		"list",
		{
			source: source === "" ? null : source,
			status: status === "all" ? null : status,
			include_uncounted: includeUncounted,
		},
		[source, status, includeUncounted],
	);

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
					<Tooltip title="Include items nothing counts for any more">
						<FormControlLabel
							control={
								<Switch
									size="small"
									checked={includeUncounted}
									onChange={(e) =>
										updateParam("showAll", e.target.checked ? "1" : null)
									}
								/>
							}
							label="Show all"
						/>
					</Tooltip>
					<TextField
						select
						size="small"
						label="Source"
						value={source}
						onChange={(e) => updateParam("source", e.target.value)}
						sx={{ minWidth: 200 }}
					>
						<MenuItem value="">All</MenuItem>
						{SOURCES.map((s) => (
							<MenuItem key={s} value={s}>
								{VULNERABILITY_SOURCE_LABEL[s]}
							</MenuItem>
						))}
					</TextField>
					<TextField
						select
						size="small"
						label="Status"
						value={status}
						onChange={(e) =>
							updateParam("status", e.target.value === "open" ? null : e.target.value)
						}
						sx={{ minWidth: 200 }}
					>
						<MenuItem value="all">All</MenuItem>
						{STATUSES.map((s) => (
							<MenuItem key={s} value={s}>
								{VULNERABILITY_STATUS_LABEL[s]}
							</MenuItem>
						))}
					</TextField>
					<Box sx={{ ml: "auto" }}>
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
								<TableCell>Severity</TableCell>
								<TableCell>Vulnerability</TableCell>
								<TableCell>Package</TableCell>
								<TableCell>Exposed</TableCell>
								<TableCell>Status</TableCell>
							</TableRow>
						</TableHead>
						<TableBody>
							{list.data.items.map((item) => (
								<ItemRow key={`${item.source}/${item.id}`} item={item} />
							))}
						</TableBody>
					</Table>
				</Paper>
			)}
		</Stack>
	);
}

function ItemRow({ item }: { item: VulnerabilityItem }) {
	const navigate = useNavigate();
	const to = vulnerabilityPath(item);
	return (
		<TableRow
			hover
			onClick={() => navigate(to)}
			sx={{ cursor: "pointer", "&:last-child td": { borderBottom: 0 } }}
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
					{item.summary || item.id}
				</MuiLink>
				<Typography
					variant="caption"
					component="div"
					color="text.secondary"
					sx={{ fontFamily: "monospace" }}
				>
					{item.id}
				</Typography>
			</TableCell>
			<TableCell>
				{item.packages.join(", ")}
				<Box sx={{ mt: 0.25 }}>
					<SourceChip source={item.source} />
				</Box>
			</TableCell>
			<TableCell>
				{item.exposed} {exposedUnit(item)}
				{!item.counts && (
					<Typography variant="caption" component="div" color="text.secondary">
						Not counted
					</Typography>
				)}
			</TableCell>
			<TableCell>
				<StatusChip status={item.status} />
				{item.status === "open" && item.suggestion && (
					<Typography variant="caption" component="div" color="text.secondary">
						Suggested: {VULNERABILITY_STATUS_LABEL[item.suggestion.outcome].toLowerCase()}
					</Typography>
				)}
			</TableCell>
		</TableRow>
	);
}

function exposedUnit(item: VulnerabilityItem): string {
	const noun = item.source === "tamanu" ? "application" : "machine";
	return item.exposed === 1 ? noun : `${noun}s`;
}
