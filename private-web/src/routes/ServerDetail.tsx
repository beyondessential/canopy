import {
	Alert,
	Box,
	Button,
	Chip,
	Dialog,
	DialogActions,
	DialogContent,
	DialogContentText,
	DialogTitle,
	LinearProgress,
	Link as MuiLink,
	Paper,
	Stack,
	Typography,
} from "@mui/material";
import ArchiveIcon from "@mui/icons-material/ArchiveOutlined";
import EditIcon from "@mui/icons-material/Edit";
import LanguageIcon from "@mui/icons-material/Language";
import RestoreIcon from "@mui/icons-material/RestoreFromTrash";
import { useState } from "react";
import { Link as RouterLink, useNavigate, useParams } from "react-router-dom";
import ActionButton from "../components/ActionButton";
import ActiveIncidentCard from "../components/ActiveIncidentCard";
import { ChecksTable, HealthIndicator } from "../components/ChecksTable";
import IncidentsLink from "../components/IncidentsLink";
import ManualEventButton from "../components/ManualEventButton";
import ServerCertificatesSection from "../components/ServerCertificatesSection";
import MaintenanceSection from "../components/MaintenanceSection";
import SilencedRefsSection from "../components/SilencedRefsSection";
import GroupTree from "../components/GroupTree";
import TimeAgo from "../components/TimeAgo";
import TimezoneTooltip from "../components/TimezoneTooltip";
import VersionIndicator from "../components/VersionIndicator";
import ApplicationTypeChip from "../components/ApplicationTypeChip";
import {
	useApplicationTypeCaps,
	useApplicationTypeLabel,
} from "../hooks/useApplicationTypes";
import { HealthLegend, StatusLegend, VersionLegend } from "../components/Legends";
import ServerRankChip from "../components/ServerRankChip";
import { useApi, useApiAction } from "../api";
import { useIsAdmin } from "../hooks/useIsAdmin";
import { usePageTitle } from "../hooks/usePageTitle";
import { humanSeconds } from "../lib/humanDuration";
import TargetName from "../components/TargetName";
import {
	applicationName,
	type ConsolidatedChecks,
	incidentTargetName,
	type HealthState,
	type ServerDetailData,
	type ServerGroup,
	type ServerInfo,
	type ServerLastStatusData,
	type ShortStatus,
} from "../types";

export default function ServerDetail() {
	const { id = "" } = useParams<{ id: string }>();
	const detail = useApi(
		"fleet/applications",
		"get_detail",
		{ server_id: id },
		[id],
	);
	const admin = useIsAdmin() === true;
	// Single refresh signal for everything on the page that talks to the
	// issues/incidents APIs. Any mutation (manual-event submit, resolve/
	// snooze on a row, etc.) bumps this so all sibling panels refetch in
	// lockstep — otherwise resolving an issue (which can auto-close an
	// incident) leaves a stale incidents list.
	const [refreshTick, setRefreshTick] = useState(0);
	const bumpRefresh = () => setRefreshTick((t) => t + 1);
	// Single source of truth for the group's open-incident state. Used to
	// label every ManualEventButton on the page identically — a child
	// server's own incidents list is empty (incidents live at the root),
	// so per-button local queries would mislabel.
	const openIncidents = useApi(
		"incidents",
		"list_for_server",
		{ server_id: id, include_closed: false },
		[id, refreshTick],
	);
	const hasOpenIncident =
		openIncidents.status === "ok" && openIncidents.data.length > 0;
	// The group's open incident, called out above the page rather than left to
	// a coloured button in the action row.
	const openIncident =
		openIncidents.status === "ok" && openIncidents.data.length > 0
			? openIncidents.data[0]
			: null;
	usePageTitle(
		detail.status === "ok"
			? applicationName(detail.data.server)
			: "Server",
	);

	if (detail.status === "loading" || detail.status === "idle") {
		return <LinearProgress />;
	}
	if (detail.status === "error") {
		return <Alert severity="error">{detail.error.message}</Alert>;
	}

	const data = detail.data;
	const archived = data.server.archived;
	const registered = data.server.registered_at != null;

	return (
		<Stack spacing={3}>
			<Header
				data={data}
				isAdmin={admin}
				hasOpenIncident={hasOpenIncident}
				refreshTick={refreshTick}
				onEventSubmitted={bumpRefresh}
				onArchived={() => detail.reload()}
			/>
			{openIncident && (
				<ActiveIncidentCard
					incident={openIncident}
					targetName={incidentTargetName(openIncident)}
				/>
			)}
			{archived ? (
				<ArchivedBanner
					serverId={data.server.id}
					isAdmin={admin}
					onRestored={() => detail.reload()}
				/>
			) : (
				!registered && (
					<Alert severity="info">
						This server hasn't checked in yet.{" "}
						<MuiLink
							component={RouterLink}
							to={`/fleet/machines/${data.server.machine_id}`}
						>
							Enrol its machine
						</MuiLink>{" "}
						to start reporting.
					</Alert>
				)
			)}
			<InfoSection
				server={data.server}
				status={data.last_status}
				health={data.health}
				checks={data.checks}
				onSilenced={bumpRefresh}
				up={data.up}
				maintained={data.maintained}
				maintenanceSettling={data.maintenance_settling}
				refreshTick={refreshTick}
			/>
			{data.group && (
				<GroupSection group={data.group} billingLabels={data.billing_labels} />
			)}
			{(data.server.notes || Object.keys(data.server.tags ?? {}).length > 0) && (
				<NotesAndTagsSection
					notes={data.server.notes}
					tags={data.server.tags}
				/>
			)}
			<ServerCertificatesSection serverId={data.server.id} />
			{data.server.display_host && (
				<Paper variant="outlined" sx={{ p: 2 }}>
					<Typography variant="h6" component="h2" gutterBottom>
						URL
					</Typography>
					<MuiLink
						href={data.server.display_host}
						target="_blank"
						rel="noopener noreferrer"
					>
						{data.server.display_host}
					</MuiLink>
				</Paper>
			)}
			<MaintenanceSection
				scope="machine"
				anchor="maintenance"
				id={data.server.machine_id}
				targetLabel={applicationName(data.server)}
				groupId={data.group?.id ?? null}
				groupName={data.group?.name ?? null}
				rank={data.server.rank ?? null}
				onChanged={() => detail.reload()}
			/>
			<SilencedRefsSection
				scope="server"
				id={data.server.id}
				refreshKey={refreshTick}
				onChanged={bumpRefresh}
			/>
			{data.group && (
				<Box>
					<Typography variant="h5" component="h2" gutterBottom>
						{data.group.name}
					</Typography>
					<GroupTree
						machines={data.group_machines}
						applications={data.group_applications}
						currentApplicationId={data.server.id}
					/>
				</Box>
			)}
			<Box>
				<VersionLegend />
				<Box sx={{ mt: 1 }}>
					<StatusLegend />
				</Box>
				<Box sx={{ mt: 1 }}>
					<HealthLegend />
				</Box>
			</Box>
		</Stack>
	);
}

function Header({
	data,
	isAdmin,
	hasOpenIncident,
	refreshTick,
	onEventSubmitted,
	onArchived,
}: {
	data: ServerDetailData;
	isAdmin: boolean;
	hasOpenIncident: boolean;
	refreshTick: number;
	onEventSubmitted: () => void;
	onArchived: () => void;
}) {
	const archived = data.server.archived;
	return (
		<Stack spacing={1.5}>
			<Stack
				direction="row"
				spacing={1}
				sx={{ alignItems: "center", flexWrap: "wrap" }}
				useFlexGap
			>
				{data.server.rank && <ServerRankChip rank={data.server.rank} />}
				<ApplicationTypeChip type={data.server.type} />
				{/* Group, then box, then workload: the trail an operator
				    would walk to get here, and the box is a link because it
				    is the hop the split made necessary. */}
				{/* spec: FLT#navigating-the-two-grains */}
				<Typography variant="h4" component="h1" sx={{ ml: 1 }}>
					<TargetName
						parts={[
							{
								label: data.server.group_name ?? "",
								to: data.server.group_id
									? `/fleet/groups/${data.server.group_id}`
									: null,
							},
							{
								label: data.machine_name ?? "",
								to: `/fleet/machines/${data.server.machine_id}`,
							},
							{ label: applicationName(data.server) },
						]}
					/>
				</Typography>
			</Stack>
			<Stack
				direction="row"
				spacing={1}
				sx={{ alignItems: "center", flexWrap: "wrap" }}
				useFlexGap
			>
				{data.server.display_host && (
					<ActionButton
						href={data.server.display_host}
						icon={<LanguageIcon />}
						label="Open"
						title={data.server.display_host}
					/>
				)}
				<IncidentsLink
					applicationId={data.server.id}
					groupId={data.group?.id ?? null}
					refreshKey={refreshTick}
				/>
				{isAdmin && (
					<>
						<ManualEventButton
							serverId={data.server.id}
							hasOpenIncident={hasOpenIncident}
							onSubmitted={onEventSubmitted}
							action
						/>
						{/* One form per machine, holding this application's own
						    section — so Edit goes to the box rather than to a
						    second form that would answer "where do I edit
						    this" differently. */}
						{/* spec: FLT#groups */}
						<ActionButton
							to={`/fleet/machines/${data.server.machine_id}/edit`}
							icon={<EditIcon />}
							label="Edit"
							color="primary"
						/>
						{!archived && (
							<DeleteServerButton
								serverId={data.server.id}
								serverName={applicationName(data.server)}
								groupId={data.server.group_id ?? null}
								onArchived={onArchived}
							/>
						)}
					</>
				)}
			</Stack>
		</Stack>
	);
}

/// Inline admin action: archive (soft-delete) the server behind a confirm
/// dialog, then navigate back to its group (if it had one) or the server list.
function DeleteServerButton({
	serverId,
	serverName,
	groupId,
	onArchived,
}: {
	serverId: string;
	serverName: string;
	groupId: string | null;
	onArchived: () => void;
}) {
	const navigate = useNavigate();
	const action = useApiAction("fleet/applications", "delete");
	const [open, setOpen] = useState(false);

	const onConfirm = async () => {
		try {
			await action.call({ server_id: serverId });
			setOpen(false);
			onArchived();
			navigate(groupId ? `/fleet/groups/${groupId}` : "/fleet");
		} catch {
			/* surfaced via action.error */
		}
	};

	return (
		<>
			<ActionButton
				color="error"
				icon={<ArchiveIcon />}
				label="Archive"
				onClick={() => setOpen(true)}
			/>
			<Dialog open={open} onClose={() => setOpen(false)} maxWidth="sm" fullWidth>
				<DialogTitle>Archive server?</DialogTitle>
				<DialogContent>
					<DialogContentText>
						Archive <strong>{serverName}</strong>? This soft-deletes the
						server — it stops being monitored and its device is released,
						but its history is kept and it can be restored later.
					</DialogContentText>
					{action.error && (
						<Alert severity="error" sx={{ mt: 2 }}>
							{action.error.message}
						</Alert>
					)}
				</DialogContent>
				<DialogActions>
					<Button onClick={() => setOpen(false)} disabled={action.pending}>
						Cancel
					</Button>
					<Button
						variant="contained"
						color="error"
						onClick={onConfirm}
						disabled={action.pending}
					>
						{action.pending ? "Archiving…" : "Archive"}
					</Button>
				</DialogActions>
			</Dialog>
		</>
	);
}

/// Shown in place of the setup/registration area when a server has been
/// archived. Keeps the rest of the page (history, status, etc.) visible.
function ArchivedBanner({
	serverId,
	isAdmin,
	onRestored,
}: {
	serverId: string;
	isAdmin: boolean;
	onRestored: () => void;
}) {
	const action = useApiAction("fleet/applications", "restore");
	const onRestore = async () => {
		try {
			await action.call({ server_id: serverId });
			onRestored();
		} catch {
			/* surfaced via action.error */
		}
	};
	return (
		<Alert
			severity="warning"
			action={
				isAdmin ? (
					<Button
						color="inherit"
						size="small"
						startIcon={<RestoreIcon />}
						onClick={onRestore}
						disabled={action.pending}
					>
						{action.pending ? "Restoring…" : "Restore"}
					</Button>
				) : undefined
			}
		>
			This server is archived. Its history is preserved below; restore it to
			resume monitoring.
			{action.error && (
				<Box sx={{ mt: 1 }}>{action.error.message}</Box>
			)}
		</Alert>
	);
}

function InfoSection({
	server,
	status,
	health,
	checks,
	onSilenced,
	up,
	maintained,
	maintenanceSettling,
	refreshTick,
}: {
	server: ServerInfo;
	status: ServerLastStatusData | null;
	health: HealthState;
	checks: ConsolidatedChecks;
	onSilenced: () => void;
	up: ShortStatus;
	maintained: boolean;
	maintenanceSettling: boolean;
	refreshTick: number;
}) {
	// Only a publicly-listable type carries a mobile-list entry.
	// spec: APP#public-listing
	const caps = useApplicationTypeCaps(server.type);
	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			{status && (
				<HealthIndicator
					health={health}
					up={up}
					monitored={server.is_monitored !== false}
					maintained={maintained}
					maintenanceSettling={maintenanceSettling}
					operators={status.operators}
				/>
			)}
			<Stack
				direction="row"
				spacing={4}
				useFlexGap
				sx={{ flexWrap: "wrap" }}
			>
				{status && <StatusInfoFields status={status} />}
				{caps?.public_listing === true && (
					<InfoItem
						label="Mobile list"
						value={server.public_name ?? "Not listed"}
					/>
				)}
				{/* The box this workload runs on. Disks, memory, clock and
				    addresses are its facts, not this application's, and a box
				    carrying a second workload is reached the same way.
				    spec: FLT */}
				<InfoItem label="Machine">
					<MuiLink
						component={RouterLink}
						to={`/fleet/machines/${server.machine_id}`}
						variant="body2"
					>
						This box
					</MuiLink>
				</InfoItem>
				<InfoItem
					label="Status alerts"
					value={
						server.is_monitored
							? `After ${humanSeconds(server.alert_when_down_for)}`
							: "Off"
					}
				/>
				{server.cloud != null && (
					<InfoItem
						label="Location"
						value={renderLocation(server)}
					/>
				)}
				{/* Only where something has been granted. "Not permitted" on every
				    server in the fleet advertises a feature that a Canopy instance
				    without DNS zones does not have.
				    spec: DOM#permission-for-a-server-to-manage-its-own-names */}
				{(server.may_manage_dns || server.may_manage_tls) && (
					<InfoItem
						label="Name management"
						value={nameManagementLabel(server)}
					/>
				)}
			</Stack>
			<ChecksTable
				checks={checks}
				operators={status?.operators ?? []}
				target={{ kind: "application", id: server.id }}
				machineId={server.machine_id}
				groupId={server.group_id}
				refreshTick={refreshTick}
				onSilenced={onSilenced}
			/>
			{status && Object.keys((status.extra ?? {}) as Record<string, unknown>).length > 0 && (
				<Box sx={{ mt: 2 }}>
					<details>
						<summary>Extra Data</summary>
						<Box
							component="pre"
							sx={{
								mt: 1,
								p: 1.5,
								borderRadius: 1,
								bgcolor: "action.hover",
								overflow: "auto",
								fontSize: "0.85em",
							}}
						>
							{JSON.stringify(status.extra, null, 2)}
						</Box>
					</details>
				</Box>
			)}
		</Paper>
	);
}

/** Global health chip rendered at the top of the InfoSection. The
 * server's per-check breakdown is shown by `<ChecksTable>` below — this
 * is the "headline" answer to "is the server OK", derived from the
 * `HealthState` rollup rather than the raw top-level `healthy` bool so
 * a failing check can't hide behind a self-reported "healthy".
 *
 * Alongside it, the operator-presence headline: identified humans logged
 * in to the box this application runs on, per the `external_users` check.
 * Only asserted while the application is actively reporting — a stale push
 * can't claim anyone is on the box *right now*. */
function StatusInfoFields({ status }: { status: ServerLastStatusData }) {
	const caps = useApplicationTypeCaps(status.type);
	const tracking = caps?.version_tracking;
	const label = useApplicationTypeLabel(status.type);
	return (
		<>
			<Stack spacing={0.25}>
				<Typography variant="caption" color="text.secondary">
					Last seen
				</Typography>
				<Typography variant="body2" component="div">
					<TimeAgo timestamp={status.created_at} />
				</Typography>
			</Stack>
			{status.platform && (
				<InfoItem label="Platform" value={status.platform} />
			)}
			{status.timezone && (
				<InfoItem label="Timezone">
					<Typography variant="body2">
						<TimezoneTooltip tz={status.timezone} />
					</Typography>
				</InfoItem>
			)}
			{/* A type with no application version shows no version block at
			    all — the caption included, since an empty one reads as a
			    reporting failure rather than an absence.
			    spec: APP#versions */}
			{tracking !== undefined && tracking !== "absent" && (
				<Stack spacing={0.25}>
					<Typography variant="caption" color="text.secondary">
						{label}
					</Typography>
					<VersionIndicator
						version={status.version}
						tracking={tracking}
						distance={status.version_distance}
					/>
				</Stack>
			)}
			{status.postgres && (
				<InfoItem
					label="PostgreSQL"
					value={status.postgres}
					mono
				/>
			)}
			{status.nodejs && (
				<InfoItem label="Node.js" value={status.nodejs} mono />
			)}
			{status.bestool && (
				<InfoItem label="bestool" value={status.bestool} mono />
			)}
			{status.min_chrome_version != null && (
				<InfoItem
					label="Chrome"
					value={`${status.min_chrome_version} or later`}
					mono
				/>
			)}
		</>
	);
}

function InfoItem({
	label,
	value,
	mono = false,
	children,
}: {
	label: string;
	value?: string;
	mono?: boolean;
	children?: React.ReactNode;
}) {
	return (
		<Stack spacing={0.25}>
			<Typography variant="caption" color="text.secondary">
				{label}
			</Typography>
			{children ?? (
				<Typography
					variant="body2"
					sx={mono ? { fontFamily: "monospace" } : undefined}
				>
					{value}
				</Typography>
			)}
		</Stack>
	);
}

function renderLocation(server: ServerInfo): string {
	if (!server.cloud) return "On premise";
	return "Cloud";
}

/// What this server is trusted to do with names under its group's domains. Only
/// called where it is trusted with one of them, so there is no "none" reading.
// spec: DOM#permission-for-a-server-to-manage-its-own-names
function nameManagementLabel(server: ServerInfo): string {
	if (server.may_manage_dns && server.may_manage_tls) return "DNS and TLS";
	return server.may_manage_dns ? "DNS only" : "TLS only";
}

function GroupSection({
	group,
	billingLabels,
}: {
	group: ServerGroup;
	billingLabels: ServerDetailData["billing_labels"];
}) {
	const tagEntries = Object.entries(group.tags ?? {});
	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<Stack
				direction="row"
				sx={{ alignItems: "center", justifyContent: "space-between", mb: 1 }}
			>
				<Typography variant="h6" component="h2">
					Group
				</Typography>
				<MuiLink
					component={RouterLink}
					to={`/fleet/groups/${group.id}`}
					underline="hover"
				>
					{group.name}
				</MuiLink>
			</Stack>
			{group.notes && (
				<Typography
					variant="body2"
					sx={{ whiteSpace: "pre-wrap", color: "text.secondary", mb: 1 }}
				>
					{group.notes}
				</Typography>
			)}
			{tagEntries.length > 0 && (
				<Stack direction="row" sx={{ flexWrap: "wrap", gap: 0.5 }}>
					{tagEntries.map(([k, v]) => (
						<Chip
							key={k}
							size="small"
							variant="outlined"
							label={`${k}=${v}`}
						/>
					))}
				</Stack>
			)}
			{billingLabels.length > 0 && (
				<>
					<Typography
						variant="caption"
						color="text.secondary"
						sx={{ display: "block", mt: 1 }}
					>
						Billing labels
					</Typography>
					<Stack direction="row" sx={{ flexWrap: "wrap", gap: 0.5 }}>
						{billingLabels.map((t) => (
							<Chip key={t.key} size="small" label={`${t.key}=${t.value}`} />
						))}
					</Stack>
				</>
			)}
		</Paper>
	);
}

function NotesAndTagsSection({
	notes,
	tags,
}: {
	notes: string;
	tags: Record<string, string>;
}) {
	const tagEntries = Object.entries(tags ?? {});
	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<Typography variant="h6" component="h2" gutterBottom>
				Notes & tags
			</Typography>
			{notes && (
				<Typography variant="body2" sx={{ whiteSpace: "pre-wrap", mb: 1 }}>
					{notes}
				</Typography>
			)}
			{tagEntries.length > 0 && (
				<Stack direction="row" sx={{ flexWrap: "wrap", gap: 0.5 }}>
					{tagEntries.map(([k, v]) => (
						<Chip
							key={k}
							size="small"
							variant="outlined"
							label={`${k}=${v}`}
						/>
					))}
				</Stack>
			)}
		</Paper>
	);
}
