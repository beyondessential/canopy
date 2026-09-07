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
import type { MaintenanceWindow } from "../types";
import DeclareMaintenanceDialog from "./DeclareMaintenanceDialog";
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
	groupId,
	groupName,
	onChanged,
	reloadKey = 0,
	anchor,
}: {
	/** DOM id, so a banner elsewhere on the page can link here. */
	anchor?: string;
	scope: "machine" | "group";
	id: string;
	targetLabel?: string;
	/** For a machine, its group: a group's window covers every machine in it,
	 * so the machine is under maintenance without having a window of its own.
	 * Its own surface has to say so. */
	groupId?: string | null;
	groupName?: string | null;
	/** Called after declaring or lifting, so the page can refresh the
	 * health and checks that the window changes. */
	onChanged?: () => void;
	/** Bumped when something else on the page declares or lifts a window over
	 * this target. */
	reloadKey?: number;
}) {
	const isAdmin = useIsAdmin() === true;
	const [tick, setTick] = useState(0);
	const [dialogOpen, setDialogOpen] = useState(false);
	const lift = useApiAction("maintenance", "lift");

	const result = useApi(
		"maintenance",
		"for_target",
		scope === "machine" ? { machine_id: id } : { server_group_id: id },
		[id, tick, reloadKey],
	);
	const covering = useApi(
		"maintenance",
		"for_target",
		{ server_group_id: groupId ?? "" },
		[groupId, tick, reloadKey],
		{ skip: !groupId },
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
	const open = windows.find((w) => w.ended_at === null) ?? null;
	const fromGroup =
		covering.status === "ok"
			? ((covering.data as MaintenanceWindow[]).find((w) => w.ended_at === null) ?? null)
			: null;
	const history = windows.filter((w) => w.ended_at !== null).slice(0, HISTORY_SHOWN);

	if (!open && !fromGroup && history.length === 0 && !isAdmin) return null;

	return (
		<Paper id={anchor} variant="outlined" sx={{ p: 2 }} data-testid="maintenance-section">
			<Heading />
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

						Checks are still recorded and shown. Nothing on this{" "}
						{scope === "machine" ? "machine" : "group"} alerts.
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
						{fromGroup ? "Declare for this server as well" : "Declare maintenance"}
					</Button>
				)
			)}
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
