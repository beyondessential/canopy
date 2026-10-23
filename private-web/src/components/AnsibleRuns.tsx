import {
	Alert,
	Box,
	Button,
	Chip,
	Stack,
	Tooltip,
	Typography,
} from "@mui/material";
import { useState } from "react";
import { useApi } from "../api";
import type {
	AnsibleLease,
	CatalogueView,
	MaintenanceWindow,
	ServerRank,
} from "../types";
import AnsibleRunDialog from "./AnsibleRunDialog";
import { GradedAction } from "./GradedAction";
import TimeAgo from "./TimeAgo";
import WindowEnd from "./WindowEnd";

type Application = { machine_id: string; type: string };
type Machine = { id: string };

/// The ways to start a run on an environment, and who holds it where someone
/// does: an install, an upgrade, and any playbook the catalogue lists.
// spec: ANSRUN#presentation
export default function AnsibleRuns({
	groupId,
	groupName,
	rank,
	machines,
	applications,
	catalogue,
	maintenanceTick,
	onChanged,
}: {
	groupId: string;
	groupName: string;
	rank: ServerRank;
	machines: ReadonlyArray<Machine>;
	applications: ReadonlyArray<Application>;
	catalogue: CatalogueView | null;
	maintenanceTick: number;
	onChanged: () => void;
}) {
	const [dialog, setDialog] = useState<{
		preset: "install" | "upgrade" | null;
		takeOver: boolean;
	} | null>(null);
	const [tick, setTick] = useState(0);
	const state = useApi(
		"ansible",
		"run_state",
		{ server_group_id: groupId, rank },
		[groupId, rank, maintenanceTick, tick],
	);

	const ids = new Set(machines.map((machine) => machine.id));
	const here = applications.filter((application) => ids.has(application.machine_id));
	const types = new Set(here.map((application) => application.type));
	const hasTamanu = [...types].some((type) => type.startsWith("tamanu-"));
	// A machine nothing is on counts only where a run can be served it, which
	// is what the run options list.
	const options = useApi("ansible", "run_options", { server_group_id: groupId, rank }, [
		groupId,
		rank,
		maintenanceTick,
		tick,
	]);
	const anyEmpty =
		options.status === "ok" &&
		(options.data as unknown as { members: { application_types: string[] }[] }).members.some(
			(member) => member.application_types.length === 0,
		);

	const playbooks = (catalogue?.playbooks ?? []).filter(
		(playbook) =>
			anyEmpty ||
			!playbook.applications ||
			playbook.applications.some((type) => types.has(type)),
	);
	const presetFor = (preset: "install" | "upgrade") =>
		catalogue?.playbooks.find((playbook) => playbook.preset === preset);

	const lease: (AnsibleLease & { playbook?: string | null; started_at?: string | null }) | null =
		state.status === "ok" ? state.data.lease : null;
	const window: MaintenanceWindow | null =
		state.status === "ok" ? state.data.window : null;
	const refuses = state.status === "ok" && state.data.refuses;
	const leaseIsYours = state.status === "ok" && state.data.lease_is_yours;
	const held = lease !== null || refuses;
	// A window speaks through its standing amendment where it has one.
	const holder =
		lease?.held_by ?? (refuses ? (window?.amended_by ?? window?.declared_by ?? null) : null);

	const changed = () => {
		setTick((n) => n + 1);
		onChanged();
	};
	const available = catalogue !== null && !held;

	const button = (label: string, preset: "install" | "upgrade" | null, enabled: boolean) => (
		<GradedAction calls="ansible/take_lease" action={`${label} on ${groupName} ${rank}`}>
			<Button
				size="small"
				variant="outlined"
				disabled={!enabled}
				onClick={() => setDialog({ preset, takeOver: false })}
				data-testid={`run-${preset ?? "any"}`}
			>
				{label}
			</Button>
		</GradedAction>
	);

	return (
		<Box data-testid="ansible-run">
			<Stack direction="row" spacing={1} useFlexGap sx={{ flexWrap: "wrap" }}>
				{hasTamanu && presetFor("install") && button("Run install", "install", available)}
				{hasTamanu && presetFor("upgrade") && button("Run upgrade", "upgrade", available)}
				{button("Run…", null, available && playbooks.length > 0)}
			</Stack>

			{catalogue === null && (
				<Typography
					variant="caption"
					color="text.secondary"
					sx={{ display: "block", mt: 1 }}
					data-testid="run-needs-catalogue"
				>
					Running a playbook needs the catalogue, which Canopy has not read yet.
				</Typography>
			)}

			{state.status === "error" && (
				<Alert severity="warning" sx={{ mt: 1 }}>
					{state.error.message}
				</Alert>
			)}

			{held && (
				<Stack
					direction="row"
					spacing={1}
					sx={{ mt: 1, alignItems: "center", flexWrap: "wrap" }}
					data-testid="run-holder"
				>
					<Typography variant="body2" data-testid="run-leased">
						<b style={{ fontWeight: 500 }}>{holder ?? "An operator"}</b>{" "}
						{lease ? (
							<>
								{lease.playbook ? (
									<>
										is running <code>{lease.playbook}</code>
									</>
								) : lease.intent === "upgrade" ? (
									"is upgrading"
								) : (
									"is configuring"
								)}
								{lease.started_at ? (
									<>
										, started <TimeAgo timestamp={lease.started_at} />
									</>
								) : (
									", starting"
								)}
								{lease.note ? `: ${lease.note}` : ""}
							</>
						) : (
							<>
								has declared work here
								{window ? <>, ending <WindowEnd window={window} /></> : null}
							</>
						)}
					</Typography>
					{(lease !== null || window !== null) && (
						<Chip size="small" label="Maintenance" color="warning" variant="outlined" />
					)}
					{lease !== null &&
							!leaseIsYours &&
							catalogue !== null &&
							playbooks.length > 0 && (
						<Tooltip title="Take the lease over from them">
							<span>
								<GradedAction calls="ansible/take_lease" action="Take over the lease">
									<Button
										size="small"
										onClick={() => setDialog({ preset: null, takeOver: true })}
										data-testid="run-take-over-open"
									>
										Take over
									</Button>
								</GradedAction>
							</span>
						</Tooltip>
					)}
				</Stack>
			)}

			{catalogue !== null && dialog !== null && (
				<AnsibleRunDialog
					open
					onClose={() => setDialog(null)}
					groupId={groupId}
					rank={rank}
					catalogue={catalogue}
					preset={dialog.preset}
					playbooks={playbooks}
					takeOver={dialog.takeOver}
					holder={holder}
					onChanged={changed}
				/>
			)}
		</Box>
	);
}
