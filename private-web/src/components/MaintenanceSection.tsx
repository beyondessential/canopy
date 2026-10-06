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
import { heldByLabel } from "../types";
import type {
	MaintenanceScope,
	MaintenanceWindow,
	ServerRank,
	TargetWindow,
} from "../types";
import DeclareMaintenanceDialog from "./DeclareMaintenanceDialog";
import { GradedAction } from "./GradedAction";
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
	machineId,
	machineName,
	groupId,
	groupName,
	rank,
	onChanged,
	reloadKey = 0,
	anchor,
}: {
	/** DOM id, so a banner elsewhere on the page can link here. */
	anchor?: string;
	scope: MaintenanceScope;
	id: string;
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
	/** Bumped when something else on the page declares or lifts a window over
	 * this target. */
	reloadKey?: number;
}) {
	const isAdmin = useIsAdmin() === true;
	const [tick, setTick] = useState(0);
	const [dialogOpen, setDialogOpen] = useState(false);
	const [amending, setAmending] = useState<MaintenanceWindow | null>(null);
	const lift = useApiAction("maintenance", "lift");

	const result = useApi(
		"maintenance",
		"for_target",
		scope === "application"
			? { application_id: id }
			: scope === "machine"
				? { machine_id: id }
				: { server_group_id: id },
		[id, tick, reloadKey],
	);
	const covering = useApi(
		"maintenance",
		"for_target",
		{ server_group_id: groupId ?? "" },
		[groupId, tick, reloadKey],
		{ skip: !groupId },
	);
	const coveringMachine = useApi(
		"maintenance",
		"for_target",
		{ machine_id: machineId ?? "" },
		[machineId, tick, reloadKey],
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

	const spans: TargetWindow[] = result.data;
	// The windows over this target now, as against those that moved off it.
	// spec: MNT#moving-a-window
	const current = (rows: TargetWindow[]) =>
		rows.filter((row) => row.moved_at === null).map((row) => row.window);
	const windows = current(spans);
	// A window past its expected end suspends nothing, whether or not the sweep
	// has closed it: saying otherwise claims alerting is off while it is back on.
	// spec: MNT#settling
	const holds = (window: MaintenanceWindow) =>
		window.ended_at === null &&
		new Date(window.expected_end).getTime() > Date.now();
	// A group's own window, not one of its environments'.
	const open = windows.find((w) => w.ended_at === null && !w.rank) ?? null;
	const environmentWindows = windows.filter((w) => w.ended_at === null && w.rank);
	const fromGroup =
		covering.status === "ok"
			? (current(covering.data).find(
					(w) => holds(w) && (!w.rank || w.rank === rank),
				) ?? null)
			: null;
	const fromMachine =
		coveringMachine.status === "ok"
			? (current(coveringMachine.data).find(holds) ?? null)
			: null;
	const history = spans
		.filter((row) => row.window.ended_at !== null || row.moved_at !== null)
		.slice(0, HISTORY_SHOWN);

	if (
		!open &&
		environmentWindows.length === 0 &&
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
							{heldByLabel({ kind: "machine", name: machineName })}
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
							{fromGroup.rank
								? heldByLabel({ kind: "environment", rank: fromGroup.rank })
								: heldByLabel({ kind: "group", name: groupName })}
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
								<GradedAction opens="maintenance/amend">
									<Button size="small" color="info" onClick={() => setAmending(open)}>
										Amend
									</Button>
								</GradedAction>
								<GradedAction calls="maintenance/lift">
									<Button
										size="small"
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
								</GradedAction>
							</Stack>
						) : undefined
					}
				>
					<Typography variant="body2">
						{holds(open) ? (
							<>
								Under maintenance, ending{" "}
								<TimeAgo timestamp={open.expected_end} />. Checks are still
								recorded and shown. Nothing on this {scope} alerts.
							</>
						) : (
							<>
								Maintenance ended <TimeAgo timestamp={open.expected_end} />,
								watching resumes shortly.
							</>
						)}
					</Typography>
					{open.note && (
						<Typography variant="body2" sx={{ mt: 0.5, fontStyle: "italic" }}>
							{open.note}
						</Typography>
					)}
				</Alert>
			) : null}
			{/* A group's environments are a choice away in the dialog, so one
			    control declares over any of them. */}
			{/* spec: MNT#declaring */}
			{isAdmin && !open && (
				<Stack direction="row" sx={{ mb: history.length ? 2 : 0 }}>
					<GradedAction calls="maintenance/declare">
						<Button
							size="small"
							variant="outlined"
							startIcon={<BuildOutlinedIcon />}
							onClick={() => setDialogOpen(true)}
						>
							{fromMachine || fromGroup
								? `Declare for this ${scope} as well`
								: "Declare maintenance"}
						</Button>
					</GradedAction>
				</Stack>
			)}
			{environmentWindows.map((window) => (
				<Alert
					key={window.id}
					severity="info"
					icon={<BuildOutlinedIcon fontSize="inherit" />}
					sx={{ mt: 1, mb: 1 }}
					data-testid="environment-window"
					action={
						isAdmin ? (
							<Stack direction="row" spacing={1}>
								<GradedAction opens="maintenance/amend">
									<Button
										size="small"
										onClick={() => setAmending(window)}
									>
										Amend
									</Button>
								</GradedAction>
								<GradedAction calls="maintenance/lift">
									<Button
										size="small"
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
								</GradedAction>
							</Stack>
						) : undefined
					}
				>
					<Typography variant="body2">
						<Box component="span" sx={{ textTransform: "capitalize" }}>
							{window.rank}
						</Box>{" "}
						{holds(window) ? (
							<>
								under maintenance, ending{" "}
								<TimeAgo timestamp={window.expected_end} />. The rest of the
								group stays watched.
							</>
						) : (
							<>
								maintenance ended <TimeAgo timestamp={window.expected_end} />,
								watching resumes shortly.
							</>
						)}
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
					{history.map(({ window, moved_at, moved_to, rank: spanRank }) => (
						<Box
							key={`${window.id}:${moved_at ?? "here"}`}
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
								{spanRank && <ServerRankChip rank={spanRank} />}
								<Box sx={{ flex: 1 }} />
								<Typography variant="caption" color="text.secondary">
									{moved_at ? (
										<>
											moved to {moved_to ?? "another target"}{" "}
											<TimeAgo timestamp={moved_at} />
										</>
									) : (
										<>
											ended <TimeAgo timestamp={window.ended_at as string} />
											{window.ended_by
												? ` by ${window.ended_by}`
												: " at its expected end"}
										</>
									)}
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
				onDone={reload}
			/>
			{amending && (
				<DeclareMaintenanceDialog
					open
					onClose={() => setAmending(null)}
					scope={scope}
					id={id}
					existing={amending}
					onDone={reload}
				/>
			)}
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
