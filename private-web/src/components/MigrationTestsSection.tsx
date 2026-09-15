import {
	Alert,
	Box,
	Chip,
	LinearProgress,
	Link as MuiLink,
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
import { Link as RouterLink, useSearchParams } from "react-router-dom";
import { useApi } from "../api";
import { errorPreview } from "../lib/errorText";
import { formatDuration } from "../lib/migrationTests";
import type { ApiResponse, ServerInfo } from "../types";

type GroupVerdict = ApiResponse<"migration_tests", "for_group">[number];
import MigrationRunDialog from "./MigrationRunDialog";
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
	// Which run is open lives in the URL, so a link to one server's migrations
	// opens on the migrations rather than on the group.
	const [params, setParams] = useSearchParams();
	const openFor = params.get("migrations");
	const setOpenFor = (serverId: string | null) =>
		setParams(
			(prev) => {
				const next = new URLSearchParams(prev);
				if (serverId === null) next.delete("migrations");
				else next.set("migrations", serverId);
				return next;
			},
			{ replace: true },
		);
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
			<Paper
			id="migration-tests"
			variant="outlined"
			sx={{ p: 2, scrollMarginTop: 16 }}
			data-testid="migration-tests"
		>
				<SectionHeading />
				<Typography variant="body2" color="text.secondary">
					No upgrade plan is open for this group, so there is nothing to
					test against.
				</Typography>
			</Paper>
		);
	}

	return (
		<Paper
			id="migration-tests"
			variant="outlined"
			sx={{ p: 2, scrollMarginTop: 16 }}
			data-testid="migration-tests"
		>
			<SectionHeading />
			<Table size="small" sx={{ tableLayout: "fixed" }}>
				<TableHead>
					<TableRow>
						<TableCell sx={{ width: "32%" }}>Server</TableCell>
						<TableCell sx={{ width: "14%", whiteSpace: "nowrap" }}>
							Upgrading to
						</TableCell>
						<TableCell sx={{ width: "14%", whiteSpace: "nowrap" }}>
							Verdict
						</TableCell>
						<TableCell sx={{ width: "26%", whiteSpace: "nowrap" }}>
							Migrations took
						</TableCell>
						<TableCell sx={{ width: "14%", whiteSpace: "nowrap" }}>
							Tested
						</TableCell>
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
								open={openFor === row.server_id}
								onOpen={() => setOpenFor(row.server_id)}
								onClose={() => setOpenFor(null)}
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
	error,
}: {
	verdict: "passed" | "failed" | "nottested";
	failedMigration: string | null;
	error: string | null;
}) {
	if (verdict === "passed") {
		return <Chip size="small" color="success" label="passed" />;
	}
	if (verdict === "nottested") {
		return <Chip size="small" variant="outlined" label="not yet tested" />;
	}
	return (
		<Tooltip
			title={
				<>
					<Box>{failedMigration ?? "no migration named"}</Box>
					{error && (
						<Box sx={{ mt: 0.5, fontFamily: "monospace", whiteSpace: "pre-wrap" }}>
							{errorPreview(error)}
						</Box>
					)}
				</>
			}
		>
			<Chip size="small" color="warning" label="failed" />
		</Tooltip>
	);
}

function TestRow({
	row,
	server,
	open,
	onOpen,
	onClose,
}: {
	row: GroupVerdict;
	server: ServerInfo | undefined;
	open: boolean;
	onOpen: () => void;
	onClose: () => void;
}) {
	const timings = row.latest?.timings ?? [];
	return (
		<>
			<TableRow data-testid="migration-test-row">
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
				<TableCell sx={{ whiteSpace: "nowrap" }}>{row.target_version}</TableCell>
				<TableCell>
					<VerdictChip
						verdict={row.verdict}
						failedMigration={row.latest?.failed_migration ?? null}
						error={row.latest?.error ?? null}
					/>
				</TableCell>
				<TableCell sx={{ whiteSpace: "nowrap" }}>
					{row.latest == null ? (
						"—"
					) : timings.length === 0 ? (
						formatDuration(row.latest.total_elapsed)
					) : (
						<MuiLink
							component="button"
							type="button"
							underline="hover"
							aria-label="Show migrations"
							onClick={onOpen}
							color="text.primary"
							sx={{ font: "inherit", verticalAlign: "baseline" }}
						>
							{formatDuration(row.latest.total_elapsed)}
							<Box component="span" sx={{ color: "primary.main", ml: 0.5 }}>
								· {timings.length} migrations
							</Box>
						</MuiLink>
					)}
				</TableCell>
				<TableCell sx={{ whiteSpace: "nowrap" }}>
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
			{row.latest && (
				<MigrationRunDialog
					open={open}
					onClose={onClose}
					serverName={server?.name ?? row.server_id}
					rank={server?.rank}
					targetVersion={row.target_version}
					latest={row.latest}
				/>
			)}
		</>
	);
}
