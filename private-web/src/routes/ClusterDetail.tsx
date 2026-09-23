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
import { useState } from "react";
import { useParams } from "react-router-dom";
import { useApi } from "../api";
import ActionButton from "../components/ActionButton";
import { ChecksTable, HealthIndicator } from "../components/ChecksTable";
import { HealthLegend, StatusLegend } from "../components/Legends";
import ServerShorty from "../components/ServerShorty";
import SilencedRefsSection from "../components/SilencedRefsSection";
import TimeAgo from "../components/TimeAgo";
import { useIsAdmin } from "../hooks/useIsAdmin";
import { usePageTitle } from "../hooks/usePageTitle";
import { humanSeconds } from "../lib/humanDuration";
import type { ClusterApplication } from "../types";

/// A registered cluster's page: its health, its reachability, the checks its
/// relay files about it, and the applications it hosts. The same shape as a
/// machine's, a cluster standing where a machine stands for the applications
/// on it.
/// spec: K8S
export default function ClusterDetail() {
	const { id = "" } = useParams<{ id: string }>();
	const isAdmin = useIsAdmin() === true;
	const [refreshTick, setRefreshTick] = useState(0);
	const bumpRefresh = () => setRefreshTick((t) => t + 1);
	const detail = useApi("fleet/clusters", "get_detail", { cluster_id: id }, [
		id,
		refreshTick,
	]);
	usePageTitle(detail.status === "ok" ? detail.data.cluster.name : "Cluster");

	if (detail.status === "loading" || detail.status === "idle") {
		return <LinearProgress />;
	}
	if (detail.status === "error") {
		return <Alert severity="error">{detail.error.message}</Alert>;
	}

	const data = detail.data;
	return (
		<Stack spacing={3}>
			{/* A cluster belongs to no group, so the title is its name alone. */}
			<Stack spacing={1.5}>
				<Stack
					direction="row"
					spacing={1}
					sx={{ alignItems: "center", flexWrap: "wrap" }}
					useFlexGap
				>
					<Chip
						size="small"
						label={`${data.applications.length} application${
							data.applications.length === 1 ? "" : "s"
						}`}
					/>
					<Typography variant="h4" component="h1" sx={{ ml: 1 }}>
						{data.cluster.name}
					</Typography>
				</Stack>
				{isAdmin && (
					<Stack direction="row" spacing={1} useFlexGap>
						<ActionButton
							to={`/fleet/clusters/${data.cluster.id}/edit`}
							icon={<EditIcon />}
							label="Edit"
							color="primary"
						/>
					</Stack>
				)}
			</Stack>

			<Paper variant="outlined" sx={{ p: 2 }}>
				<HealthIndicator
					health={data.health}
					up={data.up}
					monitored
					maintained={false}
					maintenanceSettling={false}
					operators={[]}
				/>
				<Stack direction="row" spacing={4} useFlexGap sx={{ flexWrap: "wrap" }}>
					{data.last_reported_at && (
						<InfoItem label="Last reported">
							<Typography variant="body2" component="div">
								<TimeAgo timestamp={data.last_reported_at} />
							</Typography>
						</InfoItem>
					)}
					<InfoItem
						label="Unreachable after"
						value={humanSeconds(data.cluster.alert_when_down_for)}
					/>
				</Stack>
				<ChecksTable
					checks={data.checks}
					operators={[]}
					target={{ kind: "cluster", id: data.cluster.id }}
					groupId={null}
					refreshTick={refreshTick}
					onSilenced={bumpRefresh}
				/>
			</Paper>

			<HostedApplications applications={data.applications} />

			<SilencedRefsSection
				scope="cluster"
				id={data.cluster.id}
				refreshKey={refreshTick}
				onChanged={bumpRefresh}
			/>

			<Box>
				<StatusLegend />
				<Box sx={{ mt: 1 }}>
					<HealthLegend />
				</Box>
			</Box>
		</Stack>
	);
}

/// The applications scheduled across this cluster. A cluster carries many
/// groups' applications, so each row names its group.
function HostedApplications({
	applications,
}: {
	applications: ClusterApplication[];
}) {
	return (
		<Box data-testid="applications-on-cluster">
			<Typography variant="h5" component="h2" gutterBottom>
				Applications ({applications.length})
			</Typography>
			{applications.length === 0 ? (
				<Typography variant="body2" color="text.secondary">
					None yet. Applications appear here as the relay reports them.
				</Typography>
			) : (
				<Stack spacing={1}>
					{applications.map((application) => (
						<ServerShorty key={application.id} server={application} />
					))}
				</Stack>
			)}
		</Box>
	);
}

function InfoItem({
	label,
	value,
	children,
}: {
	label: string;
	value?: string | null;
	children?: React.ReactNode;
}) {
	return (
		<Stack spacing={0.25}>
			<Typography variant="caption" color="text.secondary">
				{label}
			</Typography>
			{children ?? <Typography variant="body2">{value ?? "—"}</Typography>}
		</Stack>
	);
}
