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
import type { GradedEndpoint } from "../safety-modes";
import CheckDocButton from "./CheckDocButton";
import CheckExtrasList, {
	checkEntryExtras,
	renderCheckValue,
} from "./CheckExtras";
import { type Calls, GradedAction } from "./GradedAction";
import ExternalUsersDetails, {
	parseExternalUserSessions,
} from "./ExternalUsersDetails";
import HealthChip from "./HealthChip";
import InstanceName from "./InstanceName";
import OperatorAvatars from "./OperatorAvatars";
import TimeAgo from "./TimeAgo";
import {
	healthcheckPath,
	instanceName,
	sameNamespace,
	silenceRef,
	type CheckResult,
	type ConsolidatedCheck,
	type ConsolidatedChecks,
	type ConsolidatedInstance,
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

/** Consolidated per-check table: the target's own current checks across
 * every source, graded and ordered by the backend (most urgent first, then
 * by presented name). Capped at 5 visible rows
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
	| { kind: "machine"; id: string }
	| { kind: "cluster"; id: string };

/** A silence as this table reads one, whichever scope it came from. */
type Silence = {
	source: string;
	ref: string;
	instance?: string | null;
	created_at: string;
	created_by: string | null;
};

export function ChecksTable(props: {
	checks: ConsolidatedChecks;
	operators: OperatorPresence[];
	target: CheckTarget;
	groupId: string | null;
	refreshTick: number;
	onSilenced: () => void;
}) {
	const applicationApi = useApi(
		"silenced_refs",
		"list_for_server",
		{ server_id: props.target.kind === "application" ? props.target.id : "" },
		[props.target.kind, props.target.id, props.refreshTick],
		{ skip: props.target.kind !== "application" },
	);
	const machineApi = useApi(
		"silenced_refs",
		"list_for_machine",
		{ machine_id: props.target.kind === "machine" ? props.target.id : "" },
		[props.target.kind, props.target.id, props.refreshTick],
		{ skip: props.target.kind !== "machine" },
	);
	// A cluster belongs to no group, so its own silences are all it has.
	// spec: CHK#silences-follow-the-event
	const clusterApi = useApi(
		"silenced_refs",
		"list_for_cluster",
		{ kubernetes_cluster_id: props.target.id },
		[props.target.kind, props.target.id, props.refreshTick],
		{ skip: props.target.kind !== "cluster" },
	);
	const applicationSilences: Silence[] =
		applicationApi.status === "ok" ? applicationApi.data : [];
	const machineSilences: Silence[] =
		machineApi.status === "ok" ? machineApi.data : [];
	const clusterSilences: Silence[] =
		clusterApi.status === "ok" ? clusterApi.data : [];
	const ownSilences =
		props.target.kind === "application"
			? applicationSilences
			: props.target.kind === "cluster"
				? clusterSilences
				: machineSilences;
	if (props.groupId) {
		return (
			<ChecksTableGrouped
				{...props}
				groupId={props.groupId}
				ownSilences={ownSilences}
			/>
		);
	}
	return (
		<ChecksTableBody {...props} ownSilences={ownSilences} groupSilences={[]} />
	);
}

function ChecksTableGrouped(props: {
	checks: ConsolidatedChecks;
	operators: OperatorPresence[];
	target: CheckTarget;
	groupId: string;
	refreshTick: number;
	onSilenced: () => void;
	ownSilences: Silence[];
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
	groupId,
	onSilenced,
	ownSilences,
	groupSilences,
}: {
	checks: ConsolidatedChecks;
	operators: OperatorPresence[];
	target: CheckTarget;
	groupId: string | null;
	onSilenced: () => void;
	ownSilences: Silence[];
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
					// Match the silence refs to this entry's own source — a
					// silence on another source's same-named check is a
					// different check, and canopy's own checks are silenced
					// at a bare ref rather than under `health/`.
					const refName = silenceRef(entry.source, entry.check);
					const rowSilences = ownSilences.filter(
						(s) => s.source === entry.source && s.ref === refName,
					);
					// A group covers several application types, so its silences
					// are matched on the namespace too: the same check name
					// silenced for another type is another check.
					const rowGroupSilences = groupSilences.filter(
						(s) =>
							s.source === entry.source &&
							s.ref === refName &&
							sameNamespace(s.namespace, entry.namespace),
					);
					// An instance silence quiets one instance, not the row's check.
					// spec: CHK#silencing-one-instance
					const ownSilence = rowSilences.find((s) => !s.instance) ?? null;
					const groupSilence =
						rowGroupSilences.find((s) => !s.instance) ?? null;
					return (
						<CheckRow
							key={`${entry.source}:${entry.qualified_name}`}
							entry={entry}
							operators={operators}
							target={target}
							groupId={groupId}
							onSilenced={onSilenced}
							ownSilence={ownSilence}
							groupSilence={groupSilence}
							ownInstanceSilences={rowSilences.filter((s) => !!s.instance)}
							groupInstanceSilences={rowGroupSilences.filter(
								(s) => !!s.instance,
							)}
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
	groupId,
	onSilenced,
	ownSilence,
	groupSilence,
	ownInstanceSilences,
	groupInstanceSilences,
}: {
	entry: ConsolidatedCheck;
	operators: OperatorPresence[];
	target: CheckTarget;
	groupId: string | null;
	onSilenced: () => void;
	ownSilence: Silence | null;
	groupSilence: ServerGroupSilencedRef | null;
	/** This check's instance silences at the row's own target. */
	ownInstanceSilences: Silence[];
	/** This check's instance silences at the target's group. */
	groupInstanceSilences: ServerGroupSilencedRef[];
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
	const calm =
		entry.silenced || effective === "passed" || effective === "skipped";
	// A source gone quiet leaves its checks at their last result. Muted and
	// aged, so a stale pass never reads as a live one.
	// spec: CHK#presentation
	const muted = entry.quiet ? { opacity: 0.5 } : undefined;
	return (
		<Stack
			direction="row"
			spacing={1.5}
			data-testid="check-row"
			data-quiet={entry.quiet ? "true" : undefined}
			sx={{
				p: 1,
				border: 1,
				borderColor: "divider",
				borderRadius: 1,
				alignItems: "flex-start",
				bgcolor: calm ? undefined : "action.hover",
			}}
		>
			<Box sx={{ display: "flex", ...muted }}>
				<CheckResultIcon
					observed={entry.observed as CheckResult | null}
					effective={effective}
					silenced={entry.silenced}
				/>
			</Box>
			<Box sx={{ flex: 1, minWidth: 0, ...muted }}>
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
					{entry.quiet && entry.last_reported_at && (
						<Typography variant="caption" color="text.secondary">
							last reported <TimeAgo timestamp={entry.last_reported_at} />
						</Typography>
					)}
					<CheckDocButton
						source={entry.source}
						namespace={entry.namespace}
						check={entry.check}
					/>
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
				<InstanceList
					entry={entry}
					target={target}
					groupId={groupId}
					onSilenced={onSilenced}
					isAdmin={isAdmin}
					ownSilences={ownInstanceSilences}
					groupSilences={groupInstanceSilences}
				/>
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

/** An instanced check's degraded and silenced instances, each with its own
 * result and silence control, and a count of the rest. Renders nothing for a
 * check without instances.
 * spec: CHK#silencing-one-instance */
function InstanceList({
	entry,
	target,
	groupId,
	onSilenced,
	isAdmin,
	ownSilences,
	groupSilences,
}: {
	entry: ConsolidatedCheck;
	target: CheckTarget;
	groupId: string | null;
	onSilenced: () => void;
	isAdmin: boolean;
	ownSilences: Silence[];
	groupSilences: ServerGroupSilencedRef[];
}) {
	const counts = [
		entry.passing_instances > 0 && `${entry.passing_instances} passing`,
		entry.skipped_instances > 0 && `${entry.skipped_instances} skipped`,
	].filter(Boolean);
	if (entry.instances.length === 0 && counts.length === 0) return null;
	return (
		<Stack sx={{ mt: 1 }}>
			{entry.instances.map((instance) => (
				<InstanceRow
					key={instance.key}
					entry={entry}
					instance={instance}
					target={target}
					groupId={groupId}
					onSilenced={onSilenced}
					isAdmin={isAdmin}
					ownSilence={
						ownSilences.find((s) => s.instance === instance.key) ?? null
					}
					groupSilence={
						groupSilences.find((s) => s.instance === instance.key) ?? null
					}
				/>
			))}
			{counts.length > 0 && (
				<Typography
					variant="caption"
					color="text.secondary"
					sx={{ pt: 0.5, pl: 3.5 }}
				>
					{counts.join(", ")}
				</Typography>
			)}
		</Stack>
	);
}

function InstanceRow({
	entry,
	instance,
	target,
	groupId,
	onSilenced,
	isAdmin,
	ownSilence,
	groupSilence,
}: {
	entry: ConsolidatedCheck;
	instance: ConsolidatedInstance;
	target: CheckTarget;
	groupId: string | null;
	onSilenced: () => void;
	isAdmin: boolean;
	ownSilence: Silence | null;
	groupSilence: ServerGroupSilencedRef | null;
}) {
	const silenced = instance.silenced_on_target || instance.silenced_on_group;
	const facts = Object.entries(
		(instance.detail ?? {}) as Record<string, unknown>,
	)
		.map(([k, v]) => `${k} ${renderCheckValue(v)}`)
		.join(" · ");
	return (
		<Stack
			direction="row"
			spacing={1}
			data-testid="check-instance"
			sx={{ alignItems: "center", py: 0.25, minWidth: 0 }}
		>
			<CheckResultIcon
				observed={instance.observed as CheckResult}
				effective={instance.effective as CheckResult}
				silenced={silenced}
			/>
			<InstanceName
				instanceKey={instance.key}
				label={instance.label}
				quiet={silenced}
			/>
			<SilencedChip
				targetKind={target.kind}
				ownSilence={ownSilence}
				groupSilence={groupSilence}
			/>
			<Box sx={{ flex: 1 }} />
			{facts && (
				<Typography
					variant="caption"
					color="text.secondary"
					sx={{ fontFamily: "monospace", overflowWrap: "anywhere" }}
				>
					{facts}
				</Typography>
			)}
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
					instance={instance}
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
			`${TARGET_LABEL[targetKind]}-scope silence${
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

const TARGET_LABEL: Record<CheckTarget["kind"], string> = {
	application: "Application",
	machine: "Machine",
	cluster: "Cluster",
};

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
	instance,
}: {
	check: string;
	namespace: NamespaceRef;
	target: CheckTarget;
	groupId: string | null;
	source: string;
	onSilenced: () => void;
	ownSilence: Silence | null;
	groupSilence: ServerGroupSilencedRef | null;
	/** The one instance this control silences; unset for the whole check.
	 * spec: CHK#silencing-one-instance */
	instance?: { key: string; label: string | null };
}) {
	const silenceServer = useApiAction("silenced_refs", "silence_server");
	const silenceMachine = useApiAction("silenced_refs", "silence_machine");
	const silenceGroup = useApiAction("silenced_refs", "silence_group");
	const unsilenceServer = useApiAction("silenced_refs", "unsilence_server");
	const unsilenceMachine = useApiAction("silenced_refs", "unsilence_machine");
	const unsilenceGroup = useApiAction("silenced_refs", "unsilence_group");
	const silenceCluster = useApiAction("silenced_refs", "silence_cluster");
	const unsilenceCluster = useApiAction("silenced_refs", "unsilence_cluster");
	const [anchorEl, setAnchorEl] = useState<HTMLElement | null>(null);
	const error =
		silenceServer.error ??
		silenceMachine.error ??
		silenceGroup.error ??
		silenceCluster.error ??
		unsilenceServer.error ??
		unsilenceMachine.error ??
		unsilenceGroup.error ??
		unsilenceCluster.error;
	const refName = silenceRef(source, check);
	const silenced = !!ownSilence || !!groupSilence;
	const name = instance ? instanceName(instance) : null;
	const instanceArg = instance ? { instance: instance.key } : {};
	const ownSilenceCall =
		target.kind === "machine"
			? "silenced_refs/silence_machine"
			: target.kind === "cluster"
				? "silenced_refs/silence_cluster"
				: "silenced_refs/silence_server";
	const ownUnsilenceCall =
		target.kind === "machine"
			? "silenced_refs/unsilence_machine"
			: target.kind === "cluster"
				? "silenced_refs/unsilence_cluster"
				: "silenced_refs/unsilence_server";
	const offered: GradedEndpoint[] = [
		ownSilence ? ownUnsilenceCall : ownSilenceCall,
	];
	if (groupId) {
		offered.push(
			groupSilence
				? "silenced_refs/unsilence_group"
				: "silenced_refs/silence_group",
		);
	}
	// Each row in the popover is graded on its own, so opening it needs only the
	// lowest of them: un-silencing is write even where silencing is danger.
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
			<GradedAction opens={offered}>
				<Tooltip
					title={
						silenced
							? "Silenced — manage…"
							: instance
								? "Silence this instance…"
								: "Silence this check…"
					}
				>
					<IconButton
						size="small"
						aria-label={
							silenced
								? `Manage silence for ${name ? `${name} in ` : ""}${check}`
								: `Silence ${name ? `${name} in ` : ""}${check}`
						}
						onClick={(e) => setAnchorEl(e.currentTarget)}
						sx={instance ? { p: 0.5 } : undefined}
					>
						{silenced ? (
							<NotificationsOffIcon
								fontSize="small"
								sx={instance ? { fontSize: 16 } : undefined}
							/>
						) : (
							<NotificationsOffOutlinedIcon
								fontSize="small"
								sx={instance ? { fontSize: 16 } : undefined}
							/>
						)}
					</IconButton>
				</Tooltip>
			</GradedAction>
			<Popover
				open={!!anchorEl}
				anchorEl={anchorEl}
				onClose={() => setAnchorEl(null)}
				anchorOrigin={{ vertical: "bottom", horizontal: "right" }}
				transformOrigin={{ vertical: "top", horizontal: "right" }}
			>
				<Box sx={{ p: 1.5, maxWidth: 360 }}>
					{name ? (
						<Typography variant="body2" color="text.secondary" sx={{ mb: 1 }}>
							Permanently ignore <b>{name}</b> in{" "}
							<code>
								{source}/{refName}
							</code>
							. The instance still records, but no longer triggers or joins
							incidents.
						</Typography>
					) : (
						<Typography variant="body2" color="text.secondary" sx={{ mb: 1 }}>
							Permanently ignore <code>
								{source}/{refName}
							</code>
							. The check still records, but no longer triggers or joins
							incidents.
						</Typography>
					)}
					<Stack spacing={0.75}>
						<SilenceScopeRow
							scopeLabel={
								target.kind === "machine"
									? "this machine"
									: target.kind === "cluster"
										? "this cluster"
										: "this server"
							}
							silence={ownSilence}
							silenceCalls={ownSilenceCall}
							unsilenceCalls={ownUnsilenceCall}
							onSilence={() =>
								handle(() => {
									const args = { source, ref: refName, ...instanceArg };
									switch (target.kind) {
										case "machine":
											return silenceMachine.call({ machine_id: target.id, ...args });
										case "cluster":
											return silenceCluster.call({
												kubernetes_cluster_id: target.id,
												...args,
											});
										case "application":
											return silenceServer.call({ server_id: target.id, ...args });
									}
								})
							}
							onUnsilence={() =>
								handle(() => {
									const args = { source, ref: refName, ...instanceArg };
									switch (target.kind) {
										case "machine":
											return unsilenceMachine.call({ machine_id: target.id, ...args });
										case "cluster":
											return unsilenceCluster.call({
												kubernetes_cluster_id: target.id,
												...args,
											});
										case "application":
											return unsilenceServer.call({ server_id: target.id, ...args });
									}
								})
							}
						/>
						{groupId && (
							<SilenceScopeRow
								scopeLabel="this group"
								silence={groupSilence}
								silenceCalls="silenced_refs/silence_group"
								unsilenceCalls="silenced_refs/unsilence_group"
								onSilence={() =>
									handle(() =>
										silenceGroup.call({
											server_group_id: groupId,
											source,
											ref: refName,
											application_type:
												namespace.application_type,
											...instanceArg,
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
											...instanceArg,
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
	silenceCalls,
	unsilenceCalls,
	onSilence,
	onUnsilence,
}: {
	scopeLabel: string;
	silence: { created_at: string; created_by: string | null } | null;
	silenceCalls: Calls;
	unsilenceCalls: Calls;
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
				<GradedAction calls={unsilenceCalls}>
					<Button
						size="small"
						variant="outlined"
						startIcon={<NotificationsActiveOutlinedIcon />}
						onClick={onUnsilence}
					>
						Un-silence
					</Button>
				</GradedAction>
			</Stack>
		);
	}
	return (
		<GradedAction calls={silenceCalls}>
			<Button
				size="small"
				variant="outlined"
				startIcon={<NotificationsOffOutlinedIcon />}
				onClick={onSilence}
				sx={{ alignSelf: "flex-start" }}
			>
				For {scopeLabel}
			</Button>
		</GradedAction>
	);
}

