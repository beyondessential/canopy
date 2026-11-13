import ChevronRightIcon from "@mui/icons-material/ChevronRight";
import ExpandMoreIcon from "@mui/icons-material/ExpandMore";
import {
	Alert,
	Box,
	Button,
	Chip,
	Collapse,
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
import { useEffect, useRef, useState } from "react";
import { Link as RouterLink, useSearchParams } from "react-router-dom";
import { useApi, useApiAction } from "../api";
import { errorPreview } from "../lib/errorText";
import { formatDuration } from "../lib/migrationTests";
import {
	type ApiResponse,
	type ServerInfo,
	type ServerRank,
	applicationName,
	serverSortKey,
} from "../types";
import { GradedAction } from "./GradedAction";
import MigrationRunDialog from "./MigrationRunDialog";
import ServerRankChip from "./ServerRankChip";
import TimeAgo from "./TimeAgo";

type ServerVerdict = ApiResponse<"migration_tests", "for_group">[number];

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
	const nameOf = (id: string) => serverName(byId.get(id));
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
	const [tick, setTick] = useState(0);
	const verdicts = useApi(
		"migration_tests",
		"for_group",
		{ group_id: groupId },
		[groupId, tick],
	);

	// The section mounts after the browser has already tried the hash.
	const scrolled = useRef(false);
	useEffect(() => {
		const section = document.getElementById("migration-tests");
		if (scrolled.current || !section) return;
		if (window.location.hash !== "#migration-tests") return;
		scrolled.current = true;
		section.scrollIntoView();
	}, [verdicts.status]);

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

	const tested = verdicts.data.filter((row) => row.set_up);
	const others = verdicts.data.filter((row) => !row.set_up);
	const targets = new Set(tested.map((row) => row.target_version));
	const target = targets.size === 1 ? tested[0].target_version : null;

	return (
		<Paper
			id="migration-tests"
			variant="outlined"
			sx={{ p: 2, scrollMarginTop: 16 }}
			data-testid="migration-tests"
		>
			<SectionHeading target={target} />
			{tested.length > 0 && (
				<Box sx={{ overflowX: "auto" }}>
					<Table size="small" sx={{ tableLayout: "fixed", minWidth: 640 }}>
						<TableHead>
							<TableRow>
								<TableCell sx={{ width: "32%" }}>Server</TableCell>
								{!target && (
									<TableCell sx={{ width: "14%", whiteSpace: "nowrap" }}>
										Upgrading to
									</TableCell>
								)}
								<TableCell sx={{ width: "14%", whiteSpace: "nowrap" }}>
									Verdict
								</TableCell>
								<TableCell sx={{ width: "22%", whiteSpace: "nowrap" }}>
									Migrations took
								</TableCell>
								<TableCell sx={{ width: "12%", whiteSpace: "nowrap" }}>
									Tested
								</TableCell>
								<TableCell sx={{ width: "16%" }} />
							</TableRow>
						</TableHead>
						<TableBody>
							{[...tested]
								.sort(
									(a, b) =>
										serverSortKey(byId.get(a.server_id) ?? {}) -
											serverSortKey(byId.get(b.server_id) ?? {}) ||
										VERDICT_ORDER[a.verdict] - VERDICT_ORDER[b.verdict] ||
										nameOf(a.server_id).localeCompare(nameOf(b.server_id)),
								)
								.map((row) => (
									<TestRow
										key={row.server_id}
										row={row}
										groupId={groupId}
										server={byId.get(row.server_id)}
										showTarget={!target}
										open={openFor === row.server_id}
										onOpen={() => setOpenFor(row.server_id)}
										onClose={() => setOpenFor(null)}
										onRequested={() => setTick((t) => t + 1)}
									/>
								))}
						</TableBody>
					</Table>
				</Box>
			)}
			{others.length > 0 && (
				<NotSetUp
					servers={others
						.map((row) => byId.get(row.server_id))
						.filter((server): server is ServerInfo => server !== undefined)
						.sort(
							(a, b) =>
								serverSortKey(a) - serverSortKey(b) ||
								serverName(a).localeCompare(serverName(b)),
						)}
					others={tested.length > 0}
				/>
			)}
		</Paper>
	);
}

function NotSetUp({
	servers,
	others,
}: {
	servers: ServerInfo[];
	others: boolean;
}) {
	const [open, setOpen] = useState(false);
	const count = servers.length;
	return (
		<Box data-testid="migration-tests-not-set-up" sx={{ pt: 1.5 }}>
			<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
				{open ? (
					<ExpandMoreIcon fontSize="small" color="action" />
				) : (
					<ChevronRightIcon fontSize="small" color="action" />
				)}
				<Typography variant="body2" color="text.secondary">
					{count} {others ? "other " : ""}
					{count === 1 ? "server is" : "servers are"} not set up for testing
				</Typography>
				<MuiLink
					component="button"
					type="button"
					variant="body2"
					aria-expanded={open}
					onClick={() => setOpen((shown) => !shown)}
				>
					{open ? "Hide" : "Show"}
				</MuiLink>
			</Stack>
			<Collapse in={open}>
				<Stack spacing={0.75} sx={{ pt: 1, pl: 3.5 }}>
					{servers.map((server) => (
						<Stack
							key={server.id}
							direction="row"
							spacing={1}
							sx={{ alignItems: "center" }}
							data-testid="migration-test-not-set-up"
						>
							<MuiLink
								component={RouterLink}
								to={`/fleet/applications/${server.id}`}
								underline="hover"
								color="text.primary"
								variant="body2"
							>
								{serverName(server)}
							</MuiLink>
							{server.rank && <ServerRankChip rank={server.rank} />}
						</Stack>
					))}
				</Stack>
			</Collapse>
		</Box>
	);
}

/// Problems first: a passing server is the row an operator scrolls past.
const VERDICT_ORDER: Record<string, number> = {
	failed: 0,
	nottested: 1,
	passed: 2,
};

function SectionHeading({ target }: { target?: string | null }) {
	return (
		<Stack direction="row" spacing={1} sx={{ mb: 1, alignItems: "baseline" }}>
			<Typography variant="h6" component="h2">
				Pre-upgrade migration tests
			</Typography>
			<Typography variant="body2" color="text.secondary">
				{target
					? `to ${target}`
					: "run against a restore of each server's own data"}
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

/// Every verdict is for one of the group's own applications, so the lookup
/// only misses on a group that changed under the page.
function serverName(server: ServerInfo | undefined): string {
	return server ? applicationName(server) : "Unknown application";
}

function TestRow({
	row,
	groupId,
	server,
	showTarget,
	open,
	onOpen,
	onClose,
	onRequested,
}: {
	row: ServerVerdict;
	groupId: string;
	server: ServerInfo | undefined;
	showTarget: boolean;
	open: boolean;
	onOpen: () => void;
	onClose: () => void;
	onRequested: () => void;
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
							{serverName(server)}
						</MuiLink>
						{server?.rank && <ServerRankChip rank={server.rank} />}
					</Stack>
				</TableCell>
				{showTarget && (
					<TableCell sx={{ whiteSpace: "nowrap" }}>{row.target_version}</TableCell>
				)}
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
								· {timings.length} {timings.length === 1 ? "migration" : "migrations"}
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
				<TableCell align="right">
					{server?.rank && (
						<RequestTest
							groupId={groupId}
							rank={server.rank}
							name={serverName(server)}
							tested={row.latest != null}
							request={row.test_request ?? null}
							onRequested={onRequested}
						/>
					)}
				</TableCell>
			</TableRow>
			{row.latest && (
				<MigrationRunDialog
					open={open}
					onClose={onClose}
					serverName={serverName(server)}
					rank={server?.rank}
					targetVersion={row.target_version}
					latest={row.latest}
				/>
			)}
		</>
	);
}

/// The ask covers every server of the environment set up for testing.
// spec: RST#dispatching-a-migration-test
function RequestTest({
	groupId,
	rank,
	name,
	tested,
	request,
	onRequested,
}: {
	groupId: string;
	rank: ServerRank;
	name: string;
	tested: boolean;
	request: ServerVerdict["test_request"];
	onRequested: () => void;
}) {
	const ask = useApiAction("migration_tests", "request");
	if (request) {
		return (
			<Tooltip
				title={
					<>
						Asked for <TimeAgo timestamp={request.requested_at} />
						{request.requested_by ? ` by ${request.requested_by}` : ""}
					</>
				}
			>
				<Chip
					size="small"
					color="info"
					variant="outlined"
					label="test requested"
					data-testid="migration-test-requested"
				/>
			</Tooltip>
		);
	}
	return (
		<Stack spacing={0.5} sx={{ alignItems: "flex-end" }}>
			<GradedAction
				calls="migration_tests/request"
				action={`Test migrations for ${name}`}
			>
				<Button
					size="small"
					variant="outlined"
					disabled={ask.pending}
					onClick={async () => {
						try {
							await ask.call({ group_id: groupId, rank });
							onRequested();
						} catch {
							/* ask.error is shown beneath the button */
						}
					}}
				>
					{tested ? "Test again" : "Test now"}
				</Button>
			</GradedAction>
			{ask.error && (
				<Typography
					variant="caption"
					color="error"
					data-testid="migration-test-request-error"
				>
					{ask.error.message}
				</Typography>
			)}
		</Stack>
	);
}
