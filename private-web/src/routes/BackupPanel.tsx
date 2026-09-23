import {
	Alert,
	Box,
	Button,
	Chip,
	CircularProgress,
	Collapse,
	Dialog,
	DialogActions,
	DialogContent,
	DialogContentText,
	DialogTitle,
	FormControlLabel,
	IconButton,
	LinearProgress,
	Link as MuiLink,
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
import { Fragment, useEffect, useState } from "react";
import BackupIcon from "@mui/icons-material/Backup";
import RestoreIcon from "@mui/icons-material/SettingsBackupRestore";
import DeleteIcon from "@mui/icons-material/Delete";
import EditIcon from "@mui/icons-material/Edit";
import KeyboardArrowDownIcon from "@mui/icons-material/KeyboardArrowDown";
import KeyboardArrowUpIcon from "@mui/icons-material/KeyboardArrowUp";
import RefreshIcon from "@mui/icons-material/Refresh";
import { Link as RouterLink, useParams } from "react-router-dom";
import { useApi, useApiAction } from "../api";
import { useReloadInterval } from "../hooks/useReloadInterval";
import { useIsAdmin } from "../hooks/useIsAdmin";
import { humanDuration, humanSeconds } from "../lib/humanDuration";
import { formatBytes } from "../lib/formatBytes";
import {
	estimatedBucketCostTooltip,
	s3EgressRateForRegion,
} from "../lib/s3Pricing";
import { usePageTitle } from "../hooks/usePageTitle";
import { GradedAction } from "../components/GradedAction";
import TimeAgo from "../components/TimeAgo";
import { LatestSnapshot, SnapshotId } from "../components/SnapshotId";
import { BackupProcessingChip } from "../components/BackupProcessingChip";
import { BackupLiveProgress } from "../components/BackupLiveProgress";
import RestoreReplicasSection from "../components/RestoreReplicasSection";
import RunThroughputChart from "../components/RunThroughputChart";
import {
	BACKUP_STATUS_HELP,
	BACKUP_STATUS_INTENT,
	BACKUP_STATUS_LABEL,
	applicationName,
	type BackupConfigStatus,
	type BackupConfigView,
	type BackupMaintenanceRun,
	groupServersByRank,
	type LiveProgress,
	type RankedMachine,
	rankMachines,
	type RecentRun,
} from "../types";

export default function BackupPanel() {
	const { id = "" } = useParams<{ id: string }>();
	const isAdmin = useIsAdmin() === true;
	const config = useApi(
		"backups",
		"get",
		{ server_group_id: id },
		[id],
	);
	// Poll while provisioning so the operator sees the init Job land.
	const provisioning =
		config.status === "ok" && config.data?.status === "provisioning";
	const tick = useReloadInterval(provisioning ? 5_000 : 30_000, "canopy-data-changed");
	const configForTick = useApi(
		"backups",
		"get",
		{ server_group_id: id },
		[id, tick],
	);
	const group = useApi(
		"fleet/groups",
		"get",
		{ server_group_id: id },
		[id],
	);
	const groupName =
		group.status === "ok" ? group.data.group.name : undefined;
	usePageTitle(groupName ? `Backups · ${groupName}` : "Backups");

	const data =
		configForTick.status === "ok"
			? configForTick.data
			: config.status === "ok"
				? config.data
				: undefined;

	if (config.status === "loading" || config.status === "idle") {
		return <LinearProgress />;
	}
	if (config.status === "error") {
		return <Alert severity="error">{config.error.message}</Alert>;
	}

	if (data == null) {
		return (
			<Stack spacing={2}>
				<Box>
					<MuiLink
						component={RouterLink}
						to={`/fleet/groups/${id}`}
						variant="body2"
						underline="hover"
					>
						‹ Back to {groupName ?? "group"}
					</MuiLink>
					<Typography variant="h4" component="h1">
						{groupName ? `${groupName} backups` : "Backups"}
					</Typography>
				</Box>
				<Alert severity="info">Backups not set up for this group.</Alert>
				{isAdmin && (
					<Box>
						<GradedAction opens={["backups/create_shared", "backups/create"]}>
							<Button
								component={RouterLink}
								to={`/fleet/groups/${id}/backups/config`}
								variant="contained"
								startIcon={<BackupIcon />}
							>
								Set up backups
							</Button>
						</GradedAction>
					</Box>
				)}
			</Stack>
		);
	}

	// A backup is taken of a box, so this page counts boxes: capabilities,
	// requests and restore windows all key off a machine, and two workloads
	// sharing a host share one row here.
	// spec: BKO#status
	const members =
		group.status === "ok"
			? rankMachines(group.data.machines, group.data.applications)
			: [];
	const status = data.status as BackupConfigStatus;

	return (
		<Stack spacing={3}>
			<Box>
				<MuiLink
					component={RouterLink}
					to={`/fleet/groups/${id}`}
					variant="body2"
					underline="hover"
				>
					‹ Back to {groupName ?? "group"}
				</MuiLink>
				<Stack
					direction="row"
					spacing={2}
					sx={{
						alignItems: "center",
						justifyContent: "space-between",
						mt: 0.5,
					}}
				>
					<Stack direction="row" spacing={1.5} sx={{ alignItems: "center" }}>
						<Typography variant="h4" component="h1">
							{groupName ? `${groupName} backups` : "Backups"}
						</Typography>
						<Chip
							label={BACKUP_STATUS_LABEL[status]}
							color={BACKUP_STATUS_INTENT[status]}
							size="small"
						/>
					</Stack>
					{isAdmin && (
						<Stack direction="row" spacing={1}>
							<GradedAction opens="backups/update">
								<Button
									component={RouterLink}
									to={`/fleet/groups/${id}/backups/config`}
									variant="outlined"
									startIcon={<EditIcon />}
								>
									Edit config
								</Button>
							</GradedAction>
							<DeleteConfigButton groupId={id} onDeleted={configForTick.reload} />
						</Stack>
					)}
				</Stack>
			</Box>

			{status !== "ready" && (
				<Alert severity={BACKUP_STATUS_INTENT[status]}>
					{BACKUP_STATUS_HELP[status]}
				</Alert>
			)}

			{status === "ready" ? (
				<Box
					sx={{
						display: "grid",
						gap: 2,
						gridTemplateColumns: { xs: "1fr", md: "1fr 1fr" },
						alignItems: "start",
					}}
				>
					<ConfigSummary config={data} />
					<RepoStatsPanel groupId={id} region={data.region} />
				</Box>
			) : (
				<ConfigSummary config={data} />
			)}

			{status === "provisioning" && (
				<ProvisioningCard
					config={data}
					isAdmin={isAdmin}
					onRetried={configForTick.reload}
				/>
			)}

			{status === "ready" && (
				<>
					<ServersPanel groupId={id} members={members} isAdmin={isAdmin} />
					<SchedulesPanel groupId={id} isAdmin={isAdmin} />
					<MaintenancePanel
						groupId={id}
						config={data}
						isAdmin={isAdmin}
						onChanged={configForTick.reload}
					/>
					<RecentRunsPanel groupId={id} members={members} />
					<RestoreReplicasSection groupId={id} isAdmin={isAdmin} />
				</>
			)}
		</Stack>
	);
}

/// Decommission a group's backup config (admin). Deletes the config row + the
/// Canopy-owned passphrase Secret; the bucket and its object-locked backups are
/// untouched. Confirms first since it stops credential issuance for the group.
function DeleteConfigButton({
	groupId,
	onDeleted,
}: {
	groupId: string;
	onDeleted: () => void;
}) {
	const [open, setOpen] = useState(false);
	const del = useApiAction("backups", "delete");

	const onConfirm = async () => {
		try {
			await del.call({ server_group_id: groupId });
			setOpen(false);
			onDeleted();
		} catch {
			/* surfaced via del.error */
		}
	};

	return (
		<>
			<GradedAction calls="backups/delete">
				<Button
					variant="outlined"
					startIcon={<DeleteIcon />}
					onClick={() => setOpen(true)}
				>
					Delete
				</Button>
			</GradedAction>
			<Dialog open={open} onClose={() => !del.pending && setOpen(false)}>
				<DialogTitle>Delete backup config?</DialogTitle>
				<DialogContent>
					<DialogContentText>
						This stops credential issuance for the group and deletes the
						Canopy-owned repository passphrase. The bucket and its (object-locked)
						backups are left untouched — you can set backups up again afterwards.
					</DialogContentText>
					{del.error && (
						<Alert severity="error" sx={{ mt: 2 }}>
							{del.error.message}
						</Alert>
					)}
				</DialogContent>
				<DialogActions>
					<Button onClick={() => setOpen(false)} disabled={del.pending}>
						Cancel
					</Button>
					<GradedAction calls="backups/delete">
						<Button
							variant="contained"
							onClick={onConfirm}
							disabled={del.pending}
						>
							{del.pending ? "Deleting…" : "Delete"}
						</Button>
					</GradedAction>
				</DialogActions>
			</Dialog>
		</>
	);
}

function ConfigSummary({ config }: { config: BackupConfigView }) {
	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<Stack spacing={0.5}>
				<Typography variant="body2">
					<strong>Bucket:</strong> {config.bucket}
					{config.prefix ? `/${config.prefix}` : ""}
				</Typography>
				<Typography variant="body2">
					<strong>Region:</strong> {config.region ?? "default"}
				</Typography>
				<Typography variant="body2">
					<strong>Placement:</strong>{" "}
					{config.placement === "shared"
						? "canopy-created bucket in the shared account"
						: "existing bucket in a dedicated account"}
				</Typography>
			</Stack>
		</Paper>
	);
}

/// Per backup type, the group's effective schedule + retention (inherited from
/// the canopy-wide default, or a per-group override). Admins can override a type
/// or reset it back to the default.
function SchedulesPanel({
	groupId,
	isAdmin,
}: {
	groupId: string;
	isAdmin: boolean;
}) {
	const schedules = useApi(
		"backups",
		"group_schedules",
		{ server_group_id: groupId },
		[groupId],
	);

	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<Typography variant="h6" component="h2" gutterBottom>
				Schedule &amp; retention
			</Typography>
			{schedules.status === "loading" || schedules.status === "idle" ? (
				<LinearProgress />
			) : schedules.status === "error" ? (
				<Alert severity="error">{schedules.error.message}</Alert>
			) : schedules.data.length === 0 ? (
				<Alert severity="info">
					No backup types registered for this group's servers yet. Each type a
					server advertises appears here — scheduled or manual-only — and
					inherits the canopy-wide default retention until overridden.
				</Alert>
			) : (
				<Stack spacing={2}>
					{schedules.data.map((s) => (
						<TypeSchedule
							key={s.type}
							groupId={groupId}
							schedule={s}
							isAdmin={isAdmin}
							onChanged={schedules.reload}
						/>
					))}
				</Stack>
			)}
		</Paper>
	);
}

type GroupTypeSchedule = {
	type: string;
	effective_interval: number | null;
	effective_retention: {
		keep_latest: number;
		keep_daily: number;
		keep_weekly: number;
		keep_monthly: number;
		keep_annual: number;
	};
	has_override: boolean;
	allow_below_floor: boolean;
	next_run_at: string | null;
};

function TypeSchedule({
	groupId,
	schedule,
	isAdmin,
	onChanged,
}: {
	groupId: string;
	schedule: GroupTypeSchedule;
	isAdmin: boolean;
	onChanged: () => void;
}) {
	const [editing, setEditing] = useState(false);
	const r = schedule.effective_retention;

	return (
		<Box sx={{ borderTop: 1, borderColor: "divider", pt: 1.5 }}>
			<Stack
				direction="row"
				spacing={1}
				sx={{ alignItems: "center", flexWrap: "wrap" }}
			>
				<Typography sx={{ fontFamily: "monospace" }}>{schedule.type}</Typography>
				<Chip
					size="small"
					label={schedule.has_override ? "Override" : "Inherited default"}
					color={schedule.has_override ? "secondary" : "default"}
					variant={schedule.has_override ? "filled" : "outlined"}
				/>
				{schedule.allow_below_floor && (
					<Chip size="small" color="error" label="below floor" />
				)}
				<Box sx={{ flex: 1 }} />
				{isAdmin && !editing && (
					<GradedAction calls="backups/set_schedule">
						<Button size="small" onClick={() => setEditing(true)}>
							{schedule.has_override ? "Edit override" : "Override"}
						</Button>
					</GradedAction>
				)}
			</Stack>
			<Typography variant="body2" color="text.secondary">
				{schedule.effective_interval != null
					? `Every ${humanSeconds(schedule.effective_interval)}`
					: "Manual only (no scheduled interval)"}{" "}
				· retention latest {r.keep_latest}, daily {r.keep_daily}, weekly{" "}
				{r.keep_weekly}, monthly {r.keep_monthly}, annual {r.keep_annual}
			</Typography>
			{editing && (
				<OverrideEditor
					groupId={groupId}
					schedule={schedule}
					onDone={() => {
						setEditing(false);
						onChanged();
					}}
					onCancel={() => setEditing(false)}
				/>
			)}
		</Box>
	);
}

const RETENTION_FIELDS: Array<{
	key: keyof GroupTypeSchedule["effective_retention"];
	label: string;
	floor?: number;
}> = [
	{ key: "keep_latest", label: "Latest" },
	{ key: "keep_daily", label: "Daily", floor: 7 },
	{ key: "keep_weekly", label: "Weekly", floor: 4 },
	{ key: "keep_monthly", label: "Monthly", floor: 6 },
	{ key: "keep_annual", label: "Annual" },
];

function OverrideEditor({
	groupId,
	schedule,
	onDone,
	onCancel,
}: {
	groupId: string;
	schedule: GroupTypeSchedule;
	onDone: () => void;
	onCancel: () => void;
}) {
	const setSchedule = useApiAction("backups", "set_schedule");
	const clearSchedule = useApiAction("backups", "clear_schedule");
	const [scheduled, setScheduled] = useState(
		schedule.effective_interval != null,
	);
	const [hours, setHours] = useState(
		schedule.effective_interval != null
			? String(Math.max(1, Math.round(schedule.effective_interval / 3600)))
			: "6",
	);
	const [retention, setRetention] = useState(schedule.effective_retention);
	const [allowBelowFloor, setAllowBelowFloor] = useState(
		schedule.allow_below_floor,
	);

	const floorError = allowBelowFloor
		? []
		: RETENTION_FIELDS.filter(
				(f) => f.floor != null && retention[f.key] < f.floor,
			).map((f) => `${f.label} must be ≥ ${f.floor}`);

	const save = async () => {
		await setSchedule.call({
			server_group_id: groupId,
			type: schedule.type,
			expected_interval: scheduled ? Math.max(1, Number(hours)) * 3600 : null,
			retention,
			allow_below_floor: allowBelowFloor,
		});
		onDone();
	};
	const reset = async () => {
		await clearSchedule.call({ server_group_id: groupId, type: schedule.type });
		onDone();
	};

	const pending = setSchedule.pending || clearSchedule.pending;
	const error = setSchedule.error || clearSchedule.error;

	return (
		<Stack spacing={1.5} sx={{ mt: 1.5 }}>
			<FormControlLabel
				control={
					<Switch
						checked={scheduled}
						onChange={(e) => setScheduled(e.target.checked)}
						disabled={pending}
					/>
				}
				label={scheduled ? "Scheduled" : "Manual only"}
			/>
			{scheduled && (
				<TextField
					label="Back up every (hours)"
					type="number"
					size="small"
					value={hours}
					onChange={(e) => setHours(e.target.value)}
					disabled={pending}
					slotProps={{ htmlInput: { min: 1, step: 1 } }}
					sx={{ width: 200 }}
				/>
			)}
			<Stack direction={{ xs: "column", md: "row" }} spacing={1}>
				{RETENTION_FIELDS.map((f) => (
					<TextField
						key={f.key}
						label={f.label}
						type="number"
						size="small"
						value={retention[f.key]}
						onChange={(e) =>
							setRetention({ ...retention, [f.key]: Number(e.target.value) })
						}
						disabled={pending}
						error={!allowBelowFloor && f.floor != null && retention[f.key] < f.floor}
						helperText={
							!allowBelowFloor && f.floor != null ? `≥ ${f.floor}` : undefined
						}
						slotProps={{
							htmlInput: { min: allowBelowFloor ? 0 : (f.floor ?? 0), step: 1 },
						}}
						sx={{ width: 100 }}
					/>
				))}
			</Stack>
			<FormControlLabel
				control={
					<Switch
						checked={allowBelowFloor}
						onChange={(e) => setAllowBelowFloor(e.target.checked)}
						disabled={pending}
						color="error"
					/>
				}
				label="Allow retention below the org minimum (dangerous)"
			/>
			{allowBelowFloor && (
				<Alert severity="warning">
					Snapshots of this type may be pruned below the org-minimum retention.
					Only use this for data you are not authorised to keep longer.
				</Alert>
			)}
			{floorError.length > 0 && (
				<Alert severity="warning">{floorError.join("; ")}</Alert>
			)}
			{error && <Alert severity="error">{error.message}</Alert>}
			<Stack direction="row" spacing={1}>
				<GradedAction calls="backups/set_schedule">
					<Button
						variant="contained"
						size="small"
						onClick={save}
						disabled={pending || floorError.length > 0}
					>
						{pending ? "Saving…" : "Save override"}
					</Button>
				</GradedAction>
				{schedule.has_override && (
					<GradedAction calls="backups/clear_schedule">
						<Button
							size="small"
							onClick={reset}
							disabled={pending}
						>
							Reset to default
						</Button>
					</GradedAction>
				)}
				<Button size="small" onClick={onCancel} disabled={pending}>
					Cancel
				</Button>
			</Stack>
		</Stack>
	);
}

function ProvisioningCard({
	config,
	isAdmin,
	onRetried,
}: {
	config: BackupConfigView;
	isAdmin: boolean;
	onRetried: () => void;
}) {
	const retry = useApiAction("backups", "create_repo");
	const onRetry = async () => {
		try {
			await retry.call({ server_group_id: config.server_group_id });
			onRetried();
		} catch {
			/* surfaced via retry.error */
		}
	};
	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			{config.last_init_error ? (
				<Stack spacing={2}>
					<Alert severity="error">
						Repository creation failed: {config.last_init_error}
					</Alert>
					{isAdmin && (
						<Box>
							<GradedAction calls="backups/create_repo">
								<Button
									variant="contained"
									startIcon={<RefreshIcon />}
									onClick={onRetry}
									disabled={retry.pending}
								>
									{retry.pending ? "Retrying…" : "Retry repo creation"}
								</Button>
							</GradedAction>
						</Box>
					)}
					{retry.error && (
						<Alert severity="error">{retry.error.message}</Alert>
					)}
				</Stack>
			) : (
				<Stack direction="row" spacing={2} sx={{ alignItems: "center" }}>
					<CircularProgress size={20} />
					<Typography>Creating repository…</Typography>
				</Stack>
			)}
		</Paper>
	);
}

/// Human label for the machine a run came from. Falls back to the name of a
/// workload on it, then to a short id, and to "—" for runs with no machine.
function machineLabel(
	members: RankedMachine[],
	machineId: string | null | undefined,
): string {
	if (!machineId) return "—";
	const box = members.find((m) => m.machine.id === machineId);
	if (!box) return machineId.slice(0, 8);
	const [first] = box.applications;
	return (
		box.machine.name ||
		first?.name ||
		first?.display_host ||
		machineId.slice(0, 8)
	);
}

/// True when the run carries any of bestool's four S3 traffic tallies.
function hasS3Traffic(run: RecentRun): boolean {
	return (
		run.s3_sent_raw_bytes != null ||
		run.s3_sent_payload_bytes != null ||
		run.s3_received_raw_bytes != null ||
		run.s3_received_payload_bytes != null
	);
}

/// The outcome/status chip for a run row. A reported run shows success/failure;
/// an inferred row shows in-progress (creds still valid, no report yet) or
/// unknown (creds expired without a report — e.g. a manual restore, which bestool
/// doesn't report).
function RunOutcomeChip({ run }: { run: RecentRun }) {
	if (run.status === "reported") {
		return (
			<Chip
				size="small"
				label={run.outcome}
				color={run.outcome === "success" ? "success" : "error"}
			/>
		);
	}
	if (run.status === "in_progress") {
		// A run reporting progress can say when it was last heard from; one that
		// isn't falls back to what we've always been able to say — that credentials
		// were issued and no report has arrived.
		const live = run.progress;
		return (
			<Tooltip
				title={
					live ? (
						<>
							Last progress <TimeAgo timestamp={live.observed_at} />
							{live.current_path ? <> — working on {live.current_path}</> : null}
						</>
					) : (
						<>
							Credentials issued <TimeAgo timestamp={run.started_at ?? run.at} />;
							awaiting the run report. This client reports no progress.
						</>
					)
				}
			>
				<Chip
					size="small"
					color="info"
					variant="outlined"
					icon={<CircularProgress size={12} color="inherit" />}
					label="in progress"
				/>
			</Tooltip>
		);
	}
	return (
		<Tooltip title="Credentials were issued but no run outcome was ever reported.">
			<Chip size="small" variant="outlined" label="unknown" />
		</Tooltip>
	);
}

/// The run duration, measured Canopy-side as the interval from the run's first
/// credential issuance (its start) to its report — the real wall-clock time, not
/// a client-reported figure. In-flight rows show elapsed-so-far.
function RunDurationCell({ run }: { run: RecentRun }) {
	if (run.duration_seconds != null) {
		return (
			<Tooltip title="Time from first credential issuance to run report (measured by Canopy)">
				<span>{humanSeconds(run.duration_seconds)}</span>
			</Tooltip>
		);
	}
	if (run.status === "in_progress" && run.started_at) {
		const elapsed = Math.round(
			(Date.now() - Date.parse(run.started_at)) / 1000,
		);
		return (
			<Tooltip title="Elapsed since credentials were issued; the run hasn't reported yet">
				<span>{humanSeconds(elapsed)} so far</span>
			</Tooltip>
		);
	}
	return <>—</>;
}

/// Transfer figures for a run still in flight: what it has moved so far, against
/// the total it expects, with its current rate.
///
/// Every figure is independently optional, and an absent one reads as unknown
/// rather than as zero — a client that measures nothing must not look like a run
/// that is moving nothing.
function LiveTransferCell({ live }: { live: LiveProgress }) {
	const moved = live.bytes_uploaded;
	const expected = live.bytes_estimated;
	const pct =
		moved != null && expected != null && expected > 0
			? Math.min(100, (moved / expected) * 100)
			: null;
	if (moved == null && live.bytes_per_second == null) {
		return (
			<Tooltip title="This run reports no transfer figures.">
				<span>—</span>
			</Tooltip>
		);
	}
	return (
		<Stack spacing={0.25} sx={{ minWidth: 120 }} data-testid="live-transfer">
			<Typography variant="body2" component="span">
				{moved == null ? "—" : formatBytes(moved)}
				{expected != null && ` / ~${formatBytes(expected)}`}
			</Typography>
			{pct != null && (
				<Tooltip title={`${Math.round(pct)}% of the run's own estimate, which it may revise upward`}>
					<LinearProgress
						variant="determinate"
						value={pct}
						sx={{ height: 4, borderRadius: 2 }}
					/>
				</Tooltip>
			)}
			<Typography variant="caption" color="text.secondary">
				{live.bytes_per_second == null
					? "rate unknown"
					: `${formatBytes(live.bytes_per_second)}/s`}
			</Typography>
		</Stack>
	);
}

/// When the run froze the data it captured, shown under the row's time. For a long
/// backup this is materially earlier than the report, and it is the figure that
/// says how old the data actually is.
function SnapshotTakenCaption({ run }: { run: RecentRun }) {
	if (!run.snapshot_taken_at) return null;
	return (
		<Tooltip title="When the run froze the data it backed up — the point in time this backup represents, as opposed to when its upload finished.">
			<Typography
				variant="caption"
				color="text.secondary"
				sx={{ display: "block" }}
				data-testid="snapshot-taken"
			>
				data from <TimeAgo timestamp={run.snapshot_taken_at} />
			</Typography>
		</Tooltip>
	);
}

/// One row of the recent-runs table. Runs with an error, reported S3 traffic, or
/// live progress get an expand toggle that reveals the detail in a collapsible
/// sub-row.
function RunRow({ run, members }: { run: RecentRun; members: RankedMachine[] }) {
	const [open, setOpen] = useState(false);
	const hasError = Boolean(run.error);
	const hasS3 = hasS3Traffic(run);
	const live = run.progress;
	// A run with a correlation id has a series worth charting even once finished,
	// for as long as that series is retained.
	const chartable = run.run_id != null;
	const expandable = hasError || hasS3 || live != null || chartable;
	// The snapshot's logical tree size, from whichever source reported it:
	// bestool's own accounting, or (failing that) canopy's repo inspection.
	// Both describe the same quantity — kopia doesn't re-upload bits the
	// server already has, so this is usually larger than what actually went
	// over the wire. For a restore this is the size of the snapshot the run
	// restored from, resolved server-side from the backup run that produced it
	// (by snapshot id) or inspection's backfill.
	const fromInspect =
		run.bytes_uploaded == null && run.snapshot_logical_bytes != null;
	const snapshotSize = run.bytes_uploaded ?? run.snapshot_logical_bytes ?? null;
	// Bytes the run actually moved over the wire, from the SigV4 proxy's
	// tally: payload sent for a backup (upload), payload received for a
	// restore (download). Absent for runs the proxy didn't front.
	const transfer =
		run.purpose === "restore"
			? (run.s3_received_payload_bytes ?? null)
			: (run.s3_sent_payload_bytes ?? null);
	return (
		<>
			<TableRow sx={expandable ? { "& > *": { borderBottom: "unset" } } : undefined}>
				<TableCell padding="checkbox">
					{expandable && (
						<IconButton
							size="small"
							aria-label={open ? "Hide details" : "Show details"}
							onClick={() => setOpen((o) => !o)}
						>
							{open ? <KeyboardArrowUpIcon /> : <KeyboardArrowDownIcon />}
						</IconButton>
					)}
				</TableCell>
				<TableCell>
					<TimeAgo timestamp={run.at} />
					<SnapshotTakenCaption run={run} />
				</TableCell>
				<TableCell>{machineLabel(members, run.machine_id)}</TableCell>
				<TableCell>{run.type}</TableCell>
				<TableCell>{run.purpose}</TableCell>
				<TableCell>
					<RunOutcomeChip run={run} />
				</TableCell>
				<TableCell>
					<RunDurationCell run={run} />
				</TableCell>
				<TableCell>
					{snapshotSize == null ? (
						"—"
					) : fromInspect ? (
						<Tooltip
							title={
								run.purpose === "restore"
									? "Size of the snapshot the restore used"
									: "From repo inspection (the device reported no size)"
							}
						>
							<span>{formatBytes(snapshotSize)}</span>
						</Tooltip>
					) : (
						formatBytes(snapshotSize)
					)}
				</TableCell>
				<TableCell>
					{live ? (
						<LiveTransferCell live={live} />
					) : transfer == null ? (
						"—"
					) : (
						formatBytes(transfer)
					)}
				</TableCell>
				<TableCell>
					<SnapshotId id={run.snapshot_id} />
				</TableCell>
			</TableRow>
			{expandable && (
				<TableRow>
					<TableCell colSpan={10} sx={{ py: 0, border: 0 }}>
						<Collapse in={open} timeout="auto" unmountOnExit>
							{hasError && (
								<Alert severity="error" variant="outlined" sx={{ my: 1 }}>
									<Typography
										component="pre"
										variant="body2"
										sx={{
											m: 0,
											fontFamily: "monospace",
											whiteSpace: "pre-wrap",
											wordBreak: "break-word",
										}}
									>
										{run.error}
									</Typography>
								</Alert>
							)}
							{hasS3 && <S3TrafficDetail run={run} />}
							{live && <LiveProgressDetail live={live} />}
							{/* Mounted only while open so an expanded row is the only one
							    that fetches its series. */}
							{open && chartable && run.run_id && (
								<RunProgressSection
									runId={run.run_id}
									live={run.status === "in_progress"}
								/>
							)}
						</Collapse>
					</TableCell>
				</TableRow>
			)}
		</>
	);
}

/// Engine counters for a run in flight, and the proxy's tally set against them.
///
/// The two describe the same upload from different vantage points, so they should
/// track closely; a visible gap is retries or protocol overhead, and worth seeing.
function LiveProgressDetail({ live }: { live: LiveProgress }) {
	const engine = live.bytes_uploaded;
	const proxy = live.s3_sent_payload_bytes;
	const raw = live.s3_sent_raw_bytes;
	const overheadPct =
		raw != null && proxy != null && proxy > 0
			? ((raw - proxy) / proxy) * 100
			: null;
	const divergencePct =
		engine != null && proxy != null && engine > 0
			? ((proxy - engine) / engine) * 100
			: null;
	return (
		<Stack spacing={0.5} sx={{ my: 1 }} data-testid="live-progress-detail">
			<Typography variant="subtitle2">In flight</Typography>
			<Stat label="Read" value={formatBytes(live.bytes_read)} />
			<Stat label="Processed" value={formatBytes(live.bytes_hashed)} />
			<Stat label="Already present" value={formatBytes(live.bytes_cached)} />
			{(live.files_done != null || live.files_estimated != null) && (
				<Stat
					label="Files"
					value={`${live.files_done ?? "—"} / ${live.files_estimated ?? "—"}`}
				/>
			)}
			{(live.errors != null || live.ignored_errors != null) && (
				<Stat
					label="Errors"
					value={`${live.errors ?? 0} (${live.ignored_errors ?? 0} ignored)`}
				/>
			)}
			<Stat
				label="Last heard"
				value={`${humanSeconds(live.seconds_since_observed)} ago`}
			/>

			<Typography variant="subtitle2" sx={{ pt: 0.5 }}>
				Engine vs proxy
			</Typography>
			<Stat label="Engine uploaded" value={formatBytes(engine)} />
			<Stat
				label="Proxy sent"
				value={
					proxy == null
						? formatBytes(proxy)
						: `${formatBytes(proxy)} payload${
								divergencePct == null
									? ""
									: ` (${divergencePct >= 0 ? "+" : ""}${divergencePct.toFixed(1)}% vs engine)`
							}`
				}
			/>
			<Stat
				label="Proxy raw"
				value={
					raw == null
						? formatBytes(raw)
						: `${formatBytes(raw)}${
								overheadPct == null ? "" : ` (+${overheadPct.toFixed(1)}% overhead)`
							}`
				}
			/>

			{live.extra != null && Object.keys(live.extra as object).length > 0 && (
				<RawEngineDetail extra={live.extra} />
			)}
		</Stack>
	);
}

/// Whatever the backup engine reported that Canopy doesn't model, shown verbatim.
/// Collapsed by default: useful when a run misbehaves, noise otherwise.
function RawEngineDetail({ extra }: { extra: unknown }) {
	const [open, setOpen] = useState(false);
	return (
		<Box>
			<Button size="small" onClick={() => setOpen((o) => !o)} sx={{ px: 0.5 }}>
				{open ? "Hide" : "Show"} raw engine data
			</Button>
			<Collapse in={open} timeout="auto" unmountOnExit>
				<Typography
					component="pre"
					variant="body2"
					data-testid="raw-engine-data"
					sx={{
						m: 0,
						fontFamily: "monospace",
						whiteSpace: "pre-wrap",
						wordBreak: "break-word",
					}}
				>
					{JSON.stringify(extra, null, 2)}
				</Typography>
			</Collapse>
		</Box>
	);
}

/// The run's rate over its life. Fetched per expanded row rather than with the
/// panel, since a series is only wanted when someone opens the row.
///
/// A run still in flight keeps extending its series, so poll while it does; a
/// finished run's series is fixed and fetched once.
function RunProgressSection({
	runId,
	live,
}: {
	runId: string;
	live: boolean;
}) {
	const tick = useReloadInterval(live ? 5_000 : 300_000, "canopy-data-changed");
	const series = useApi("backups", "run_progress", { run_id: runId }, [
		runId,
		live ? tick : 0,
	]);
	if (series.status === "loading" || series.status === "idle") {
		return <LinearProgress sx={{ my: 1 }} />;
	}
	if (series.status === "error") {
		return (
			<Alert severity="error" variant="outlined" sx={{ my: 1 }}>
				{series.error.message}
			</Alert>
		);
	}
	if (series.data.length === 0) return null;
	return (
		<Box sx={{ my: 1 }}>
			<Typography variant="subtitle2" gutterBottom>
				Upload rate
			</Typography>
			<RunThroughputChart series={series.data} />
		</Box>
	);
}

/// S3 traffic the proxy tallied during a run, shown in the expand row: raw is
/// the full HTTP message (incl. SigV4 chunk framing), payload the object data.
function S3TrafficDetail({ run }: { run: RecentRun }) {
	return (
		<Stack spacing={0.5} sx={{ my: 1 }}>
			<Typography variant="subtitle2">S3 traffic</Typography>
			<Stat
				label="Sent"
				value={`${formatBytes(run.s3_sent_payload_bytes)} payload / ${formatBytes(run.s3_sent_raw_bytes)} raw`}
			/>
			<Stat
				label="Received"
				value={`${formatBytes(run.s3_received_payload_bytes)} payload / ${formatBytes(run.s3_received_raw_bytes)} raw`}
			/>
		</Stack>
	);
}

/// Tooltip for the bucket-bytes figure: the estimated storage cost plus where
/// the number comes from. Unlike the kopia-reported stats above it (refreshed
/// by repo inspection), it's read from CloudWatch's daily BucketSizeBytes
/// metric once a day, so it can lag the other figures.
function bucketBytesTooltip(
	bucketBytes: number,
	measuredAt: string | null | undefined,
	region: string | null,
) {
	return (
		<>
			{estimatedBucketCostTooltip(bucketBytes, region)}
			<br />
			From CloudWatch storage metrics
			{measuredAt != null
				? `, measured ${new Date(measuredAt).toLocaleString()}`
				: ""}
			; collected about once a day, so this figure updates less often than the
			repository stats above.
		</>
	);
}

/// Repository stats (top, beside the config summary). Read-only snapshot of the
/// kopia repo's size/counts as of the last inspection.
function RepoStatsPanel({
	groupId,
	region,
}: {
	groupId: string;
	region: string | null;
}) {
	const stats = useApi(
		"backups",
		"stats",
		{ server_group_id: groupId },
		[groupId],
	);
	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<Typography variant="h6" component="h2" gutterBottom>
				Repository stats
			</Typography>
			{stats.status === "loading" || stats.status === "idle" ? (
				<LinearProgress />
			) : stats.status === "error" ? (
				<Alert severity="error">{stats.error.message}</Alert>
			) : (
				<Stack spacing={0.5}>
					{stats.data.stats == null ? (
						<Typography color="text.secondary">
							No stats yet (awaiting first inspection).
						</Typography>
					) : (
						<>
							<Stat label="Snapshots" value={stats.data.stats.snapshot_count} />
							<Stat label="Sources" value={stats.data.stats.source_count} />
							<Stat
								label="Logical bytes"
								value={formatBytes(stats.data.stats.logical_bytes)}
							/>
							<Stat
								label="Physical bytes"
								value={formatBytes(stats.data.stats.physical_bytes)}
							/>
							<Typography variant="caption" color="text.secondary">
								Observed <TimeAgo timestamp={stats.data.stats.observed_at} />
							</Typography>
							<Typography variant="body2">
								<strong>Bucket bytes:</strong>{" "}
								{stats.data.stats.bucket_bytes == null ? (
									"unknown"
								) : (
									<Tooltip
										title={bucketBytesTooltip(
											stats.data.stats.bucket_bytes,
											stats.data.stats.bucket_bytes_observed_at,
											region,
										)}
									>
										<span>{formatBytes(stats.data.stats.bucket_bytes)}</span>
									</Tooltip>
								)}
							</Typography>
						</>
					)}
					<S3MonthlyTrafficStat
						sentBytes={stats.data.s3_month_sent_bytes}
						receivedBytes={stats.data.s3_month_received_bytes}
						region={region}
					/>
				</Stack>
			)}
		</Paper>
	);
}

/// This calendar month's S3 traffic tallied across the group's device backup
/// runs, with a tooltip estimating the egress cost. Uploads (sent) are data
/// transfer IN, which AWS never charges for; downloads (received) are egress
/// to the internet, billed per GB by region.
function S3MonthlyTrafficStat({
	sentBytes,
	receivedBytes,
	region,
}: {
	sentBytes: number;
	receivedBytes: number;
	region: string | null;
}) {
	const { rate, region: priceRegion, assumed } = s3EgressRateForRegion(region);
	const estimatedCost = (receivedBytes / 1_000_000_000) * rate;
	const tooltip =
		`Uploads are free (data transfer in). Downloads are billed as internet ` +
		`egress: ~$${rate.toFixed(3)}/GB in ${priceRegion}` +
		(assumed ? " (assumed — no region set on this config)" : "") +
		`, so this month's downloads are an estimated $${estimatedCost.toFixed(2)}. ` +
		`Excludes AWS's 100 GB/month free egress allowance and per-request costs.`;
	return (
		<Stack spacing={0}>
			<Tooltip title={tooltip}>
				<Typography variant="body2" sx={{ cursor: "help", width: "fit-content" }}>
					<strong>S3 traffic (this month):</strong> {formatBytes(sentBytes)} sent /{" "}
					{formatBytes(receivedBytes)} received
				</Typography>
			</Tooltip>
			<Typography variant="caption" color="text.secondary">
				* device backup traffic only — repository maintenance/inspection traffic
				isn't tallied
			</Typography>
		</Stack>
	);
}

/// The group's recent backup runs (bottom). Failed runs expand to their error.
function RecentRunsPanel({
	groupId,
	members,
}: {
	groupId: string;
	members: RankedMachine[];
}) {
	// Poll faster while a run is in flight, so its progress figures actually
	// advance while someone is watching — a live view that only moved on page
	// reload would defeat the point. Back off to a gentle cadence when the table
	// is all settled history. Mirrors MaintenancePanel.
	const [inFlight, setInFlight] = useState(false);
	const tick = useReloadInterval(
		inFlight ? 5_000 : 30_000,
		"canopy-data-changed",
	);
	const stats = useApi(
		"backups",
		"stats",
		{ server_group_id: groupId },
		[groupId, tick],
	);
	const anyInFlight =
		stats.status === "ok" &&
		stats.data.recent_runs.some((r) => r.status === "in_progress");
	useEffect(() => {
		setInFlight(anyInFlight);
	}, [anyInFlight]);
	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<Typography variant="h6" component="h2" gutterBottom>
				Recent runs
			</Typography>
			{stats.status === "loading" || stats.status === "idle" ? (
				<LinearProgress />
			) : stats.status === "error" ? (
				<Alert severity="error">{stats.error.message}</Alert>
			) : stats.data.recent_runs.length === 0 ? (
				<Typography color="text.secondary">No runs reported yet.</Typography>
			) : (
				<Box sx={{ overflowX: "auto" }}>
					<Table size="small">
						<TableHead>
							<TableRow>
								<TableCell padding="checkbox" />
								<TableCell>When</TableCell>
								<TableCell>Server</TableCell>
								<TableCell>Type</TableCell>
								<TableCell>Purpose</TableCell>
								<TableCell>Outcome</TableCell>
								<TableCell>Duration</TableCell>
								<TableCell>Snapshot size</TableCell>
								<TableCell>Transfer</TableCell>
								<TableCell>Snapshot</TableCell>
							</TableRow>
						</TableHead>
						<TableBody>
							{stats.data.recent_runs.map((r) => (
								<RunRow key={r.key} run={r} members={members} />
							))}
						</TableBody>
					</Table>
				</Box>
			)}
		</Paper>
	);
}

const MAINT_KIND_LABEL: Record<string, string> = {
	quick: "Quick",
	full: "Full",
};

/// One row of the maintenance table. Failed runs get an expand toggle that
/// reveals the error in a collapsible sub-row (mirrors RunRow).
function MaintRow({ run }: { run: BackupMaintenanceRun }) {
	const [open, setOpen] = useState(false);
	const hasError = Boolean(run.error);
	const running = run.outcome == null;
	return (
		<>
			<TableRow
				sx={hasError ? { "& > *": { borderBottom: "unset" } } : undefined}
			>
				<TableCell padding="checkbox">
					{hasError && (
						<IconButton
							size="small"
							aria-label={open ? "Hide error" : "Show error"}
							onClick={() => setOpen((o) => !o)}
						>
							{open ? <KeyboardArrowUpIcon /> : <KeyboardArrowDownIcon />}
						</IconButton>
					)}
				</TableCell>
				<TableCell>
					<TimeAgo timestamp={run.started_at} />
				</TableCell>
				<TableCell>{MAINT_KIND_LABEL[run.kind] ?? run.kind}</TableCell>
				<TableCell>
					{running ? (
						<Chip size="small" label="running" color="info" />
					) : (
						<Chip
							size="small"
							label={run.outcome}
							color={run.outcome === "success" ? "success" : "error"}
						/>
					)}
				</TableCell>
				<TableCell>
					{run.finished_at ? <TimeAgo timestamp={run.finished_at} /> : "—"}
				</TableCell>
				<TableCell>
					{run.finished_at
						? humanDuration(run.started_at, run.finished_at)
						: "—"}
				</TableCell>
				<TableCell>
					{run.bytes_reclaimed == null ? "—" : formatBytes(run.bytes_reclaimed)}
				</TableCell>
			</TableRow>
			{hasError && (
				<TableRow>
					<TableCell colSpan={7} sx={{ py: 0, border: 0 }}>
						<Collapse in={open} timeout="auto" unmountOnExit>
							<Alert severity="error" variant="outlined" sx={{ my: 1 }}>
								<Typography
									component="pre"
									variant="body2"
									sx={{
										m: 0,
										fontFamily: "monospace",
										whiteSpace: "pre-wrap",
										wordBreak: "break-word",
									}}
								>
									{run.error}
								</Typography>
							</Alert>
						</Collapse>
					</TableCell>
				</TableRow>
			)}
		</>
	);
}

/// At-a-glance "has maintenance run, and did it succeed" indicator derived from
/// the recent runs. The authoritative overdue/failed alerting is the group
/// incident (backup-maintenance-stale / backup-maintenance-error); this is the
/// quick read for an operator looking at the panel.
function MaintenanceSummary({ runs }: { runs: BackupMaintenanceRun[] }) {
	if (runs.length === 0) {
		return (
			<Alert severity="warning" variant="outlined">
				No maintenance has run yet.
			</Alert>
		);
	}
	const lastFinished = runs.find((r) => r.outcome != null);
	const lastSuccess = runs.find((r) => r.outcome === "success");
	const failing = lastFinished?.outcome === "failure";
	return (
		<Stack spacing={0.5}>
			<Box>
				{failing ? (
					<Chip size="small" color="error" label="Last run failed" />
				) : lastFinished ? (
					<Chip size="small" color="success" label="Healthy" />
				) : (
					<Chip size="small" color="info" label="Running" />
				)}
			</Box>
			<Typography variant="body2" color="text.secondary">
				{lastSuccess ? (
					<>
						Last successful maintenance{" "}
						<TimeAgo
							timestamp={lastSuccess.finished_at ?? lastSuccess.started_at}
						/>
					</>
				) : (
					"No successful maintenance recorded yet."
				)}
			</Typography>
		</Stack>
	);
}

/// Repo maintenance: the at-a-glance health summary plus recent kopia
/// maintenance cycles. Every cycle expires snapshots per the retention policy
/// (`kopia snapshot expire --delete`); full maintenance additionally reclaims
/// the freed space, while quick is the lighter compaction. Failed runs expand
/// to their error.
function MaintenancePanel({
	groupId,
	config,
	isAdmin,
	onChanged,
}: {
	groupId: string;
	config: BackupConfigView;
	isAdmin: boolean;
	onChanged: () => void;
}) {
	// Poll faster while a run is in flight so the "running" indicator appears and
	// clears promptly; back off to a gentle cadence when idle.
	const [running, setRunning] = useState(false);
	const tick = useReloadInterval(running ? 5_000 : 30_000, "canopy-data-changed");
	const stats = useApi(
		"backups",
		"stats",
		{ server_group_id: groupId },
		[groupId, tick],
	);
	const request = useApiAction("backups", "request_maintenance");
	const cancel = useApiAction("backups", "cancel_maintenance");
	const queued = config.force_full_maintenance_at != null;

	// An open run (no outcome yet) is one that's currently in flight — the run row
	// is written before the kopia work starts. `full` in flight blocks a useful
	// new full request; any run in flight drives the spinner + poll cadence.
	const recent =
		stats.status === "ok" ? stats.data.recent_maintenance : [];
	const fullRunning = recent.some((m) => m.outcome == null && m.kind === "full");
	const anyRunning = recent.some((m) => m.outcome == null);
	if (anyRunning !== running) setRunning(anyRunning);

	const onRequest = async () => {
		try {
			await request.call({ server_group_id: groupId });
			onChanged();
		} catch {
			/* surfaced via request.error */
		}
	};
	const onCancel = async () => {
		try {
			await cancel.call({ server_group_id: groupId });
			onChanged();
		} catch {
			/* surfaced via cancel.error */
		}
	};

	return (
		<Paper variant="outlined" sx={{ p: 2 }} data-testid="repo-maintenance">
			<Stack
				direction="row"
				spacing={2}
				sx={{
					alignItems: "center",
					justifyContent: "space-between",
					mb: 1,
				}}
			>
				<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
					<Typography variant="h6" component="h2">
						Repo maintenance
					</Typography>
					{anyRunning && (
						<Chip
							size="small"
							color="info"
							variant="outlined"
							icon={<CircularProgress size={12} thickness={6} />}
							label="Running"
						/>
					)}
				</Stack>
				{isAdmin &&
					(queued ? (
						<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
							<Chip
								size="small"
								color="info"
								label={
									config.force_full_maintenance_by
										? `Full run queued by ${config.force_full_maintenance_by}`
										: "Full run queued"
								}
							/>
							<GradedAction calls="backups/cancel_maintenance">
								<Button
									size="small"
									disabled={cancel.pending}
									onClick={onCancel}
								>
									Cancel
								</Button>
							</GradedAction>
						</Stack>
					) : (
						<Tooltip
							title={
								fullRunning
									? "A full maintenance run is already in progress"
									: ""
							}
						>
							<span>
								<GradedAction calls="backups/request_maintenance">
									<Button
										size="small"
										variant="outlined"
										disabled={request.pending || fullRunning}
										onClick={onRequest}
									>
										Run full maintenance now
									</Button>
								</GradedAction>
							</span>
						</Tooltip>
					))}
			</Stack>
			{(request.error || cancel.error) && (
				<Alert severity="error" sx={{ mb: 1 }}>
					{(request.error ?? cancel.error)?.message}
				</Alert>
			)}
			{isAdmin && queued && (
				<Typography variant="body2" color="text.secondary" sx={{ mb: 1 }}>
					Queued — the scheduler picks it up within a minute; it runs once the
					group has no other maintenance in flight.
				</Typography>
			)}
			{stats.status === "loading" || stats.status === "idle" ? (
				<LinearProgress />
			) : stats.status === "error" ? (
				<Alert severity="error">{stats.error.message}</Alert>
			) : (
				<Stack spacing={1.5}>
					<MaintenanceSummary runs={stats.data.recent_maintenance} />
					{stats.data.recent_maintenance.length > 0 && (
						<Box sx={{ overflowX: "auto" }}>
							<Table size="small">
								<TableHead>
									<TableRow>
										<TableCell padding="checkbox" />
										<TableCell>Started</TableCell>
										<TableCell>Kind</TableCell>
										<TableCell>Outcome</TableCell>
										<TableCell>Finished</TableCell>
										<TableCell>Duration</TableCell>
										<TableCell>
											<Tooltip title="Approximate: kopia's garbage collection is two-phase, so a run can under-report until a later full run sweeps quarantined blobs">
												<span>Reclaimed</span>
											</Tooltip>
										</TableCell>
									</TableRow>
								</TableHead>
								<TableBody>
									{stats.data.recent_maintenance.map((m) => (
										<MaintRow key={m.id} run={m} />
									))}
								</TableBody>
							</Table>
						</Box>
					)}
				</Stack>
			)}
		</Paper>
	);
}

/// The group's machines and their backup types: per (machine, type) the
/// schedule state, when the next backup is expected (per-machine, so a lagging
/// box isn't masked by a freshly-backed-up sibling), the latest snapshot, and
/// the on-demand "backup now" action.
///
/// Boxes rather than workloads: a backup is taken of the box, so a host
/// carrying two applications appears once here.
/// spec: BKO#status
function ServersPanel({
	groupId,
	members,
	isAdmin,
}: {
	groupId: string;
	members: RankedMachine[];
	isAdmin: boolean;
}) {
	// As with the runs panel: watch closely while a backup is in flight so its
	// figures move on their own, and back off once nothing is running.
	const [inFlight, setInFlight] = useState(false);
	const tick = useReloadInterval(
		inFlight ? 5_000 : 30_000,
		"canopy-data-changed",
	);
	const stats = useApi(
		"backups",
		"stats",
		{ server_group_id: groupId },
		[groupId, tick],
	);
	const requestNow = useApiAction("backups", "request_now");
	const cancel = useApiAction("backups", "cancel_request");
	const allowRestore = useApiAction("backups", "allow_restore");
	const disallowRestore = useApiAction("backups", "disallow_restore");

	const pending = stats.status === "ok" ? stats.data.pending_requests : [];
	const capabilities = stats.status === "ok" ? stats.data.capabilities : [];
	const anyInFlight = capabilities.some((c) => c.processing_since != null);
	useEffect(() => {
		setInFlight(anyInFlight);
	}, [anyInFlight]);
	const restoreWindows =
		stats.status === "ok" ? stats.data.restore_windows : [];
	const restoreWindowFor = (machineId: string) =>
		restoreWindows.find((w) => w.machine_id === machineId);

	const onAllowRestore = async (machineId: string) => {
		try {
			await allowRestore.call({ machine_id: machineId });
			stats.reload();
		} catch {
			/* surfaced via allowRestore.error */
		}
	};
	const onDisallowRestore = async (machineId: string) => {
		try {
			await disallowRestore.call({ machine_id: machineId });
			stats.reload();
		} catch {
			/* surfaced via disallowRestore.error */
		}
	};

	// Per-machine restore-window control, shown under the box's name. Restores
	// are gated behind a deliberate, time-boxed opt-in, so this is where an
	// operator opens (or closes) the window before running `bestool canopy
	// restore` on the box.
	const restoreControl = (machineId: string) => {
		if (!isAdmin) return null;
		const win = restoreWindowFor(machineId);
		const busy = allowRestore.pending || disallowRestore.pending;
		if (win) {
			return (
				<Stack
					direction="row"
					spacing={0.5}
					sx={{ alignItems: "center", flexWrap: "wrap" }}
				>
					<Chip
						size="small"
						color="warning"
						variant="outlined"
						icon={<RestoreIcon />}
						label={
							<>
								restores allowed until{" "}
								<TimeAgo timestamp={win.allowed_until} />
							</>
						}
					/>
					<GradedAction calls="backups/disallow_restore">
						<Button
							size="small"
							onClick={() => onDisallowRestore(machineId)}
							disabled={busy}
						>
							Disable
						</Button>
					</GradedAction>
				</Stack>
			);
		}
		return (
			<GradedAction calls="backups/allow_restore">
				<Button
					size="small"
					startIcon={<RestoreIcon />}
					onClick={() => onAllowRestore(machineId)}
					disabled={busy}
				>
					Allow restores
				</Button>
			</GradedAction>
		);
	};

	// The backup types to offer per machine: the ones the box has declared it
	// can run (bestool fails fast on a type it has no definition for), unioned
	// with any type that already has a pending backup request — so a request can
	// always be cancelled even if the box later stops declaring that type.
	const typesForMachine = (machineId: string): string[] => {
		const set = new Set<string>();
		for (const c of capabilities) {
			if (c.machine_id === machineId) set.add(c.type);
		}
		for (const p of pending) {
			if (p.machine_id === machineId && p.purpose === "backup") set.add(p.type);
		}
		return [...set].sort();
	};
	const pendingFor = (machineId: string, type: string) =>
		pending.find(
			(p) =>
				p.machine_id === machineId && p.type === type && p.purpose === "backup",
		);
	const capFor = (machineId: string, type: string) =>
		capabilities.find((c) => c.machine_id === machineId && c.type === type);

	const onRequest = async (machineId: string, type: string) => {
		try {
			await requestNow.call({ machine_id: machineId, type, purpose: "backup" });
			stats.reload();
		} catch {
			/* surfaced via requestNow.error */
		}
	};
	const onCancel = async (machineId: string, type: string) => {
		try {
			await cancel.call({ machine_id: machineId, type, purpose: "backup" });
			stats.reload();
		} catch {
			/* surfaced via cancel.error */
		}
	};

	// The box's own page, at its backup section. A box with no name of its own
	// is recognisable by what runs on it, so fall back to that before the id.
	const machineLink = (box: RankedMachine) => (
		<MuiLink
			component={RouterLink}
			to={`/fleet/machines/${box.machine.id}#backups`}
			variant="body2"
			underline="hover"
		>
			{box.machine.name ??
				(box.applications[0] && applicationName(box.applications[0])) ??
				box.machine.id.slice(0, 8)}
		</MuiLink>
	);

	const actionCell = (machineId: string, type: string) => {
		const req = pendingFor(machineId, type);
		if (req) {
			return (
				<Stack
					direction="row"
					spacing={1}
					sx={{ alignItems: "center", justifyContent: "flex-end" }}
				>
					<Chip
						size="small"
						color="info"
						label={
							<>
								requested <TimeAgo timestamp={req.requested_at} />
							</>
						}
					/>
					{isAdmin && (
						<GradedAction calls="backups/cancel_request">
							<Button
								size="small"
								onClick={() => onCancel(machineId, type)}
								disabled={cancel.pending}
							>
								Cancel
							</Button>
						</GradedAction>
					)}
				</Stack>
			);
		}
		return (
			isAdmin && (
				<GradedAction calls="backups/request_now">
					<Button
						size="small"
						variant="outlined"
						startIcon={<BackupIcon />}
						onClick={() => onRequest(machineId, type)}
						disabled={requestNow.pending}
					>
						Backup now
					</Button>
				</GradedAction>
			)
		);
	};

	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<Typography variant="h6" component="h2" gutterBottom>
				Machines
			</Typography>
			{members.length === 0 ? (
				<Typography color="text.secondary">
					No member machines in this group.
				</Typography>
			) : (
				<Box sx={{ overflowX: "auto" }}>
					<Table size="small">
						<TableHead>
							<TableRow>
								<TableCell>Machine</TableCell>
								<TableCell>Type</TableCell>
								<TableCell>Next backup</TableCell>
								<TableCell>Latest snapshot</TableCell>
								<TableCell align="right">Actions</TableCell>
							</TableRow>
						</TableHead>
						<TableBody>
							{groupServersByRank(members).map(([rank, rankMembers]) => (
								<Fragment key={rank ?? "_unranked"}>
									{rank && (
										<TableRow>
											<TableCell
												colSpan={5}
												sx={{ borderBottom: "none", pb: 0 }}
											>
												<Typography
													variant="overline"
													color="text.secondary"
												>
													{rank}
												</Typography>
											</TableCell>
										</TableRow>
									)}
									{rankMembers.map((m) => {
										const types = typesForMachine(m.machine.id);
										if (types.length === 0) {
											return (
												<TableRow key={m.machine.id}>
													<TableCell>
														<Stack spacing={0.5} sx={{ alignItems: "flex-start" }}>
															{machineLink(m)}
															{restoreControl(m.machine.id)}
														</Stack>
													</TableCell>
													<TableCell colSpan={3}>
														<Typography variant="body2" color="text.secondary">
															No backup types registered yet
														</Typography>
													</TableCell>
													<TableCell align="right">
														<Tooltip title="This machine hasn't registered any backup types yet.">
															{/* span so the tooltip works on the disabled button */}
															<span>
																<Button
																	size="small"
																	variant="outlined"
																	startIcon={<BackupIcon />}
																	disabled
																>
																	Backup now
																</Button>
															</span>
														</Tooltip>
													</TableCell>
												</TableRow>
											);
										}
										return types.map((t, i) => {
											const cap = capFor(m.machine.id, t);
											return (
												<TableRow key={`${m.machine.id}:${t}`}>
													{i === 0 && (
														<TableCell
															rowSpan={types.length}
															sx={{ verticalAlign: "top" }}
														>
															<Stack
																spacing={0.5}
																sx={{ alignItems: "flex-start" }}
															>
																{machineLink(m)}
																{restoreControl(m.machine.id)}
															</Stack>
														</TableCell>
													)}
													<TableCell>
														<Stack
															direction="row"
															spacing={1}
															sx={{ alignItems: "center" }}
														>
															<Typography
																variant="body2"
																sx={{ fontFamily: "monospace" }}
															>
																{t}
															</Typography>
															{cap?.enabled === false && (
																<Tooltip title="Not on the backup schedule for this machine (toggle it on in the machine's Backups section). You can still back it up on demand.">
																	<Chip
																		size="small"
																		variant="outlined"
																		label="not scheduled"
																	/>
																</Tooltip>
															)}
															<BackupProcessingChip
																since={cap?.processing_since}
															/>
														</Stack>
														<BackupLiveProgress progress={cap?.progress} />
													</TableCell>
													<TableCell>
														{cap?.next_backup_at ? (
															<TimeAgo timestamp={cap.next_backup_at} />
														) : (
															<Typography variant="body2" color="text.secondary">
																—
															</Typography>
														)}
													</TableCell>
													<TableCell>
														<LatestSnapshot
															id={cap?.latest_snapshot_id}
															at={cap?.latest_snapshot_at}
															bytes={cap?.latest_snapshot_bytes}
														/>
													</TableCell>
													<TableCell align="right">{actionCell(m.machine.id, t)}</TableCell>
												</TableRow>
											);
										});
									})}
								</Fragment>
							))}
						</TableBody>
					</Table>
				</Box>
			)}
			{(requestNow.error ||
				cancel.error ||
				allowRestore.error ||
				disallowRestore.error) && (
				<Alert severity="error" sx={{ mt: 1 }}>
					{
						(requestNow.error ||
							cancel.error ||
							allowRestore.error ||
							disallowRestore.error)!.message
					}
				</Alert>
			)}
		</Paper>
	);
}

function Stat({
	label,
	value,
}: {
	label: string;
	value: number | string | null;
}) {
	return (
		<Typography variant="body2">
			<strong>{label}:</strong>{" "}
			{value == null ? "unknown" : value}
		</Typography>
	);
}

/// Format a byte count, rendering null as "unknown" (indicators always show a
/// state, never hide).
