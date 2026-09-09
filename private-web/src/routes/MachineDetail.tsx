import {
	Alert,
	Box,
	Chip,
	LinearProgress,
	Paper,
	Stack,
	Typography,
} from "@mui/material";
import EditIcon from "@mui/icons-material/Edit";
import InsightsIcon from "@mui/icons-material/Insights";
import { useEffect, useState } from "react";
import { useLocation, useParams } from "react-router-dom";
import { useApi } from "../api";
import ActionButton from "../components/ActionButton";
import ActiveIncidentCard from "../components/ActiveIncidentCard";
import { ChecksTable, HealthIndicator } from "../components/ChecksTable";
import IncidentsLink from "../components/IncidentsLink";
import { HealthLegend, StatusLegend } from "../components/Legends";
import MachineBackupSection from "../components/MachineBackupSection";
import MachineIdentitySection from "../components/MachineIdentitySection";
import MachineSetupInstructions from "../components/MachineSetupInstructions";
import MaintenanceSection from "../components/MaintenanceSection";
import ServerRankChip from "../components/ServerRankChip";
import ServerShorty from "../components/ServerShorty";
import GroupTree from "../components/GroupTree";
import SilencedRefsSection from "../components/SilencedRefsSection";
import TargetName from "../components/TargetName";
import TimeAgo from "../components/TimeAgo";
import TimezoneTooltip from "../components/TimezoneTooltip";
import { useIsAdmin } from "../hooks/useIsAdmin";
import { usePageTitle } from "../hooks/usePageTitle";
import { humanSeconds } from "../lib/humanDuration";
import {
	incidentTargetName,
	type MachineDetailData,
	SERVER_RANK_ORDER,
	type ServerInfo,
	type ServerRank,
} from "../types";

/// A machine's own page: the box, what it reports about itself, its health,
/// and the workloads on it.
///
/// What is deliberately absent is what belongs to a workload — an application
/// version, a database engine, a URL. Those live on each application's page,
/// linked from the list below.
/// spec: FLT
export default function MachineDetail() {
	const { id = "" } = useParams<{ id: string }>();
	const isAdmin = useIsAdmin() === true;
	const [refreshTick, setRefreshTick] = useState(0);
	const bumpRefresh = () => setRefreshTick((t) => t + 1);
	const detail = useApi("fleet/machines", "get_detail", { machine_id: id }, [
		id,
		refreshTick,
	]);
	// The group's open incident, if any. An incident is never a box's, so this
	// is read by group and shown as the group's.
	const groupId = detail.status === "ok" ? (detail.data.group?.id ?? null) : null;
	const activeIncidents = useApi(
		"incidents",
		"list_for_group",
		{ server_group_id: groupId ?? "", include_closed: false, limit: 1 },
		[groupId, refreshTick],
		{ skip: groupId === null },
	);
	// Honour a `#backups` anchor (linked from the group's backup page): once the
	// detail has loaded and the section is painted, scroll it into view.
	const location = useLocation();
	const detailLoaded = detail.status === "ok";
	useEffect(() => {
		if (!detailLoaded || location.hash !== "#backups") return;
		const frame = requestAnimationFrame(() => {
			document
				.getElementById("backups")
				?.scrollIntoView({ behavior: "smooth", block: "start" });
		});
		return () => cancelAnimationFrame(frame);
	}, [detailLoaded, location.hash]);
	usePageTitle(
		detail.status === "ok"
			? (machineLabel(detail.data) ?? "Unnamed machine")
			: "Machine",
	);

	if (detail.status === "loading" || detail.status === "idle") {
		return <LinearProgress />;
	}
	if (detail.status === "error") {
		return <Alert severity="error">{detail.error.message}</Alert>;
	}

	const data = detail.data;
	const enrolled = data.machine.registered_at != null;
	const openIncident =
		activeIncidents.status === "ok" && activeIncidents.data.length > 0
			? activeIncidents.data[0]
			: null;
	// Munin watches the box, reachable over the tailnet — build its URL from
	// the bound identity's MagicDNS name (live value preferred, falling back to
	// the stored snapshot). Offered only when the box is known to run Munin and
	// has a tailnet name.
	// spec: SVC#munin-link
	const tailnetName =
		data.device_info?.tailnet_live?.display_name ??
		data.device_info?.device?.tailscale_node_name ??
		null;
	const muninUrl =
		data.munin && tailnetName ? `https://${tailnetName}:4950/` : null;

	// A box has no rank of its own: it takes the highest of the workloads on
	// it, which is the same derivation its billing stage uses.
	// spec: FLT#environments
	const rank = machineRank(data.applications);

	return (
		<Stack spacing={3}>
			{/* The same header the application page has, because the two grains
			    are siblings and reading one should teach the other: chips, then
			    the group-prefixed title, then the actions. */}
			{/* spec: FLT#navigating-the-two-grains */}
			<Stack spacing={1.5}>
				<Stack
					direction="row"
					spacing={1}
					sx={{ alignItems: "center", flexWrap: "wrap" }}
					useFlexGap
				>
					{rank && <ServerRankChip rank={rank} />}
					{/* A box created a minute ago has nothing on it, and that is
					    its normal condition rather than a count of zero. */}
					<Chip
						size="small"
						label={
							data.applications.length === 0
								? "not yet reporting"
								: `${data.applications.length} application${
										data.applications.length === 1 ? "" : "s"
									}`
						}
					/>
					<Typography variant="h4" component="h1" sx={{ ml: 1 }}>
						<TargetName
							parts={[
								{
									label: data.group?.name ?? "",
									to: data.group ? `/fleet/groups/${data.group.id}` : null,
								},
								{ label: machineLabel(data) ?? "Unnamed machine" },
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
					{muninUrl && (
						<ActionButton
							href={muninUrl}
							icon={<InsightsIcon />}
							label="Munin"
						/>
					)}
					<IncidentsLink
						groupId={data.group?.id ?? null}
						refreshKey={refreshTick}
					/>
					{isAdmin && (
						<ActionButton
							to={`/fleet/machines/${data.machine.id}/edit`}
							icon={<EditIcon />}
							label="Edit"
							color="primary"
						/>
					)}
				</Stack>
			</Stack>

			{openIncident && (
				<ActiveIncidentCard
					incident={openIncident}
					targetName={incidentTargetName(openIncident)}
				/>
			)}

			{data.machine.deleted_at != null && (
				<Alert severity="warning">This machine is archived.</Alert>
			)}
			{!enrolled && data.machine.deleted_at == null && (
				<>
					<Alert severity="info">
						This machine hasn't checked in yet. Follow the setup
						instructions below to enrol it.
					</Alert>
					<MachineSetupInstructions
						machineId={data.machine.id}
						onRegistered={() => detail.reload()}
					/>
				</>
			)}

			<Paper variant="outlined" sx={{ p: 2 }}>
				<HealthIndicator
					health={data.health}
					up={data.up}
					monitored={data.machine.is_monitored}
					maintained={data.maintained}
					maintenanceSettling={data.maintenance_settling}
					operators={data.operators}
				/>
				<Stack
					direction="row"
					spacing={4}
					useFlexGap
					sx={{ flexWrap: "wrap" }}
				>
					<Figures figures={data.figures} lastReportedAt={data.last_reported_at} />
					<InfoItem
						label="Location"
						value={
							data.machine.cloud == null
								? "unknown"
								: data.machine.cloud
									? "cloud"
									: "on premise"
						}
					/>
					<InfoItem
						label="Unreachable after"
						value={humanSeconds(data.machine.alert_when_down_for)}
					/>
				</Stack>
				<ChecksTable
					checks={data.checks}
					operators={data.operators}
					target={{ kind: "machine", id: data.machine.id }}
					groupId={data.group?.id ?? null}
					refreshTick={refreshTick}
					onSilenced={bumpRefresh}
				/>
			</Paper>

			<ApplicationsOnThisBox applications={data.applications} />

			<MachineBackupSection
				machineId={data.machine.id}
				groupId={data.group?.id ?? null}
				isAdmin={isAdmin}
			/>

			{data.billing_labels.length > 0 && (
				<Paper variant="outlined" sx={{ p: 2 }}>
					<Typography variant="h6" gutterBottom>
						Billing labels
					</Typography>
					<Stack direction="row" spacing={1} sx={{ flexWrap: "wrap" }} useFlexGap>
						{data.billing_labels.map((tag) => (
							<Typography
								key={tag.key}
								variant="body2"
								sx={{ fontFamily: "monospace" }}
							>
								{tag.key}={tag.value}
							</Typography>
						))}
					</Stack>
				</Paper>
			)}

			{(data.machine.notes ||
				Object.keys(data.machine.tags ?? {}).length > 0) && (
				<Paper variant="outlined" sx={{ p: 2 }}>
					{data.machine.notes && (
						<Typography variant="body2" sx={{ whiteSpace: "pre-wrap" }}>
							{data.machine.notes}
						</Typography>
					)}
					{Object.entries(data.machine.tags ?? {}).map(([key, value]) => (
						<Typography
							key={key}
							variant="body2"
							sx={{ fontFamily: "monospace" }}
						>
							{key}={value}
						</Typography>
					))}
				</Paper>
			)}

			<MachineIdentitySection
				machineId={data.machine.id}
				deviceInfo={data.device_info}
				isAdmin={isAdmin}
				enrolled={enrolled}
				refresh={() => detail.reload()}
			/>

			<SilencedRefsSection
				scope="machine"
				id={data.machine.id}
				refreshKey={refreshTick}
				onChanged={bumpRefresh}
			/>

			<MaintenanceSection
				scope="machine"
				anchor="maintenance"
				id={data.machine.id}
				targetLabel={machineLabel(data) ?? undefined}
				groupId={data.group?.id ?? null}
				groupName={data.group?.name ?? null}
				rank={rank}
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
						currentMachineId={data.machine.id}
					/>
				</Box>
			)}

			<Box>
				<StatusLegend />
				<Box sx={{ mt: 1 }}>
					<HealthLegend />
				</Box>
			</Box>
		</Stack>
	);
}

/// What to call the box: the name an operator gave it, else the hostname it
/// reports, else nothing — the id is already in the address bar.
function machineLabel(data: MachineDetailData): string | null {
	if (data.machine.name) return data.machine.name;
	const hostname = readString(data.figures, "hostname");
	return hostname ?? null;
}

function readString(figures: unknown, key: string): string | undefined {
	if (typeof figures !== "object" || figures === null) return undefined;
	const value = (figures as Record<string, unknown>)[key];
	return typeof value === "string" ? value : undefined;
}

function readNumber(figures: unknown, key: string): number | undefined {
	if (typeof figures !== "object" || figures === null) return undefined;
	const value = (figures as Record<string, unknown>)[key];
	return typeof value === "number" ? value : undefined;
}

/// What the box says about itself. Distinct from an application's figures,
/// which are about its software.
/// spec: FIG#sourcing
function Figures({
	figures,
	lastReportedAt,
}: {
	figures: unknown;
	lastReportedAt: string | null;
}) {
	const platform = readString(figures, "platform") ?? osLabel(figures);
	const timezone = readString(figures, "osTimezone");
	const cores = readNumber(figures, "cpuCores");
	const memory = readNumber(figures, "totalMemoryBytes");
	const uptime = readNumber(figures, "uptimeSecs");
	const bestool = readString(figures, "bestoolVersion");
	return (
		<>
			{lastReportedAt && (
				<InfoItem label="Last reported">
					<Typography variant="body2" component="div">
						<TimeAgo timestamp={lastReportedAt} />
					</Typography>
				</InfoItem>
			)}
			{platform && <InfoItem label="Platform" value={platform} />}
			{timezone && (
				<InfoItem label="Timezone">
					<Typography variant="body2">
						<TimezoneTooltip tz={timezone} />
					</Typography>
				</InfoItem>
			)}
			{cores != null && <InfoItem label="Processors" value={String(cores)} />}
			{memory != null && <InfoItem label="Memory" value={gibibytes(memory)} />}
			{uptime != null && (
				<InfoItem label="Uptime" value={humanSeconds(uptime)} />
			)}
			{bestool && <InfoItem label="bestool" value={bestool} mono />}
		</>
	);
}

/// A platform string assembled from the parts a reporter sends when it sends
/// no `platform` of its own.
function osLabel(figures: unknown): string | undefined {
	const name = readString(figures, "osName");
	const version = readString(figures, "osVersion");
	if (!name) return undefined;
	return version ? `${name} ${version}` : name;
}

function gibibytes(bytes: number): string {
	return `${(bytes / 1024 ** 3).toFixed(1)} GiB`;
}

/// The workloads this box carries. Two or more is the case the machine grain
/// exists for.
///
/// The rows are the same shorty the fleet uses elsewhere, so a workload reads
/// the same here as it does anywhere else: state, name, what it is, and where
/// it answers. The group is left off — every one of them is in this box's.
function ApplicationsOnThisBox({
	applications,
}: {
	applications: ServerInfo[];
}) {
	return (
		<Box data-testid="applications-on-box">
			<Typography variant="h5" component="h2" gutterBottom>
				Applications ({applications.length})
			</Typography>
			{applications.length === 0 ? (
				<Typography variant="body2" color="text.secondary">
					None yet. Applications appear here as the machine reports them.
				</Typography>
			) : (
				<Stack spacing={1}>
					{applications.map((application) => (
						<ServerShorty
							key={application.id}
							server={application}
							withGroup={false}
						/>
					))}
				</Stack>
			)}
		</Box>
	);
}

/// The highest rank among the workloads on a box, which is what a box's rank
/// means. A box carrying nothing yet has none.
// spec: FLT#environments
function machineRank(applications: ServerInfo[]): ServerRank | null {
	for (const rank of SERVER_RANK_ORDER) {
		if (applications.some((application) => application.rank === rank)) {
			return rank;
		}
	}
	return null;
}

function InfoItem({
	label,
	value,
	mono = false,
	children,
}: {
	label: string;
	value?: string | null;
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
					{value ?? "—"}
				</Typography>
			)}
		</Stack>
	);
}
