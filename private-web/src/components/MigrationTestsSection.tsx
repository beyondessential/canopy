import KeyboardArrowDownIcon from "@mui/icons-material/KeyboardArrowDown";
import KeyboardArrowUpIcon from "@mui/icons-material/KeyboardArrowUp";
import {
	Alert,
	Box,
	Chip,
	Collapse,
	IconButton,
	LinearProgress,
	Paper,
	Stack,
	Table,
	TableBody,
	TableCell,
	TableHead,
	TableRow,
	Tooltip,
	Typography,
} from "@mui/material";
import { Link as MuiLink } from "@mui/material";
import { useState } from "react";
import { Link as RouterLink } from "react-router-dom";
import { useApi } from "../api";
import type { ApiResponse, ServerInfo } from "../types";

type GroupVerdict = ApiResponse<"migration_tests", "for_group">[number];
import ServerRankChip from "./ServerRankChip";
import TimeAgo from "./TimeAgo";

/// Where each of the group's servers stands against the version it would take
/// next: the migrations were run against a restore replica of its real data, so
/// a pass means that data survives the upgrade and the timings say how long the
/// window needs to be.
// spec: RST#verdicts
export default function MigrationTestsSection({
	groupId,
	servers,
}: {
	groupId: string;
	servers: ServerInfo[];
}) {
	const byId = new Map(servers.map((server) => [server.id, server]));
	const nameOf = (id: string) => byId.get(id)?.name ?? id;
	const verdicts = useApi(
		"migration_tests",
		"for_group",
		{ group_id: groupId },
		[groupId],
	);

	if (verdicts.status === "loading" || verdicts.status === "idle") {
		return (
			<Paper variant="outlined" sx={{ p: 2 }}>
				<SectionHeading />
				<LinearProgress />
			</Paper>
		);
	}
	if (verdicts.status === "error") {
		return (
			<Paper variant="outlined" sx={{ p: 2 }}>
				<SectionHeading />
				<Alert severity="error">{verdicts.error.message}</Alert>
			</Paper>
		);
	}

	if (verdicts.data.length === 0) {
		return (
			<Paper variant="outlined" sx={{ p: 2 }} data-testid="migration-tests">
				<SectionHeading />
				<Typography variant="body2" color="text.secondary">
					No upgrade plan is open for this group, so there is nothing to
					test against.
				</Typography>
			</Paper>
		);
	}

	return (
		<Paper variant="outlined" sx={{ p: 2 }} data-testid="migration-tests">
			<SectionHeading />
			<Table size="small">
				<TableHead>
					<TableRow>
						<TableCell padding="checkbox" />
						<TableCell>Server</TableCell>
						<TableCell>Upgrading to</TableCell>
						<TableCell>Verdict</TableCell>
						<TableCell>Migrations took</TableCell>
						<TableCell>Growth</TableCell>
						<TableCell>Tested</TableCell>
					</TableRow>
				</TableHead>
				<TableBody>
					{[...verdicts.data]
						.sort(
							(a, b) =>
								VERDICT_ORDER[a.verdict] - VERDICT_ORDER[b.verdict] ||
								nameOf(a.server_id).localeCompare(nameOf(b.server_id)),
						)
						.map((row) => (
							<TestRow
								key={row.server_id}
								row={row}
								server={byId.get(row.server_id)}
							/>
						))}
				</TableBody>
			</Table>
		</Paper>
	);
}

/// Problems first: a passing server is the row an operator scrolls past.
const VERDICT_ORDER: Record<string, number> = {
	failed: 0,
	nottested: 1,
	passed: 2,
};

function SectionHeading() {
	return (
		<Stack direction="row" spacing={1} sx={{ mb: 1, alignItems: "baseline" }}>
			<Typography variant="h6" component="h2">
				Pre-upgrade migration tests
			</Typography>
			<Typography variant="body2" color="text.secondary">
				run against a restore of each server's own data
			</Typography>
		</Stack>
	);
}

function VerdictChip({
	verdict,
	failedMigration,
}: {
	verdict: "passed" | "failed" | "nottested";
	failedMigration: string | null;
}) {
	if (verdict === "passed") {
		return <Chip size="small" color="success" label="passed" />;
	}
	if (verdict === "nottested") {
		return <Chip size="small" variant="outlined" label="not yet tested" />;
	}
	return (
		<Tooltip title={failedMigration ?? "no migration named"}>
			<Chip size="small" color="warning" label="failed" />
		</Tooltip>
	);
}

function formatDuration(seconds: number) {
	if (seconds < 60) return `${seconds}s`;
	if (seconds < 3600) return `${Math.round(seconds / 60)}m`;
	const hours = seconds / 3600;
	return `${hours.toFixed(hours < 10 ? 1 : 0)}h`;
}

/// How much the migrations added. Sizes are what makes a duration comparable
/// across groups, and growth is what catches a heavy backfill.
function formatGrowth(before: number, after: number) {
	const added = after - before;
	if (added <= 0) return "none";
	const percent = before > 0 ? Math.round((added / before) * 100) : null;
	return percent === null ? formatBytes(added) : `+${formatBytes(added)} (${percent}%)`;
}

function formatBytes(bytes: number) {
	const units = ["B", "kB", "MB", "GB", "TB"];
	let value = bytes;
	let unit = 0;
	while (value >= 1000 && unit < units.length - 1) {
		value /= 1000;
		unit += 1;
	}
	return `${value.toFixed(value < 10 && unit > 0 ? 1 : 0)}${units[unit]}`;
}

function TestRow({
	row,
	server,
}: {
	row: GroupVerdict;
	server: ServerInfo | undefined;
}) {
	const [open, setOpen] = useState(false);
	const timings = row.latest?.timings ?? [];
	const expandable = timings.length > 0;
	return (
		<>
			<TableRow
				data-testid="migration-test-row"
				sx={expandable ? { "& > *": { borderBottom: "unset" } } : undefined}
			>
				<TableCell padding="checkbox">
					{expandable && (
						<IconButton
							size="small"
							aria-label={open ? "Hide migrations" : "Show migrations"}
							onClick={() => setOpen((o) => !o)}
						>
							{open ? <KeyboardArrowUpIcon /> : <KeyboardArrowDownIcon />}
						</IconButton>
					)}
				</TableCell>
				<TableCell>
					<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
						<MuiLink
							component={RouterLink}
							to={`/fleet/applications/${row.server_id}`}
							underline="hover"
							color="text.primary"
						>
							{server?.name ?? row.server_id}
						</MuiLink>
						{server?.rank && <ServerRankChip rank={server.rank} />}
					</Stack>
				</TableCell>
				<TableCell>{row.target_version}</TableCell>
				<TableCell>
					<VerdictChip
						verdict={row.verdict}
						failedMigration={row.latest?.failed_migration ?? null}
					/>
				</TableCell>
				<TableCell>
					{row.latest ? formatDuration(row.latest.total_elapsed) : "—"}
				</TableCell>
				<TableCell>
					{row.latest
						? formatGrowth(
								row.latest.data_bytes_before,
								row.latest.data_bytes_after,
							)
						: "—"}
				</TableCell>
				<TableCell>
					{row.latest ? (
						<Tooltip title={`snapshot ${row.latest.snapshot_id ?? "unknown"}`}>
							<Box component="span">
								<TimeAgo timestamp={row.latest.reported_at} />
							</Box>
						</Tooltip>
					) : (
						"—"
					)}
				</TableCell>
			</TableRow>
			{expandable && (
				<TableRow>
					<TableCell sx={{ py: 0 }} colSpan={7}>
						<Collapse in={open} timeout="auto" unmountOnExit>
							<Table size="small" sx={{ my: 1 }} data-testid="migration-timings">
								<TableBody>
									{timings.map((timing) => (
										<TableRow key={timing.ordinal}>
											<TableCell>{timing.name}</TableCell>
											<TableCell>{formatDuration(timing.elapsed)}</TableCell>
											<TableCell>
												{timing.name === row.latest?.failed_migration && (
													<Chip size="small" color="warning" label="failed" />
												)}
											</TableCell>
										</TableRow>
									))}
								</TableBody>
							</Table>
						</Collapse>
					</TableCell>
				</TableRow>
			)}
		</>
	);
}
