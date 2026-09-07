import {
	Alert,
	Box,
	Button,
	Chip,
	LinearProgress,
	Paper,
	Stack,
	Typography,
} from "@mui/material";
import AddIcon from "@mui/icons-material/Add";
import BuildOutlinedIcon from "@mui/icons-material/BuildOutlined";
import ArchiveIcon from "@mui/icons-material/ArchiveOutlined";
import BackupIcon from "@mui/icons-material/Backup";
import EditIcon from "@mui/icons-material/Edit";
import RestoreIcon from "@mui/icons-material/RestoreFromTrash";
import { useState } from "react";
import { Link as RouterLink, useNavigate, useParams } from "react-router-dom";
import GroupDomainsSection from "../components/GroupDomainsSection";
import GroupInventorySection from "../components/GroupInventorySection";
import MigrationTestsSection from "../components/MigrationTestsSection";
import { OperatorAvatar, connectedFor } from "../components/OperatorAvatars";
import ActiveIncidentCard from "../components/ActiveIncidentCard";
import GroupTree from "../components/GroupTree";
import MaintenanceSection from "../components/MaintenanceSection";
import SilencedRefsSection from "../components/SilencedRefsSection";
import { useApi, useApiAction } from "../api";
import { useIsAdmin } from "../hooks/useIsAdmin";
import { usePageTitle } from "../hooks/usePageTitle";
import {
	BACKUP_STATUS_INTENT,
	BACKUP_STATUS_LABEL,
	type BackupConfigStatus,
	aggregateOperators,
	type AggregatedOperator,
} from "../types";

export default function GroupDetail() {
	const { id = "" } = useParams<{ id: string }>();
	const navigate = useNavigate();
	const detail = useApi("fleet/groups", "get", { server_group_id: id }, [id]);
	const admin = useIsAdmin() === true;
	const archive = useApiAction("fleet/groups", "delete");
	// A window declared or lifted below changes what a run on this group's
	// inventory would be served, so the two sections read the same state.
	const [maintenanceTick, setMaintenanceTick] = useState(0);
	// Only the currently-open incident matters for the active-incident
	// section; closed ones live behind the /incidents filter route.
	const activeIncidents = useApi(
		"incidents",
		"list_for_group",
		{ server_group_id: id, include_closed: false, limit: 1 },
		[id],
	);
	// Same payload the status-page card uses, so the operator list here
	// always matches the count shown there.
	const groupStatuses = useApi(
		"statuses",
		"group_details",
		{ server_group_id: id },
		[id],
	);
	usePageTitle(detail.status === "ok" ? detail.data.group.name : "Group");

	if (detail.status === "loading" || detail.status === "idle") {
		return <LinearProgress />;
	}
	if (detail.status === "error") {
		return <Alert severity="error">{detail.error.message}</Alert>;
	}

	const { group, applications, machines, billing_labels } = detail.data;
	const tagEntries = Object.entries(group.tags ?? {});
	const operators =
		groupStatuses.status === "ok"
			? aggregateOperators(groupStatuses.data.members)
			: [];
	const openIncident =
		activeIncidents.status === "ok" && activeIncidents.data.length > 0
			? activeIncidents.data[0]
			: null;

	// A group archives when empty, or when every member has gone quiet, in
	// which case archiving cascades to those servers. The card carries that
	// fact: it is the same windowed question the archive itself enforces, and
	// a member unreachable for months is quiet without reading as "gone".
	const allQuiet =
		applications.length === 0 ||
		(groupStatuses.status === "ok" && groupStatuses.data.all_members_quiet);
	const onArchive = async () => {
		const cascade =
			applications.length > 0
				? ` This also archives its ${applications.length} quiet server${applications.length === 1 ? "" : "s"}.`
				: "";
		if (
			!confirm(
				`Archive group "${group.name}"?${cascade} It's hidden from listings but can be restored from the Archived tab.`,
			)
		)
			return;
		try {
			await archive.call({ server_group_id: group.id });
			navigate("/fleet");
		} catch {
			/* surfaced via archive.error */
		}
	};

	return (
		<Stack spacing={3}>
			<Stack
				direction="row"
				spacing={2}
				sx={{ alignItems: "center", justifyContent: "space-between" }}
			>
				<Stack direction="row" spacing={1.5} sx={{ alignItems: "center" }}>
					<Typography variant="h4" component="h1">
						{group.name}
					</Typography>
					{detail.data.maintained && (
						<Chip
							size="small"
							variant="outlined"
							color="info"
							icon={<BuildOutlinedIcon />}
							label={
								detail.data.maintenance_settling
									? "Maintenance just ended"
									: "Under maintenance"
							}
							component="a"
							href="#maintenance"
							clickable
							data-testid="maintenance-marker"
						/>
					)}
				</Stack>
				{admin && (
					<Stack direction="row" spacing={1}>
						<Button
							component={RouterLink}
							to={`/fleet/groups/${group.id}/machines/new`}
							variant="contained"
							startIcon={<AddIcon />}
						>
							Add machine
						</Button>
						<Button
							component={RouterLink}
							to={`/fleet/groups/${group.id}/edit`}
							variant="outlined"
							startIcon={<EditIcon />}
						>
							Edit
						</Button>
						{allQuiet && !group.deleted_at && (
							<Button
								variant="outlined"
								color="error"
								startIcon={<ArchiveIcon />}
								onClick={onArchive}
								disabled={archive.pending}
							>
								Archive
							</Button>
						)}
					</Stack>
				)}
			</Stack>

			{archive.error && (
				<Alert severity="error">{archive.error.message}</Alert>
			)}

			{group.deleted_at && (
				<ArchivedGroupBanner
					groupId={group.id}
					isAdmin={admin}
					onRestored={detail.reload}
				/>
			)}

			{openIncident && <ActiveIncidentCard incident={openIncident} />}

			{operators.length > 0 && <OperatorsSection operators={operators} />}

			{group.notes && (
				<Paper variant="outlined" sx={{ p: 2 }}>
					<Typography variant="h6" component="h2" gutterBottom>
						Notes
					</Typography>
					<Typography variant="body2" sx={{ whiteSpace: "pre-wrap" }}>
						{group.notes}
					</Typography>
				</Paper>
			)}

			{tagEntries.length > 0 && (
				<Paper variant="outlined" sx={{ p: 2 }}>
					<Typography variant="h6" component="h2" gutterBottom>
						Tags
					</Typography>
					<Box sx={{ display: "flex", flexWrap: "wrap", gap: 0.5 }}>
						{tagEntries.map(([k, v]) => (
							<Chip
								key={k}
								size="small"
								variant="outlined"
								label={`${k}=${v}`}
							/>
						))}
					</Box>
				</Paper>
			)}

			{billing_labels.length > 0 && (
				<Paper variant="outlined" sx={{ p: 2 }}>
					<Typography variant="h6" component="h2" gutterBottom>
						Billing labels
					</Typography>
					<Typography variant="body2" color="text.secondary" gutterBottom>
						Effective AWS cost-allocation tags for this group's resources.
					</Typography>
					<Box sx={{ display: "flex", flexWrap: "wrap", gap: 0.5 }}>
						{billing_labels.map((t) => (
							<Chip key={t.key} size="small" label={`${t.key}=${t.value}`} />
						))}
					</Box>
				</Paper>
			)}

			<Box>
				<Typography variant="h5" component="h2" gutterBottom>
					Machines ({machines.length})
				</Typography>
				{machines.length === 0 ? (
					<Alert severity="info">
						Nothing in this group yet. Add a machine; the applications on it
						arrive by report.
					</Alert>
				) : (
					<GroupTree machines={machines} applications={applications} />
				)}
			</Box>

			<BackupsCard groupId={group.id} isAdmin={admin} />

			<MigrationTestsSection groupId={group.id} servers={applications} />

			<GroupInventorySection
				groupId={group.id}
				applications={applications}
				maintenanceTick={maintenanceTick}
				onMaintenanceChange={() => setMaintenanceTick((n) => n + 1)}
			/>

			<GroupDomainsSection groupId={group.id} />

			<MaintenanceSection
				scope="group"
				anchor="maintenance"
				id={group.id}
				targetLabel={group.name}
				reloadKey={maintenanceTick}
				onChanged={() => setMaintenanceTick((n) => n + 1)}
			/>
			<SilencedRefsSection scope="group" id={group.id} />
		</Stack>
	);
}

/// Compact backups summary on the group detail page: a status chip + link to
/// the full panel, or a "Set up backups" CTA (admin) when no config exists.
function BackupsCard({
	groupId,
	isAdmin,
}: {
	groupId: string;
	isAdmin: boolean;
}) {
	const config = useApi(
		"backups",
		"get",
		{ server_group_id: groupId },
		[groupId],
	);
	if (config.status !== "ok") return null;

	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<Stack
				direction="row"
				spacing={2}
				sx={{ alignItems: "center", justifyContent: "space-between" }}
			>
				<Stack direction="row" spacing={1.5} sx={{ alignItems: "center" }}>
					<Typography variant="h6" component="h2">
						Backups
					</Typography>
					{config.data && (
						<Chip
							size="small"
							label={
								BACKUP_STATUS_LABEL[
									config.data.status as BackupConfigStatus
								]
							}
							color={
								BACKUP_STATUS_INTENT[
									config.data.status as BackupConfigStatus
								]
							}
						/>
					)}
				</Stack>
				{config.data == null ? (
					isAdmin ? (
						<Button
							component={RouterLink}
							to={`/fleet/groups/${groupId}/backups/config`}
							variant="outlined"
							startIcon={<BackupIcon />}
						>
							Set up backups
						</Button>
					) : (
						<Typography variant="body2" color="text.secondary">
							Not set up
						</Typography>
					)
				) : (
					<Button
						component={RouterLink}
						to={`/fleet/groups/${groupId}/backups`}
						variant="outlined"
					>
						View backups
					</Button>
				)}
			</Stack>
		</Paper>
	);
}

/// Distinct people connected across the group's boxes right now (from each
/// member's `external_users` check, deduped by Tailscale login). The same
/// aggregate the status-page card chip counts. Hidden entirely when nobody's
/// connected.
function OperatorsSection({
	operators,
}: {
	operators: AggregatedOperator[];
}) {
	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<Typography variant="h6" component="h2" gutterBottom>
				{operators.length} operator{operators.length === 1 ? "" : "s"} in
				the group right now
			</Typography>
			<Stack spacing={1}>
				{operators.map(({ op, machines }) => {
					const dur = connectedFor(op.connected_since);
					return (
						<Stack
							key={op.login}
							direction="row"
							spacing={1.5}
							sx={{ alignItems: "center" }}
						>
							<OperatorAvatar op={op} size={32} />
							<Box>
								<Typography variant="body2">
									{op.name ? `${op.name} (${op.login})` : op.login}
								</Typography>
								<Typography variant="caption" color="text.secondary">
									{[dur && `connected ${dur}`, `on ${machines.join(", ")}`]
										.filter(Boolean)
										.join(" · ")}
								</Typography>
							</Box>
						</Stack>
					);
				})}
			</Stack>
		</Paper>
	);
}


function ArchivedGroupBanner({
	groupId,
	isAdmin,
	onRestored,
}: {
	groupId: string;
	isAdmin: boolean;
	onRestored: () => void;
}) {
	const action = useApiAction("fleet/groups", "restore");
	const onRestore = async () => {
		try {
			await action.call({ server_group_id: groupId });
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
			This group is archived. Restore it to bring it back to the listings.
			{action.error && <Box sx={{ mt: 1 }}>{action.error.message}</Box>}
		</Alert>
	);
}

