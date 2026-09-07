import {
	Alert as MuiAlert,
	Box,
	Button,
	Chip,
	IconButton,
	LinearProgress,
	Link as MuiLink,
	MenuItem,
	Paper,
	Stack,
	TextField,
	Tooltip,
	Typography,
} from "@mui/material";
import AccessTimeIcon from "@mui/icons-material/AccessTime";
import BugReportIcon from "@mui/icons-material/BugReport";
import CheckCircleOutlinedIcon from "@mui/icons-material/CheckCircleOutlined";
import NotesIcon from "@mui/icons-material/StickyNote2";
import RefreshIcon from "@mui/icons-material/Refresh";
import { useState } from "react";
import { Link as RouterLink, useParams } from "react-router-dom";
import { useApi, useApiAction } from "../api";
import IssueRow from "../components/IssueRow";
import BuildOutlinedIcon from "@mui/icons-material/BuildOutlined";
import DeclareMaintenanceDialog from "../components/DeclareMaintenanceDialog";
import { AddNoteButton } from "../components/NotesList";
import { useIsAdmin } from "../hooks/useIsAdmin";
import { useIsNotificationHeld } from "../hooks/useIsNotificationHeld";
import TimeAgo from "../components/TimeAgo";
import ResolverAvatar from "../components/ResolverAvatar";
import { usePageTitle } from "../hooks/usePageTitle";
import { humanDuration } from "../lib/humanDuration";
import {
	CHECK_RESULT_ORDER,
	RESOLVED_REASONS,
	RESOLVED_REASON_LABEL,
	incidentTargetName,
	isIncidentLingering,
	type CheckResult,
	type IncidentIssueData,
	type IncidentNoteData,
	type IncidentWithIssues,
	type IssueData,
	type ResolvedReason,
} from "../types";

type Filter = "all" | "issues" | "notes";


export default function IncidentDetail() {
	const { id = "" } = useParams<{ id: string }>();
	const [refreshTick, setRefreshTick] = useState(0);
	const bumpRefresh = () => setRefreshTick((t) => t + 1);
	const [filter, setFilter] = useState<Filter>("all");

	const detail = useApi(
		"incidents",
		"get",
		{ incident_id: id },
		[id, refreshTick],
	);
	const notes = useApi(
		"incidents",
		"list_notes",
		{ incident_id: id },
		[id, refreshTick],
	);

	usePageTitle(
		detail.status === "ok"
			? `Incident ${detail.data.incident.id.slice(0, 8)} on ${incidentTargetName(detail.data.incident) || "(unknown)"}`
			: "Incident",
	);

	if (detail.status === "loading" || detail.status === "idle") {
		return <LinearProgress />;
	}
	if (detail.status === "error") {
		return <MuiAlert severity="error">{detail.error.message}</MuiAlert>;
	}

	const { incident, issues } = detail.data;
	const noteList = notes.status === "ok" ? notes.data : [];

	return (
		<Stack spacing={3}>
			<Header incident={incident} onChanged={bumpRefresh} />

			<Stack
				direction="row"
				spacing={1}
				sx={{ alignItems: "center", flexWrap: "wrap" }}
			>
				<Chip
					label="All"
					color={filter === "all" ? "primary" : "default"}
					onClick={() => setFilter("all")}
					clickable
				/>
				<Chip
					icon={<BugReportIcon />}
					label={`Issues (${issues.length})`}
					color={filter === "issues" ? "primary" : "default"}
					onClick={() => setFilter("issues")}
					clickable
				/>
				<Chip
					icon={<NotesIcon />}
					label={`Notes (${noteList.length})`}
					color={filter === "notes" ? "primary" : "default"}
					onClick={() => setFilter("notes")}
					clickable
				/>
				<Box sx={{ ml: "auto" }}>
					<IconButton aria-label="Refresh" size="small" onClick={bumpRefresh}>
						<RefreshIcon fontSize="small" />
					</IconButton>
				</Box>
			</Stack>

			<Timeline
				issues={filter === "notes" ? [] : issues}
				notes={filter === "issues" ? [] : noteList}
				onChanged={bumpRefresh}
			/>
		</Stack>
	);
}

function Header({
	incident,
	onChanged,
}: {
	incident: IncidentWithIssues["incident"];
	onChanged: () => void;
}) {
	const open = incident.closed_at == null;
	const heldUntilActive = useIsNotificationHeld(
		incident.notification_held_until,
	);
	const lingering = isIncidentLingering(incident);
	// A lingering incident's open notice is also gated by the drainer, so
	// the held countdown would be wrong — the recovery state wins.
	const held = open && heldUntilActive && !lingering;
	const isAdmin = useIsAdmin() === true;
	const resolve = useApiAction("incidents", "resolve");
	const unresolve = useApiAction("incidents", "unresolve");

	const [resolveOpen, setResolveOpen] = useState(false);
	const [maintenanceOpen, setMaintenanceOpen] = useState(false);
	const [reason, setReason] = useState<ResolvedReason>("fixed");

	const wrap = async (fn: () => Promise<unknown>) => {
		try {
			await fn();
			onChanged();
		} catch {
			/* surfaced via *.error */
		}
	};
	const error = resolve.error ?? unresolve.error;

	const timeText = (() => {
		if (open) {
			return (
				<>
					opened <TimeAgo timestamp={incident.opened_at} />
				</>
			);
		}
		const closedAt = incident.closed_at;
		if (!closedAt) return null;
		const lasted = humanDuration(incident.opened_at, closedAt);
		const reasonKey = incident.resolved_reason as ResolvedReason | null;
		const reasonLabel =
			reasonKey && reasonKey in RESOLVED_REASON_LABEL
				? RESOLVED_REASON_LABEL[reasonKey].toLowerCase()
				: incident.resolved_reason;
		return (
			<>
				closed
				{reasonLabel ? ` as ${reasonLabel}` : ""} <TimeAgo timestamp={closedAt} />, lasted {lasted}
			</>
		);
	})();

	return (
		<Paper
			variant="outlined"
			sx={{
				p: 2,
				borderColor: open
					? held
						? "warning.main"
						: "error.main"
					: "divider",
				borderWidth: open ? 2 : 1,
			}}
		>
			<Stack
				direction="row"
				spacing={2}
				sx={{ alignItems: "flex-start", justifyContent: "space-between" }}
			>
				<Box sx={{ minWidth: 0, flex: 1 }}>
					<Typography variant="h4" component="h1">
						Incident{" "}
						<Box
							component="span"
							sx={{ fontFamily: "monospace", color: "text.secondary" }}
						>
							{incident.id.slice(0, 8)}
						</Box>{" "}
						on{" "}
						{incident.server_group_id != null ? (
							<MuiLink
								component={RouterLink}
								to={`/fleet/groups/${incident.server_group_id}`}
								underline="hover"
								color="inherit"
							>
								{incidentTargetName(incident) || "(unknown group)"}
							</MuiLink>
						) : (
							// A canopy-wide incident has no group page; the
							// self-alerts view is its home.
							<MuiLink
								component={RouterLink}
								to="/alerts"
								underline="hover"
								color="inherit"
							>
								{incident.server_group_name || "Canopy"}
							</MuiLink>
						)}
					</Typography>
					<Typography variant="body2" color="text.secondary" sx={{ mt: 0.5 }}>
						{timeText}
					</Typography>
					{held && incident.notification_held_until && (
						<Stack
							direction="row"
							spacing={1}
							sx={{ mt: 1, alignItems: "center", flexWrap: "wrap" }}
							useFlexGap
						>
							<Chip
								icon={<AccessTimeIcon />}
								label={
									<>
										Holding; posting{" "}
										<TimeAgo
											timestamp={incident.notification_held_until}
										/>
									</>
								}
								color="warning"
								variant="outlined"
								size="small"
							/>
							<Typography variant="caption" color="text.secondary">
								Resolving inside the window cancels the open and skips
								the resolve, so Slack never hears about either edge.
							</Typography>
						</Stack>
					)}
					{lingering && incident.lingering_since && (
						<Stack
							direction="row"
							spacing={1}
							sx={{ mt: 1, alignItems: "center", flexWrap: "wrap" }}
							useFlexGap
						>
							<Chip
								icon={<AccessTimeIcon />}
								label={
									<>
										Recovering; last failure cleared{" "}
										<TimeAgo timestamp={incident.lingering_since} />
									</>
								}
								color="info"
								variant="outlined"
								size="small"
							/>
							<Typography variant="caption" color="text.secondary">
								A failure returning within the linger window continues
								this incident; otherwise it closes as of when the last
								failure cleared.
							</Typography>
						</Stack>
					)}
				</Box>
				<Box sx={{ flexShrink: 0 }}>
					{incident.resolved_at && (
						<ResolverAvatar
							resolvedBy={incident.resolved_by}
							resolvedByName={incident.resolved_by_name}
							resolvedByPic={incident.resolved_by_pic}
							resolvedReason={incident.resolved_reason}
							size={36}
						/>
					)}
				</Box>
			</Stack>

			<Stack
				direction="row"
				spacing={1}
				sx={{ mt: 2, alignItems: "center", flexWrap: "wrap" }}
				useFlexGap
			>
				{isAdmin && (incident.resolved_at ? (
					<Button
						size="small"
						variant="outlined"
						color="warning"
						startIcon={<CheckCircleOutlinedIcon />}
						onClick={() =>
							wrap(() => unresolve.call({ incident_id: incident.id }))
						}
					>
						Unresolve
					</Button>
				) : (
					<Button
						size="small"
						variant="outlined"
						color="success"
						startIcon={<CheckCircleOutlinedIcon />}
						onClick={() => setResolveOpen((v) => !v)}
					>
						Resolve…
					</Button>
				))}
				{isAdmin && (
					<>
						<AddNoteButton
							apiModule="incidents"
							parentKey="incident_id"
							parentId={incident.id}
							onAdded={onChanged}
							variant="outlined"
						/>
					</>
				)}
				{/* An operator who recognises this as work they are already
				    doing declares the window from where they are reading it. */}
				{/* spec: MNT#declaring */}
				{isAdmin && incident.server_group_id != null && (
					<>
						<Button
							size="small"
							variant="outlined"
							startIcon={<BuildOutlinedIcon />}
							onClick={() => setMaintenanceOpen(true)}
						>
							This is maintenance…
						</Button>
						<DeclareMaintenanceDialog
							open={maintenanceOpen}
							onClose={() => setMaintenanceOpen(false)}
							scope="group"
							id={incident.server_group_id}
							targetLabel={incident.server_group_name ?? undefined}
							onDone={onChanged}
						/>
					</>
				)}
				<Box sx={{ ml: "auto" }}>
					<Stack direction="row" spacing={2} sx={{ alignItems: "center" }}>
						<Stat
							icon={<BugReportIcon fontSize="inherit" />}
							value={incident.issue_count ?? 0}
							noun="issue"
						/>
						<Stat
							icon={<NotesIcon fontSize="inherit" />}
							value={incident.note_count ?? 0}
							noun="note"
						/>
					</Stack>
				</Box>
			</Stack>
			{resolveOpen && (
				<Stack direction="row" spacing={1} sx={{ mt: 1, alignItems: "center" }}>
					<TextField
						select
						size="small"
						label="Reason"
						value={reason}
						onChange={(e) => setReason(e.target.value as ResolvedReason)}
						sx={{ minWidth: 160 }}
					>
						{RESOLVED_REASONS.map((r) => (
							<MenuItem key={r} value={r}>
								{RESOLVED_REASON_LABEL[r]}
							</MenuItem>
						))}
					</TextField>
					<Button
						variant="outlined"
						size="small"
						color="success"
						startIcon={<CheckCircleOutlinedIcon />}
						onClick={() =>
							wrap(() =>
								resolve.call({ incident_id: incident.id, reason }),
							).then(() => setResolveOpen(false))
						}
					>
						Resolve
					</Button>
					<Button
						variant="outlined"
						size="small"
						onClick={() => setResolveOpen(false)}
					>
						Cancel
					</Button>
				</Stack>
			)}
			{error && (
				<MuiAlert severity="error" sx={{ mt: 1 }}>
					{error.message}
				</MuiAlert>
			)}
		</Paper>
	);
}

type TimelineEntry =
	| { kind: "issue"; at: number; issue: IncidentIssueData }
	| { kind: "note"; at: number; note: IncidentNoteData };

const CHECK_RESULT_RANK = new Map(
	CHECK_RESULT_ORDER.map((result, index) => [result, index]),
);

/// Where an issue sits in the severity ranking, by its *effective* result —
/// the graded one everything else acts on, so a check an operator capped at
/// warning ranks as a warning here too.
///
/// An issue with no recorded result predates the check-state model and has no
/// severity to rank on, so it sorts below everything graded rather than above
/// it, which is where an unknown lands if you index it directly.
function issueSeverityRank(issue: IssueData): number {
	const rank = issue.effective_result
		? CHECK_RESULT_RANK.get(issue.effective_result as CheckResult)
		: undefined;
	return rank ?? CHECK_RESULT_ORDER.length;
}

function Timeline({
	issues,
	notes,
	onChanged,
}: {
	issues: IncidentIssueData[];
	notes: IncidentNoteData[];
	onChanged: () => void;
}) {
	const entries: TimelineEntry[] = [
		...issues.map<TimelineEntry>((i) => ({
			kind: "issue",
			at: Date.parse(i.joined_at),
			issue: i,
		})),
		...notes.map<TimelineEntry>((n) => ({
			kind: "note",
			at: Date.parse(n.created_at),
			note: n,
		})),
	];
	// Issues above notes, ranked by severity so whatever is failing is at the
	// top of the incident and an operator doesn't have to read a whole
	// chronology to find it. Ties, and notes among themselves, fall back to
	// most recent first.
	entries.sort((a, b) => {
		if (a.kind !== b.kind) return a.kind === "issue" ? -1 : 1;
		if (a.kind === "issue" && b.kind === "issue") {
			const bySeverity =
				issueSeverityRank(a.issue.issue) - issueSeverityRank(b.issue.issue);
			if (bySeverity !== 0) return bySeverity;
		}
		return b.at - a.at;
	});

	if (entries.length === 0) {
		return <MuiAlert severity="info">No timeline entries yet.</MuiAlert>;
	}

	return (
		<Stack spacing={1}>
			{entries.map((e) =>
				e.kind === "issue" ? (
					<IssueRow
						key={`issue-${e.issue.issue.id}`}
						issue={e.issue.issue}
						defaultExpanded={
							e.issue.issue.active && !e.issue.issue.resolved_at
						}
						onChanged={onChanged}
					/>
				) : (
					<NoteEntry key={`note-${e.note.id}`} note={e.note} />
				),
			)}
		</Stack>
	);
}

function NoteEntry({ note }: { note: IncidentNoteData }) {
	return (
		<Box
			sx={{
				p: 1.5,
				border: 1,
				borderColor: "divider",
				borderRadius: 1,
				bgcolor: "background.paper",
			}}
		>
			<Stack
				direction="row"
				spacing={1}
				sx={{ alignItems: "center", justifyContent: "space-between" }}
			>
				<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
					<NotesIcon fontSize="small" color="action" />
					<Typography variant="caption" color="text.secondary">
						{note.author}
					</Typography>
				</Stack>
				<Typography variant="caption" color="text.secondary">
					<TimeAgo timestamp={note.created_at} />
				</Typography>
			</Stack>
			<Typography
				variant="body2"
				component="pre"
				sx={{ mt: 0.5, mb: 0, whiteSpace: "pre-wrap", fontFamily: "inherit" }}
			>
				{note.body}
			</Typography>
		</Box>
	);
}

function Stat({
	icon,
	value,
	noun,
}: {
	icon: React.ReactNode;
	value: number;
	noun: string;
}) {
	const title = `${value} ${noun}${value === 1 ? "" : "s"}`;
	return (
		<Tooltip title={title}>
			<Stack
				direction="row"
				spacing={0.5}
				sx={{
					alignItems: "center",
					color: "text.secondary",
					fontSize: "0.875rem",
				}}
			>
				{icon}
				<Box component="span">{value}</Box>
			</Stack>
		</Tooltip>
	);
}
