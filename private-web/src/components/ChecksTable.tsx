//! The checks a target carries, and the headline health those checks roll up
//! to.
//!
//! Shared by the two detail pages rather than written twice: a check filed
//! against a machine and one filed against an application are the same shape,
//! graded the same way, and silenced through the same control. Only the target
//! the silence names differs, which is what `scope` and `targetId` carry.

import {
	Alert,
	Box,
	Button,
	Chip,
	IconButton,
	Link as MuiLink,
	Popover,
	Stack,
	Tooltip,
	Typography,
} from "@mui/material";
import BuildCircleIcon from "@mui/icons-material/BuildCircle";
import CancelIcon from "@mui/icons-material/Cancel";
import CheckCircleIcon from "@mui/icons-material/CheckCircle";
import NotificationsActiveOutlinedIcon from "@mui/icons-material/NotificationsActiveOutlined";
import NotificationsOffIcon from "@mui/icons-material/NotificationsOff";
import NotificationsOffOutlinedIcon from "@mui/icons-material/NotificationsOffOutlined";
import RemoveCircleOutlinedIcon from "@mui/icons-material/RemoveCircleOutlined";
import WarningAmberIcon from "@mui/icons-material/WarningAmber";
import { useState } from "react";
import { Link as RouterLink } from "react-router-dom";
import { useApi, useApiAction } from "../api";
import { useIsAdmin } from "../hooks/useIsAdmin";
import CheckDocButton from "./CheckDocButton";
import CheckExtrasList, { checkEntryExtras } from "./CheckExtras";
import ExternalUsersDetails, {
	parseExternalUserSessions,
} from "./ExternalUsersDetails";
import HealthChip from "./HealthChip";
import OperatorAvatars from "./OperatorAvatars";
import TimeAgo from "./TimeAgo";
import {
	healthcheckPath,
	sameNamespace,
	silenceRef,
	type CheckResult,
	type ConsolidatedCheck,
	type ConsolidatedChecks,
	type HealthState,
	type NamespaceRef,
	type OperatorPresence,
	type ServerGroupSilencedRef,
	type ShortStatus,
} from "../types";

export function HealthIndicator({
	health,
	up,
	monitored,
	maintained,
	maintenanceSettling,
	operators,
}: {
	health: HealthState;
	up: ShortStatus;
	monitored: boolean;
	maintained: boolean;
	maintenanceSettling: boolean;
	operators: OperatorPresence[];
}) {
	const reporting = up === "up";
	return (
		<Stack
			direction="row"
			spacing={2}
			useFlexGap
			sx={{ mb: 1.5, alignItems: "center", flexWrap: "wrap" }}
		>
			<HealthChip
				health={health}
				stale={!reporting}
				monitored={monitored}
				maintained={maintained}
				maintenanceSettling={maintenanceSettling}
				maintenanceHref="#maintenance"
			/>
			{reporting && operators.length > 0 && (
				<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
					<OperatorAvatars operators={operators} size={24} />
					<Typography variant="body2">
						{operators.length} operator
						{operators.length === 1 ? "" : "s"} in the machine right
						now
					</Typography>
				</Stack>
			)}
		</Stack>
	);
}

/** Consolidated per-check table: every source's current checks, graded
 * and sorted most-urgent-first by the backend. Capped at 5 visible rows
 * with an "expand all" toggle so a server reporting 30 checks doesn't
 * push the rest of the page off-screen. Render nothing when there are no
 * checks to show.
 *
 * Each entry already carries its own `silenced` flag (from the same
 * scoped-policy pass the health rollup uses); the silenced-refs fetch
 * here only feeds the manage buttons and the "silenced at N scope" chip.
 * Splits into a grouped/ungrouped variant only to keep the group-scope
 * silenced-refs fetch off ungrouped servers — `useApi` is unconditional,
 * so a single component can't gate the hook on `groupId`. */
/** What these checks are filed against. A check is silenced at its own target
 * and at that target's group, so the table needs to know which grain it is
 * presenting to offer the right pair.
 * spec: CHK#silences-follow-the-event */
export type CheckTarget =
	| { kind: "application"; id: string }
	| { kind: "machine"; id: string };

/** A silence as this table reads one, whichever scope it came from. */
type Silence = { source: string; ref: string; created_at: string; created_by: string | null };

export function ChecksTable(props: {
	checks: ConsolidatedChecks;
	operators: OperatorPresence[];
	target: CheckTarget;
	/** The box under an application, whose checks the list also carries. Leave
	 * unset on a machine's own table, where the target is already the box. */
	machineId?: string | null;
	groupId: string | null;
	refreshTick: number;
	onSilenced: () => void;
}) {
	// An application's list carries its box's checks too, so both grains'
	// silences are read: a machine check is silenced on the machine wherever it
	// is presented from.
	// spec: CHK#a-machines-checks-present-on-its-applications
	const machineId =
		props.target.kind === "machine" ? props.target.id : (props.machineId ?? "");
	const applicationApi = useApi(
		"silenced_refs",
		"list_for_server",
		{ server_id: props.target.kind === "application" ? props.target.id : "" },
		[props.target.kind, props.target.id, props.refreshTick],
	);
	const machineApi = useApi(
		"silenced_refs",
		"list_for_machine",
		{ machine_id: machineId },
		[machineId, props.refreshTick],
	);
	const applicationSilences: Silence[] =
		applicationApi.status === "ok" ? applicationApi.data : [];
	const machineSilences: Silence[] =
		machineApi.status === "ok" ? machineApi.data : [];
	const ownSilences =
		props.target.kind === "application" ? applicationSilences : machineSilences;
	if (props.groupId) {
		return (
			<ChecksTableGrouped
				{...props}
				groupId={props.groupId}
				ownSilences={ownSilences}
				machineSilences={machineSilences}
			/>
		);
	}
	return (
		<ChecksTableBody
			{...props}
			ownSilences={ownSilences}
			machineSilences={machineSilences}
			groupSilences={[]}
		/>
	);
}

function ChecksTableGrouped(props: {
	checks: ConsolidatedChecks;
	operators: OperatorPresence[];
	target: CheckTarget;
	machineId?: string | null;
	groupId: string;
	refreshTick: number;
	onSilenced: () => void;
	ownSilences: Silence[];
	machineSilences: Silence[];
}) {
	const groupApi = useApi(
		"silenced_refs",
		"list_for_group",
		{ server_group_id: props.groupId },
		[props.groupId, props.refreshTick],
	);
	const groupSilences = groupApi.status === "ok" ? groupApi.data : [];
	return <ChecksTableBody {...props} groupSilences={groupSilences} />;
}

function ChecksTableBody({
	checks,
	operators,
	target,
	machineId,
	groupId,
	onSilenced,
	ownSilences,
	machineSilences,
	groupSilences,
}: {
	checks: ConsolidatedChecks;
	operators: OperatorPresence[];
	target: CheckTarget;
	machineId?: string | null;
	groupId: string | null;
	onSilenced: () => void;
	ownSilences: Silence[];
	machineSilences: Silence[];
	groupSilences: ServerGroupSilencedRef[];
}) {
	const entries = checks.checks;
	const [expanded, setExpanded] = useState(false);
	if (entries.length === 0) return null;
	const HIDE_AFTER = 5;
	const visible = expanded ? entries : entries.slice(0, HIDE_AFTER);
	const hidden = entries.length - visible.length;
	return (
		<Box sx={{ mt: 2 }}>
			<Typography variant="overline" color="text.secondary">
				Checks ({entries.length})
			</Typography>
			<Stack spacing={1} sx={{ mt: 0.5 }}>
				{visible.map((entry) => {
					// A machine check in an application's list is the box's,
					// one filing seen from each workload on it. Its silence
					// control acts on the machine, so the row's target is the
					// box rather than the table's.
					// spec: CHK#a-machines-checks-present-on-its-applications
					const fromMachine =
						target.kind === "application" &&
						entry.subject === "machine" &&
						!!machineId;
					const rowTarget: CheckTarget = fromMachine
						? { kind: "machine", id: machineId as string }
						: target;
					const rowOwnSilences = fromMachine ? machineSilences : ownSilences;
					// Match the silence refs to this entry's own source — a
					// silence on another source's same-named check is a
					// different check, and canopy's own checks are silenced
					// at a bare ref rather than under `health/`.
					const refName = silenceRef(entry.source, entry.check);
					const ownSilence =
						rowOwnSilences.find(
							(s) => s.source === entry.source && s.ref === refName,
						) ?? null;
					// A group covers several application types, so its silences
					// are matched on the namespace too: the same check name
					// silenced for another type is another check.
					const groupSilence =
						groupSilences.find(
							(s) =>
								s.source === entry.source &&
								s.ref === refName &&
								sameNamespace(s.namespace, entry.namespace),
						) ?? null;
					return (
						<CheckRow
							key={`${entry.subject}:${entry.source}:${entry.check}`}
							entry={entry}
							operators={operators}
							target={rowTarget}
							fromMachine={fromMachine}
							groupId={groupId}
							onSilenced={onSilenced}
							ownSilence={ownSilence}
							groupSilence={groupSilence}
						/>
					);
				})}
			</Stack>
			{hidden > 0 && (
				<Button
					size="small"
					onClick={() => setExpanded(true)}
					sx={{ mt: 0.5 }}
				>
					Show {hidden} more
				</Button>
			)}
			{expanded && entries.length > HIDE_AFTER && (
				<Button
					size="small"
					onClick={() => setExpanded(false)}
					sx={{ mt: 0.5 }}
				>
					Collapse
				</Button>
			)}
		</Box>
	);
}

function CheckRow({
	entry,
	operators,
	target,
	fromMachine,
	groupId,
	onSilenced,
	ownSilence,
	groupSilence,
}: {
	entry: ConsolidatedCheck;
	operators: OperatorPresence[];
	target: CheckTarget;
	/** True when this row is the box's check shown on an application. */
	fromMachine: boolean;
	groupId: string | null;
	onSilenced: () => void;
	ownSilence: Silence | null;
	groupSilence: ServerGroupSilencedRef | null;
}) {
	const isAdmin = useIsAdmin() === true;
	// `external_users` gets a formatted session list instead of the raw
	// `users` JSON; the headline `count` is subsumed by it too. Falls
	// through to the generic dl when the payload shape is unexpected.
	const allExtras = checkEntryExtras(
		(entry.detail ?? {}) as Record<string, unknown>,
	);
	const sessions =
		entry.check === "external_users"
			? parseExternalUserSessions(allExtras)
			: null;
	const extras =
		sessions === null
			? allExtras
			: allExtras.filter(([k]) => k !== "users" && k !== "count");
	const effective = entry.effective as CheckResult;
	const quiet =
		entry.silenced || effective === "passed" || effective === "skipped";
	return (
		<Stack
			direction="row"
			spacing={1.5}
			sx={{
				p: 1,
				border: 1,
				borderColor: "divider",
				borderRadius: 1,
				alignItems: "flex-start",
				bgcolor: quiet ? undefined : "action.hover",
			}}
		>
			<CheckResultIcon
				observed={entry.observed as CheckResult | null}
				effective={effective}
				silenced={entry.silenced}
			/>
			<Box sx={{ flex: 1, minWidth: 0 }}>
				<Stack
					direction="row"
					spacing={1}
					sx={{ alignItems: "center", flexWrap: "wrap" }}
					useFlexGap
				>
					<Typography variant="body2" sx={{ fontFamily: "monospace" }}>
						<MuiLink
							component={RouterLink}
							to={healthcheckPath(entry.source, entry.namespace, entry.check)}
						>
							{entry.qualified_name}
						</MuiLink>
					</Typography>
					<Typography variant="caption" color="text.secondary">
						{entry.source}
					</Typography>
					<CheckDocButton
						source={entry.source}
						namespace={entry.namespace}
						check={entry.check}
					/>
					{fromMachine && <MachineCheckChip />}
					<SilencedChip
						targetKind={target.kind}
						ownSilence={ownSilence}
						groupSilence={groupSilence}
					/>
				</Stack>
				{sessions !== null && (
					<ExternalUsersDetails
						sessions={sessions}
						operators={operators}
					/>
				)}
				<CheckExtrasList extras={extras} />
			</Box>
			{isAdmin && (
				<SilenceCheckButton
					check={entry.check}
					namespace={entry.namespace}
					target={target}
					groupId={groupId}
					source={entry.source}
					onSilenced={onSilenced}
					ownSilence={ownSilence}
					groupSilence={groupSilence}
				/>
			)}
		</Stack>
	);
}

/** Per-check result icon, coloured by the check's *effective* result
 * (what policy grades it to). A silenced check gets the same neutral
 * grey treatment as a skipped one — its result still records, it just
 * doesn't count toward the server's health. When the observed result
 * differs from the effective one, the tooltip notes the grading. */
function CheckResultIcon({
	observed,
	effective,
	silenced = false,
}: {
	observed: CheckResult | null;
	effective: CheckResult;
	silenced?: boolean;
}) {
	if (silenced) {
		return (
			<Tooltip
				title={`Silenced — reported ${observed ?? "?"}, not counted toward server health`}
				arrow
			>
				<NotificationsOffIcon fontSize="small" color="disabled" />
			</Tooltip>
		);
	}
	const DESCRIPTION: Record<CheckResult, string> = {
		passed: "Passing",
		warning: "Warning — degraded but not failing",
		failed: "Failing",
		broken: "Broken — the check itself is failing, not the system under test",
		skipped: "Skipped — a precondition was not met",
	};
	const tooltip =
		observed && observed !== effective
			? `${DESCRIPTION[effective]} (reported ${observed}, graded ${effective})`
			: DESCRIPTION[effective];
	switch (effective) {
		case "passed":
			return (
				<Tooltip title={tooltip} arrow>
					<CheckCircleIcon fontSize="small" color="success" />
				</Tooltip>
			);
		case "warning":
			return (
				<Tooltip title={tooltip} arrow>
					<WarningAmberIcon fontSize="small" color="warning" />
				</Tooltip>
			);
		case "failed":
			return (
				<Tooltip title={tooltip} arrow>
					<CancelIcon fontSize="small" color="error" />
				</Tooltip>
			);
		case "broken":
			return (
				<Tooltip title={tooltip} arrow>
					<BuildCircleIcon fontSize="small" color="warning" />
				</Tooltip>
			);
		case "skipped":
			return (
				<Tooltip title={tooltip} arrow>
					<RemoveCircleOutlinedIcon fontSize="small" color="disabled" />
				</Tooltip>
			);
	}
}

/** Inline indicator showing that a check's `(status, health/<check>)` ref
 * is already in the silence list at one or both scopes. Shown for all
 * viewers (silences are listable without admin); the row's silence
 * button still gates the manage actions on admin. */

/** Marks a row as the box's rather than the workload's. An application's list
 * carries both, and which grain a check speaks for decides where it is
 * silenced and what a failure implicates.
 * spec: CHK#a-machines-checks-present-on-its-applications */
function MachineCheckChip() {
	return (
		<Tooltip title="Filed against the machine this runs on, and shared with every application on it.">
			<Chip
				size="small"
				variant="outlined"
				label="machine"
				data-testid="check-machine-subject"
			/>
		</Tooltip>
	);
}

function SilencedChip({
	targetKind,
	ownSilence,
	groupSilence,
}: {
	targetKind: CheckTarget["kind"];
	ownSilence: Silence | null;
	groupSilence: ServerGroupSilencedRef | null;
}) {
	if (!ownSilence && !groupSilence) return null;
	const scopes: string[] = [];
	if (ownSilence) scopes.push(targetKind);
	if (groupSilence) scopes.push("group");
	const tooltipLines: string[] = [];
	if (ownSilence) {
		tooltipLines.push(
			`${targetKind === "machine" ? "Machine" : "Application"}-scope silence${
				ownSilence.created_by ? ` by ${ownSilence.created_by}` : ""
			}`,
		);
	}
	if (groupSilence) {
		tooltipLines.push(
			`Group-scope silence${
				groupSilence.created_by ? ` by ${groupSilence.created_by}` : ""
			}`,
		);
	}
	return (
		<Tooltip title={tooltipLines.join(" · ")}>
			<Chip
				size="small"
				variant="outlined"
				icon={<NotificationsOffIcon />}
				label={`silenced (${scopes.join(" + ")})`}
			/>
		</Tooltip>
	);
}

/** Compact silence trigger on each `CheckRow`. Opens a popover that
 * shows, per scope, either the existing silence (with an Un-silence
 * action) or a Silence button. Filled icon + primary colour signals that
 * the row is already silenced at one or both scopes — operators can spot
 * "this check is covered" without opening the popover. On any mutation,
 * calls the parent's `onSilenced` so the `ChecksTable`'s silence fetches
 * and the page's `SilencedRefsSection` refetch in lockstep. */
function SilenceCheckButton({
	check,
	namespace,
	target,
	groupId,
	source,
	onSilenced,
	ownSilence,
	groupSilence,
}: {
	check: string;
	namespace: NamespaceRef;
	target: CheckTarget;
	groupId: string | null;
	source: string;
	onSilenced: () => void;
	ownSilence: Silence | null;
	groupSilence: ServerGroupSilencedRef | null;
}) {
	const silenceServer = useApiAction("silenced_refs", "silence_server");
	const silenceMachine = useApiAction("silenced_refs", "silence_machine");
	const silenceGroup = useApiAction("silenced_refs", "silence_group");
	const unsilenceServer = useApiAction("silenced_refs", "unsilence_server");
	const unsilenceMachine = useApiAction("silenced_refs", "unsilence_machine");
	const unsilenceGroup = useApiAction("silenced_refs", "unsilence_group");
	const [anchorEl, setAnchorEl] = useState<HTMLElement | null>(null);
	const error =
		silenceServer.error ??
		silenceMachine.error ??
		silenceGroup.error ??
		unsilenceServer.error ??
		unsilenceMachine.error ??
		unsilenceGroup.error;
	const refName = silenceRef(source, check);
	const silenced = !!ownSilence || !!groupSilence;
	const handle = async (fn: () => Promise<unknown>) => {
		try {
			await fn();
			onSilenced();
			setAnchorEl(null);
		} catch {
			/* surfaced via error */
		}
	};
	return (
		<>
			<Tooltip
				title={silenced ? "Silenced — manage…" : "Silence this check…"}
			>
				<IconButton
					size="small"
					color={silenced ? "primary" : "default"}
					aria-label={
						silenced
							? `Manage silence for ${check}`
							: `Silence ${check}`
					}
					onClick={(e) => setAnchorEl(e.currentTarget)}
				>
					{silenced ? (
						<NotificationsOffIcon fontSize="small" />
					) : (
						<NotificationsOffOutlinedIcon fontSize="small" />
					)}
				</IconButton>
			</Tooltip>
			<Popover
				open={!!anchorEl}
				anchorEl={anchorEl}
				onClose={() => setAnchorEl(null)}
				anchorOrigin={{ vertical: "bottom", horizontal: "right" }}
				transformOrigin={{ vertical: "top", horizontal: "right" }}
			>
				<Box sx={{ p: 1.5, maxWidth: 360 }}>
					<Typography variant="body2" color="text.secondary" sx={{ mb: 1 }}>
						Permanently ignore <code>
							{source}/{refName}
						</code>
						. The check still records, but no longer triggers or joins
						incidents.
					</Typography>
					<Stack spacing={0.75}>
						<SilenceScopeRow
							scopeLabel={
								target.kind === "machine" ? "this machine" : "this server"
							}
							silence={ownSilence}
							onSilence={() =>
								handle(() =>
									target.kind === "machine"
										? silenceMachine.call({
												machine_id: target.id,
												source,
												ref: refName,
											})
										: silenceServer.call({
												server_id: target.id,
												source,
												ref: refName,
											}),
								)
							}
							onUnsilence={() =>
								handle(() =>
									target.kind === "machine"
										? unsilenceMachine.call({
												machine_id: target.id,
												source,
												ref: refName,
											})
										: unsilenceServer.call({
												server_id: target.id,
												source,
												ref: refName,
											}),
								)
							}
						/>
						{groupId && (
							<SilenceScopeRow
								scopeLabel="this group"
								silence={groupSilence}
								onSilence={() =>
									handle(() =>
										silenceGroup.call({
											server_group_id: groupId,
											source,
											ref: refName,
											application_type:
												namespace.application_type,
										}),
									)
								}
								onUnsilence={() =>
									handle(() =>
										unsilenceGroup.call({
											server_group_id: groupId,
											source,
											ref: refName,
											application_type:
												namespace.application_type,
										}),
									)
								}
							/>
						)}
					</Stack>
					{error && (
						<Alert severity="error" sx={{ mt: 1 }}>
							{error.message}
						</Alert>
					)}
				</Box>
			</Popover>
		</>
	);
}

/** One row in the silence-check popover, scoped to either the server or
 * the group. Renders an Un-silence button (with provenance) when the
 * scope already has a silence for this ref, or a Silence button when it
 * doesn't. */
function SilenceScopeRow({
	scopeLabel,
	silence,
	onSilence,
	onUnsilence,
}: {
	scopeLabel: string;
	silence: { created_at: string; created_by: string | null } | null;
	onSilence: () => void;
	onUnsilence: () => void;
}) {
	if (silence) {
		return (
			<Stack
				direction="row"
				spacing={1}
				sx={{ alignItems: "center", flexWrap: "wrap" }}
				useFlexGap
			>
				<Typography variant="caption" sx={{ flex: 1, minWidth: 0 }}>
					Silenced for {scopeLabel}
					<Box component="span" sx={{ color: "text.secondary" }}>
						{" — "}
						<TimeAgo timestamp={silence.created_at} />
						{silence.created_by && ` by ${silence.created_by}`}
					</Box>
				</Typography>
				<Button
					size="small"
					variant="outlined"
					startIcon={<NotificationsActiveOutlinedIcon />}
					onClick={onUnsilence}
				>
					Un-silence
				</Button>
			</Stack>
		);
	}
	return (
		<Button
			size="small"
			variant="outlined"
			startIcon={<NotificationsOffOutlinedIcon />}
			onClick={onSilence}
			sx={{ alignSelf: "flex-start" }}
		>
			For {scopeLabel}
		</Button>
	);
}

