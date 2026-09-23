import {
	Alert as MuiAlert,
	Box,
	Button,
	IconButton,
	Link as MuiLink,
	MenuItem,
	Stack,
	TextField,
	Typography,
} from "@mui/material";
import CheckCircleOutlinedIcon from "@mui/icons-material/CheckCircleOutlined";
import ExpandLessIcon from "@mui/icons-material/ExpandLess";
import ExpandMoreIcon from "@mui/icons-material/ExpandMore";
import NotificationsOffOutlinedIcon from "@mui/icons-material/NotificationsOffOutlined";
import SnoozeIcon from "@mui/icons-material/Snooze";
import { Fragment, useState } from "react";
import { Link as RouterLink } from "react-router-dom";
import { useApiAction } from "../api";
import { useIsAdmin } from "../hooks/useIsAdmin";
import { humanDuration } from "../lib/humanDuration";
import MessageView from "./MessageView";
import NotesList, { AddNoteButton } from "./NotesList";
import ResolverAvatar from "./ResolverAvatar";
import TargetName from "./TargetName";
import CheckDocButton from "./CheckDocButton";
import CheckResultChip from "./CheckResultChip";
import { type Calls, GradedAction } from "./GradedAction";
import StatusSnapshotPanel, { StatusSnapshotButton } from "./StatusSnapshot";
import TimeAgo from "./TimeAgo";
import {
	RESOLVED_REASONS,
	RESOLVED_REASON_LABEL,
	healthcheckNameFromRef,
	healthcheckPath,
	type CheckResult,
	type IssueData,
	type IssueIncidentLink,
	type ResolvedReason,
} from "../types";

function isSnoozeActive(snoozedUntil: string | null): boolean {
	if (!snoozedUntil) return false;
	return Date.parse(snoozedUntil) > Date.now();
}

function headline(issue: IssueData): string {
	if (issue.description && issue.description.trim() !== "") {
		return issue.description;
	}
	return issue.message.split("\n")[0] ?? "";
}

/** One issue. Expanded by default: shows a provenance line (source, ref,
 * incident links), the message body, action buttons and the notes panel.
 * When collapsed, only the header row is visible —
 * even the action buttons are hidden. Header layout:
 * `[toggle] [server] [headline] [resolver-avatar?] [snapshot] [result] [time]`.
 * The headline is struck through when the issue is inactive or resolved.
 * The server is always shown because incidents can span child servers in
 * a group — relying on a page H1 to identify the server is insufficient.
 */
export default function IssueRow({
	issue,
	defaultExpanded = false,
	onChanged,
}: {
	issue: IssueData;
	/** Initial expanded state. Default `false` (collapsed) — fits list views;
	 * incident-detail timelines pass `true`. */
	defaultExpanded?: boolean;
	onChanged: () => void;
}) {
	const [expanded, setExpanded] = useState(defaultExpanded);
	const [notesRefresh, setNotesRefresh] = useState(0);
	const [headerSnapshotOpen, setHeaderSnapshotOpen] = useState(false);
	const snoozeActive = isSnoozeActive(issue.snoozed_until);
	const struckThrough = !issue.active || !!issue.resolved_at;
	const isAdmin = useIsAdmin() === true;
	return (
		<Box
			sx={{
				p: 1.5,
				border: 1,
				borderColor: "divider",
				borderRadius: 1,
				bgcolor: issue.active && !snoozeActive ? undefined : "action.hover",
			}}
		>
			<Header
				issue={issue}
				expanded={expanded}
				setExpanded={setExpanded}
				snoozeActive={snoozeActive}
				struckThrough={struckThrough}
				headerSnapshotOpen={headerSnapshotOpen}
				toggleHeaderSnapshot={() => setHeaderSnapshotOpen((v) => !v)}
			/>
			{headerSnapshotOpen && issue.application_id && (
				<StatusSnapshotPanel
					serverId={issue.application_id}
					at={issue.last_seen}
					onClose={() => setHeaderSnapshotOpen(false)}
				/>
			)}
			{expanded && (
				<Body
					issue={issue}
					snoozeActive={snoozeActive}
					notesRefresh={notesRefresh}
					isAdmin={isAdmin}
					onNoteAdded={() => setNotesRefresh((t) => t + 1)}
					onChanged={onChanged}
				/>
			)}
		</Box>
	);
}

function Header({
	issue,
	expanded,
	setExpanded,
	snoozeActive,
	struckThrough,
	headerSnapshotOpen,
	toggleHeaderSnapshot,
}: {
	issue: IssueData;
	expanded: boolean;
	setExpanded: (v: (p: boolean) => boolean) => void;
	snoozeActive: boolean;
	struckThrough: boolean;
	headerSnapshotOpen: boolean;
	toggleHeaderSnapshot: () => void;
}) {
	return (
		<Stack
			direction="row"
			spacing={1}
			sx={{ alignItems: "center", minWidth: 0 }}
		>
			<IconButton
				aria-label={expanded ? "Collapse" : "Expand"}
				size="small"
				onClick={() => setExpanded((v) => !v)}
			>
				{expanded ? (
					<ExpandLessIcon fontSize="small" />
				) : (
					<ExpandMoreIcon fontSize="small" />
				)}
			</IconButton>
			{issue.application_id != null ? (
				<MuiLink
					component={RouterLink}
					to={`/fleet/applications/${issue.application_id}`}
					underline="hover"
					color="text.primary"
					sx={{ fontWeight: 500, flexShrink: 0 }}
				>
					<TargetName
						parts={[
							{ label: issue.server_group_name ?? "" },
							{
								label:
									issue.server_name && issue.server_name.trim() !== ""
										? issue.server_name
										: issue.server_host || "(unknown)",
							},
						]}
					/>
				</MuiLink>
			) : issue.machine_id != null ? (
				// A machine's issue: the box's own, so it names the box rather
				// than reading as group-wide, and links to it. Reaching the box
				// is the whole point of the row — an operator triaging an
				// unreachable host has nowhere else to go from here.
				// spec: CHK#reachability
				<Box sx={{ fontWeight: 500, flexShrink: 0 }}>
					<TargetName
						parts={[
							{
								label: issue.server_group_name ?? "",
								to: issue.server_group_id
									? `/fleet/groups/${issue.server_group_id}`
									: null,
							},
							{
								label: issue.machine_name?.trim() || "(machine)",
								to: `/fleet/machines/${issue.machine_id}`,
							},
						]}
					/>
				</Box>
			) : (
				// Group-scoped issue (no target): link the group instead of a
				// bogus `/applications/null`. Falls back to a plain label when
				// the issue carries no group name.
				<Box sx={{ fontWeight: 500, flexShrink: 0 }}>
					{issue.server_group_name ? (
						<TargetName
							parts={[
								{
									label: issue.server_group_name,
									to: issue.server_group_id
										? `/fleet/groups/${issue.server_group_id}`
										: null,
								},
								{ label: "(group-wide)" },
							]}
						/>
					) : (
						<Typography
							component="span"
							color="text.secondary"
						>
							(group-wide)
						</Typography>
					)}
				</Box>
			)}
			<Typography
				variant="body2"
				sx={{
					flex: 1,
					minWidth: 0,
					overflow: "hidden",
					textOverflow: "ellipsis",
					whiteSpace: "nowrap",
					textDecoration: struckThrough ? "line-through" : undefined,
					color: struckThrough ? "text.secondary" : undefined,
				}}
				title={headline(issue)}
			>
				{headline(issue)}
			</Typography>
			{snoozeActive && (
				<Typography
					variant="caption"
					color="warning.main"
					sx={{ flexShrink: 0 }}
					title={`snoozed until ${issue.snoozed_until}`}
				>
					snoozed
				</Typography>
			)}
			<HeaderActor issue={issue} />
			<Box sx={{ flexShrink: 0 }}>
				<StatusSnapshotButton
					open={headerSnapshotOpen}
					onClick={toggleHeaderSnapshot}
					tooltip="Status snapshot when this issue was last seen"
				/>
			</Box>
			{issue.effective_result && (
				<Box sx={{ flexShrink: 0 }}>
					<CheckResultChip result={issue.effective_result as CheckResult} />
				</Box>
			)}
			{headerCheckName(issue) && (
				<Box sx={{ flexShrink: 0 }}>
					<CheckDocButton
						source={issue.source}
						namespace={issue.namespace ?? undefined}
						check={headerCheckName(issue) as string}
					/>
				</Box>
			)}
			<Typography
				variant="body2"
				color="text.secondary"
				sx={{ flexShrink: 0 }}
			>
				<ClosureOrTime issue={issue} />
			</Typography>
		</Stack>
	);
}

/** The check this issue tracks, for the header's documentation button:
 * the stamped check name, or derived from the ref for rows that predate
 * the check-state model. */
function headerCheckName(issue: IssueData): string | null {
	return issue.check_name ?? healthcheckNameFromRef(issue.source, issue.ref);
}

/** Header time slot. For closed issues, gives the closure context — reason
 * (operator-resolved) or "on its own" (device sent inactive) — plus the
 * lifetime. For still-active issues, falls back to last-seen. */
function ClosureOrTime({ issue }: { issue: IssueData }) {
	if (issue.resolved_at) {
		const reasonKey = issue.resolved_reason as ResolvedReason | null;
		const reasonLabel =
			reasonKey && reasonKey in RESOLVED_REASON_LABEL
				? RESOLVED_REASON_LABEL[reasonKey].toLowerCase()
				: issue.resolved_reason;
		const lasted = humanDuration(issue.first_seen, issue.resolved_at);
		return (
			<>
				closed{reasonLabel ? ` as ${reasonLabel}` : ""}{" "}
				<TimeAgo timestamp={issue.resolved_at} />, lasted {lasted}
			</>
		);
	}
	if (!issue.active) {
		const lasted = humanDuration(issue.first_seen, issue.last_seen);
		return (
			<>
				closed on its own <TimeAgo timestamp={issue.last_seen} />, lasted {lasted}
			</>
		);
	}
	return <TimeAgo timestamp={issue.last_seen} />;
}

/** Leftmost slot of the right-side header cluster. Resolver avatar when the
 * issue has been resolved; empty otherwise. */
function HeaderActor({ issue }: { issue: IssueData }) {
	if (!issue.resolved_at) return null;
	return (
		<ResolverAvatar
			resolvedBy={issue.resolved_by}
			resolvedByName={issue.resolved_by_name}
			resolvedByPic={issue.resolved_by_pic}
			resolvedReason={issue.resolved_reason}
		/>
	);
}

function Body({
	issue,
	snoozeActive,
	notesRefresh,
	isAdmin,
	onNoteAdded,
	onChanged,
}: {
	issue: IssueData;
	snoozeActive: boolean;
	notesRefresh: number;
	isAdmin: boolean;
	onNoteAdded: () => void;
	onChanged: () => void;
}) {
	return (
		<Box sx={{ mt: 1 }}>
			<IssueMeta issue={issue} />
			<MessageView message={issue.message} />
			{isAdmin && (
				<IssueActions
					issue={issue}
					snoozeActive={snoozeActive}
					onNoteAdded={onNoteAdded}
					onChanged={onChanged}
				/>
			)}
			<Box sx={{ mt: 1.5 }}>
				<NotesList
					apiModule="issues"
					parentKey="issue_id"
					parentId={issue.id}
					refreshKey={notesRefresh}
					canEdit={isAdmin}
				/>
			</Box>
		</Box>
	);
}

function IssueActions({
	issue,
	snoozeActive,
	onNoteAdded,
	onChanged,
}: {
	issue: IssueData;
	snoozeActive: boolean;
	onNoteAdded: () => void;
	onChanged: () => void;
}) {
	const resolve = useApiAction("issues", "resolve");
	const unresolve = useApiAction("issues", "unresolve");
	const snooze = useApiAction("issues", "snooze");
	const unsnooze = useApiAction("issues", "unsnooze");
	const silenceServer = useApiAction("silenced_refs", "silence_server");
	const silenceMachine = useApiAction("silenced_refs", "silence_machine");
	const silenceGroup = useApiAction("silenced_refs", "silence_group");

	const [resolveOpen, setResolveOpen] = useState(false);
	const [snoozeOpen, setSnoozeOpen] = useState(false);
	const [silenceOpen, setSilenceOpen] = useState(false);
	const [reason, setReason] = useState<ResolvedReason>("fixed");
	const [snoozeHours, setSnoozeHours] = useState(4);

	const wrap = async (fn: () => Promise<unknown>) => {
		try {
			await fn();
			onChanged();
		} catch {
			/* surfaced via *.error */
		}
	};

	const error =
		resolve.error
		?? unresolve.error
		?? snooze.error
		?? unsnooze.error
		?? silenceServer.error
		?? silenceMachine.error
		?? silenceGroup.error;

	const silenceCalls: Calls = [
		...(issue.application_id != null ? ["silenced_refs/silence_server" as const] : []),
		...(issue.machine_id != null ? ["silenced_refs/silence_machine" as const] : []),
		...(issue.server_group_id ? ["silenced_refs/silence_group" as const] : []),
	];

	return (
		<Box sx={{ mt: 1 }}>
			<Stack direction="row" spacing={1} sx={{ flexWrap: "wrap" }} useFlexGap>
				{issue.resolved_at ? (
					<GradedAction calls="issues/unresolve">
						<Button
							size="small"
							variant="outlined"
							startIcon={<CheckCircleOutlinedIcon />}
							onClick={() => wrap(() => unresolve.call({ issue_id: issue.id }))}
						>
							Unresolve
						</Button>
					</GradedAction>
				) : (
					<GradedAction calls="issues/resolve">
						<Button
							size="small"
							variant="outlined"
							startIcon={<CheckCircleOutlinedIcon />}
							onClick={() => setResolveOpen((v) => !v)}
						>
							Resolve…
						</Button>
					</GradedAction>
				)}
				{snoozeActive ? (
					<GradedAction calls="issues/unsnooze">
						<Button
							size="small"
							variant="outlined"
							startIcon={<SnoozeIcon />}
							onClick={() => wrap(() => unsnooze.call({ issue_id: issue.id }))}
						>
							Unsnooze
						</Button>
					</GradedAction>
				) : (
					<GradedAction calls="issues/snooze">
						<Button
							size="small"
							variant="outlined"
							startIcon={<SnoozeIcon />}
							onClick={() => setSnoozeOpen((v) => !v)}
						>
							Snooze…
						</Button>
					</GradedAction>
				)}
				<GradedAction opens={silenceCalls}>
					<Button
						size="small"
						variant="outlined"
						startIcon={<NotificationsOffOutlinedIcon />}
						onClick={() => setSilenceOpen((v) => !v)}
					>
						Silence ref…
					</Button>
				</GradedAction>
				<AddNoteButton
					apiModule="issues"
					parentKey="issue_id"
					parentId={issue.id}
					onAdded={onNoteAdded}
					variant="outlined"
				/>
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
					<GradedAction calls="issues/resolve">
						<Button
							variant="outlined"
							size="small"
							startIcon={<CheckCircleOutlinedIcon />}
							onClick={() =>
								wrap(() => resolve.call({ issue_id: issue.id, reason })).then(
									() => setResolveOpen(false),
								)
							}
						>
							Resolve
						</Button>
					</GradedAction>
					<Button
						variant="outlined"
						size="small"
						onClick={() => setResolveOpen(false)}
					>
						Cancel
					</Button>
				</Stack>
			)}
			{snoozeOpen && (
				<Stack direction="row" spacing={1} sx={{ mt: 1, alignItems: "center" }}>
					<TextField
						type="number"
						size="small"
						label="Hours"
						value={snoozeHours}
						onChange={(e) => setSnoozeHours(Number(e.target.value))}
						sx={{ width: 100 }}
						slotProps={{ htmlInput: { min: 1, max: 24 * 30 } }}
					/>
					<GradedAction calls="issues/snooze">
						<Button
							variant="outlined"
							size="small"
							startIcon={<SnoozeIcon />}
							onClick={() => {
								const until = new Date(
									Date.now() + snoozeHours * 3_600_000,
								).toISOString();
								wrap(() => snooze.call({ issue_id: issue.id, until })).then(() =>
									setSnoozeOpen(false),
								);
							}}
						>
							Snooze
						</Button>
					</GradedAction>
					<Button
						variant="outlined"
						size="small"
						onClick={() => setSnoozeOpen(false)}
					>
						Cancel
					</Button>
				</Stack>
			)}
			{silenceOpen && (
				<Stack spacing={1} sx={{ mt: 1 }}>
					<Typography variant="body2" color="text.secondary">
						Permanently ignore <code>{issue.source}/{issue.ref}</code> issues at
						the chosen scope. The issues still record, but no longer trigger or
						join incidents. Manage and un-silence from the detail page.
					</Typography>
					<Stack direction="row" spacing={1}>
						{issue.application_id != null && (
							<GradedAction calls="silenced_refs/silence_server">
								<Button
									variant="outlined"
									size="small"
									startIcon={<NotificationsOffOutlinedIcon />}
									onClick={() =>
										wrap(() =>
											silenceServer.call({
												server_id: issue.application_id,
												source: issue.source,
												ref: issue.ref,
											}),
										).then(() => setSilenceOpen(false))
									}
								>
									For this server
								</Button>
							</GradedAction>
						)}
						{issue.machine_id != null && (
							<GradedAction calls="silenced_refs/silence_machine">
								<Button
									variant="outlined"
									size="small"
									startIcon={<NotificationsOffOutlinedIcon />}
									onClick={() =>
										wrap(() =>
											silenceMachine.call({
												machine_id: issue.machine_id,
												source: issue.source,
												ref: issue.ref,
											}),
										).then(() => setSilenceOpen(false))
									}
								>
									For this machine
								</Button>
							</GradedAction>
						)}
						{issue.server_group_id && (
							<GradedAction calls="silenced_refs/silence_group">
								<Button
									variant="outlined"
									size="small"
									startIcon={<NotificationsOffOutlinedIcon />}
									onClick={() =>
										wrap(() =>
											silenceGroup.call({
												server_group_id: issue.server_group_id!,
												source: issue.source,
												ref: issue.ref,
												// A group spans several application types,
												// so the silence names the one this issue's
												// check belongs to.
												application_type:
													issue.namespace?.application_type ?? null,
											}),
										).then(() => setSilenceOpen(false))
									}
								>
									For this group
								</Button>
							</GradedAction>
						)}
						<Button
							variant="outlined"
							size="small"
							onClick={() => setSilenceOpen(false)}
						>
							Cancel
						</Button>
					</Stack>
				</Stack>
			)}
			{error && (
				<MuiAlert severity="error" sx={{ mt: 1 }}>
					{error.message}
				</MuiAlert>
			)}
		</Box>
	);
}

/** One-line provenance shown just below the title in the expanded body:
 * who reported it (source + ref) and which incident(s) it joined, if any.
 * Each incident is linked by the first 8 hex chars of its UUID.
 *
 * Flappy issues can attach to dozens of incidents — listing them all turns
 * the meta line into a wall of text. For chains of four or more, collapse
 * the middle to `N more times` and only render the two latest plus the
 * earliest. */
function IssueMeta({ issue }: { issue: IssueData }) {
	const incidents = issue.incidents;
	const n = incidents.length;
	const checkName = healthcheckNameFromRef(issue.source, issue.ref);

	return (
		<Typography variant="body2" color="text.secondary" sx={{ mb: 1 }}>
			Issue created by <b>{issue.source}</b> (
			<Box
				component="code"
				sx={{ fontFamily: "monospace", fontSize: "0.9em" }}
			>
				{checkName ? (
					<MuiLink
						component={RouterLink}
						to={healthcheckPath(issue.source, issue.namespace ?? undefined, checkName)}
					>
						{issue.ref}
					</MuiLink>
				) : (
					issue.ref
				)}
			</Box>
			)
			{n > 0 && (
				<>
					, {n} {n === 1 ? "time" : "times"}
					{n <= 3 ? (
						<>
							{", in "}
							{incidents.map((inc, i) => (
								<Fragment key={inc.incident_id}>
									{i > 0 && ", "}
									<IncidentRef inc={inc} />
								</Fragment>
							))}
						</>
					) : (
						<>
							{", latest in "}
							<IncidentRef inc={incidents[0]} />,{" "}
							<IncidentRef inc={incidents[1]} />, {n - 3} more{" "}
							{n - 3 === 1 ? "time" : "times"}, earliest in{" "}
							<IncidentRef inc={incidents[n - 1]} />
						</>
					)}
				</>
			)}
		</Typography>
	);
}

function IncidentRef({ inc }: { inc: IssueIncidentLink }) {
	return (
		<>
			<MuiLink
				component={RouterLink}
				to={`/incidents/${inc.incident_id}`}
			>
				incident {inc.incident_id.slice(0, 8)}
			</MuiLink>{" "}
			(opened <TimeAgo timestamp={inc.opened_at} />
			{inc.closed_at && (
				<>
					, closed <TimeAgo timestamp={inc.closed_at} />
				</>
			)}
			)
		</>
	);
}
