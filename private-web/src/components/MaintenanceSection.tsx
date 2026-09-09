import {
	Alert,
	Box,
	Button,
	LinearProgress,
	Link as MuiLink,
	Paper,
	Stack,
	Typography,
} from "@mui/material";
import BuildOutlinedIcon from "@mui/icons-material/BuildOutlined";
import { useState } from "react";
import { Link as RouterLink } from "react-router-dom";
import { useApi, useApiAction } from "../api";
import { useIsAdmin } from "../hooks/useIsAdmin";
import type { MaintenanceScope, MaintenanceWindow, ServerRank } from "../types";
import DeclareMaintenanceDialog from "./DeclareMaintenanceDialog";
import ServerRankChip from "./ServerRankChip";
import TimeAgo from "./TimeAgo";

const HISTORY_SHOWN = 5;

/** The target's maintenance: the window holding over it with the actions to
 * amend or lift it, and the windows that have ended as history. Sits on the
 * server and group detail pages. */
// spec: MNT#presentation
export default function MaintenanceSection({
	scope,
	id,
	targetLabel,
	machineId,
	machineName,
	groupId,
	groupName,
	rank,
	onChanged,
	anchor,
}: {
	/** DOM id, so a banner elsewhere on the page can link here. */
	anchor?: string;
	scope: MaintenanceScope;
	id: string;
	targetLabel?: string;
	/** For an application, the box it runs on: a machine's window covers every
	 * application on it, so the application is under maintenance without having
	 * a window of its own. Its own surface has to say so. */
	machineId?: string | null;
	machineName?: string | null;
	/** For a machine or an application, the group: a group's window covers
	 * every machine in it, so the target is under maintenance without having a
	 * window of its own. */
	groupId?: string | null;
	groupName?: string | null;
	/** The environment the target serves: a window over its group's
	 * environment at that rank covers it too. */
	rank?: ServerRank | null;
	/** Called after declaring or lifting, so the page can refresh the
	 * health and checks that the window changes. */
	onChanged?: () => void;
}) {
	const isAdmin = useIsAdmin() === true;
	const [tick, setTick] = useState(0);
	const [dialogOpen, setDialogOpen] = useState(false);
	const lift = useApiAction("maintenance", "lift");

	const result = useApi(
		"maintenance",
		"for_target",
		scope === "application"
			? { application_id: id }
			: scope === "machine"
				? { machine_id: id }
				: { server_group_id: id },
		[id, tick],
	);
	const covering = useApi(
		"maintenance",
		"for_target",
		{ server_group_id: groupId ?? "" },
		[groupId, tick],
		{ skip: !groupId },
	);
	const coveringMachine = useApi(
		"maintenance",
		"for_target",
		{ machine_id: machineId ?? "" },
		[machineId, tick],
		{ skip: !machineId },
	);

	const reload = () => {
		setTick((t) => t + 1);
		onChanged?.();
	};

	if (result.status === "loading" || result.status === "idle") {
		return (
			<Paper id={anchor} variant="outlined" sx={{ p: 2 }}>
				<Heading />
				<LinearProgress />
			</Paper>
		);
	}
	if (result.status === "error") {
		return (
			<Paper id={anchor} variant="outlined" sx={{ p: 2 }}>
				<Heading />
				<Alert severity="error">{result.error.message}</Alert>
			</Paper>
		);
	}

	const windows: MaintenanceWindow[] = result.data;
	// A group's own window, not one of its environments'.
	const open = windows.find((w) => w.ended_at === null && !w.rank) ?? null;
	const environments = windows.filter((w) => w.ended_at === null && w.rank);
	const fromGroup =
		covering.status === "ok"
			? ((covering.data as MaintenanceWindow[]).find(
					(w) => w.ended_at === null && (!w.rank || w.rank === rank),
				) ?? null)
			: null;
	const fromMachine =
		coveringMachine.status === "ok"
			? ((coveringMachine.data as MaintenanceWindow[]).find(
					(w) => w.ended_at === null,
				) ?? null)
			: null;
	const history = windows.filter((w) => w.ended_at !== null).slice(0, HISTORY_SHOWN);

	if (
		!open &&
		environments.length === 0 &&
		!fromGroup &&
		!fromMachine &&
		history.length === 0 &&
		!isAdmin
	)
		return null;

	return (
		<Paper id={anchor} variant="outlined" sx={{ p: 2 }} data-testid="maintenance-section">
			<Heading />
			{fromMachine && (
				<Alert
					severity="info"
					icon={<BuildOutlinedIcon fontSize="inherit" />}
					sx={{ mb: 2 }}
					data-testid="covering-machine-window"
				>
					<Typography variant="body2">
						Under maintenance, ending{" "}
						<TimeAgo timestamp={fromMachine.expected_end} />, as part of{" "}
						<MuiLink component={RouterLink} to={`/fleet/machines/${machineId}`}>
							{machineName ?? "the machine it runs on"}
						</MuiLink>
						. Amend or lift it there.
					</Typography>
					{fromMachine.note && (
						<Typography variant="body2" sx={{ mt: 0.5, fontStyle: "italic" }}>
							{fromMachine.note}
						</Typography>
					)}
				</Alert>
			)}
			{fromGroup && (
				<Alert
					severity="info"
					icon={<BuildOutlinedIcon fontSize="inherit" />}
					sx={{ mb: 2 }}
					data-testid="covering-group-window"
				>
					<Typography variant="body2">
						Under maintenance, ending{" "}
						<TimeAgo timestamp={fromGroup.expected_end} />, as part of{" "}
						<MuiLink component={RouterLink} to={`/fleet/groups/${groupId}`}>
							{groupName ?? "its group"}
						</MuiLink>
						. Amend or lift it there.
					</Typography>
					{fromGroup.note && (
						<Typography variant="body2" sx={{ mt: 0.5, fontStyle: "italic" }}>
							{fromGroup.note}
						</Typography>
					)}
				</Alert>
			)}
			{open ? (
				<Alert
					severity="info"
					icon={<BuildOutlinedIcon fontSize="inherit" />}
					sx={{ mb: history.length ? 2 : 0 }}
					action={
						isAdmin ? (
							<Stack direction="row" spacing={1}>
								<Button size="small" color="info" onClick={() => setDialogOpen(true)}>
									Amend
								</Button>
								<Button
									size="small"
									color="info"
									variant="outlined"
									disabled={lift.pending}
									onClick={async () => {
										try {
											await lift.call({ id: open.id });
											reload();
										} catch {
											/* surfaced below */
										}
									}}
								>
									Lift
								</Button>
							</Stack>
						) : undefined
					}
				>
					<Typography variant="body2">
						Under maintenance, ending <TimeAgo timestamp={open.expected_end} />.

						Checks are still recorded and shown. Nothing on this {scope}{" "}
						alerts.
					</Typography>
					{open.note && (
						<Typography variant="body2" sx={{ mt: 0.5, fontStyle: "italic" }}>
							{open.note}
						</Typography>
					)}
				</Alert>
			) : (
				isAdmin && (
					<Button
						size="small"
						variant="outlined"
						startIcon={<BuildOutlinedIcon />}
						onClick={() => setDialogOpen(true)}
						sx={{ mb: history.length ? 2 : 0 }}
					>
						{fromMachine || fromGroup
							? `Declare for this ${scope} as well`
							: `Declare maintenance for this ${scope}`}
					</Button>
				)
			)}
			{environments.map((window) => (
				<Alert
					key={window.id}
					severity="info"
					icon={<BuildOutlinedIcon fontSize="inherit" />}
					sx={{ mt: 1 }}
					data-testid="environment-window"
					action={
						isAdmin ? (
							<Button
								size="small"
								color="info"
								variant="outlined"
								disabled={lift.pending}
								onClick={async () => {
									try {
										await lift.call({ id: window.id });
										reload();
									} catch {
										/* surfaced below */
									}
								}}
							>
								Lift
							</Button>
						) : undefined
					}
				>
					<Typography variant="body2">
						<Box component="span" sx={{ textTransform: "capitalize" }}>
							{window.rank}
						</Box>{" "}
						under maintenance, ending <TimeAgo timestamp={window.expected_end} />.
						The rest of the group stays watched.
					</Typography>
					{window.note && (
						<Typography variant="body2" sx={{ mt: 0.5, fontStyle: "italic" }}>
							{window.note}
						</Typography>
					)}
				</Alert>
			))}
			{lift.error && (
				<Alert severity="error" sx={{ mt: 1 }}>
					{lift.error.message}
				</Alert>
			)}
			{history.length > 0 && (
				<Stack spacing={1}>
					{history.map((window) => (
						<Box
							key={window.id}
							sx={{ p: 1.5, border: 1, borderColor: "divider", borderRadius: 1 }}
						>
							<Stack
								direction="row"
								spacing={1}
								sx={{ alignItems: "center", flexWrap: "wrap" }}
								useFlexGap
							>
								<Typography variant="body2">
									{window.note ?? "Maintenance"}
								</Typography>
								{window.rank && <ServerRankChip rank={window.rank} />}
								<Box sx={{ flex: 1 }} />
								<Typography variant="caption" color="text.secondary">
									ended <TimeAgo timestamp={window.ended_at as string} />
									{window.ended_by
										? ` by ${window.ended_by}`
										: " at its expected end"}
								</Typography>
							</Stack>
						</Box>
					))}
				</Stack>
			)}
			<DeclareMaintenanceDialog
				open={dialogOpen}
				onClose={() => setDialogOpen(false)}
				scope={scope}
				id={id}
				targetLabel={targetLabel}
				existing={open}
				onDone={reload}
			/>
		</Paper>
	);
}

function Heading() {
	return (
		<Typography variant="h6" component="h2" gutterBottom>
			Maintenance
		</Typography>
	);
}
