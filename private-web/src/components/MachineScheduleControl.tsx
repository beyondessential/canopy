//! A machine's own schedule for a backup type: the button that opens its
//! editor, and the editor, which says what is being replaced when the machine
//! has no override of its own.
// spec: BKO#editing-schedules

import {
	Alert,
	Button,
	Dialog,
	DialogActions,
	DialogContent,
	DialogTitle,
	Stack,
	Typography,
} from "@mui/material";
import { useState } from "react";
import { useApiAction } from "../api";
import type { MachineBackupCapabilityView } from "../types";
import {
	ScheduleEditor,
	ScheduleHistory,
	ScheduleLayerChip,
	ScheduleSummary,
	useScheduleState,
} from "./BackupSchedule";
import { GradedAction } from "./GradedAction";

/** The Edit / Override button for one machine and type, with its dialog. */
export default function MachineScheduleControl({
	machineId,
	machineName,
	cap,
	onChanged,
}: {
	machineId: string;
	machineName?: string;
	cap: MachineBackupCapabilityView;
	onChanged: () => void;
}) {
	const [open, setOpen] = useState(false);
	const overridden = cap.schedule.layer === "machine";
	return (
		<>
			<GradedAction
				opens={[
					"backups/set_machine_schedule",
					"backups/clear_machine_schedule",
				]}
				action={`${overridden ? "Edit" : "Override"} ${cap.type} schedule${machineName ? ` for ${machineName}` : ""}`}
			>
				<Button
					size="small"
					aria-label={`${overridden ? "Edit" : "Override"} ${cap.type} schedule${machineName ? ` for ${machineName}` : ""}`}
					onClick={() => setOpen(true)}
				>
					{overridden ? "Edit override" : "Override"}
				</Button>
			</GradedAction>
			{open && (
				<MachineScheduleDialog
					machineId={machineId}
					machineName={machineName}
					cap={cap}
					onClose={() => setOpen(false)}
					onChanged={() => {
						setOpen(false);
						onChanged();
					}}
				/>
			)}
		</>
	);
}

function MachineScheduleDialog({
	machineId,
	machineName,
	cap,
	onClose,
	onChanged,
}: {
	machineId: string;
	machineName?: string;
	cap: MachineBackupCapabilityView;
	onClose: () => void;
	onChanged: () => void;
}) {
	const set = useApiAction("backups", "set_machine_schedule");
	const clear = useApiAction("backups", "clear_machine_schedule");
	const overridden = cap.schedule.layer === "machine";
	const state = useScheduleState(cap.schedule.schedule, {
		type: cap.type,
		machineId,
	});

	const pending = set.pending || clear.pending;
	const error = set.error || clear.error;

	const save = async () => {
		if (!state.schedule) return;
		try {
			await set.call({
				machine_id: machineId,
				type: cap.type,
				schedule: state.schedule,
			});
			onChanged();
		} catch {
			/* surfaced via set.error */
		}
	};
	const reset = async () => {
		try {
			await clear.call({ machine_id: machineId, type: cap.type });
			onChanged();
		} catch {
			/* surfaced via clear.error */
		}
	};

	return (
		<Dialog open onClose={pending ? undefined : onClose} fullWidth maxWidth="sm">
			<DialogTitle>
				<Typography component="span" sx={{ fontFamily: "monospace" }}>
					{cap.type}
				</Typography>{" "}
				schedule{machineName ? ` for ${machineName}` : ""}
			</DialogTitle>
			<DialogContent>
				<Stack spacing={2} sx={{ pt: 1 }}>
					{overridden ? (
						<Typography variant="body2" color="text.secondary">
							This machine has its own schedule. Resetting it falls back to the
							group's, or the fleet default.
						</Typography>
					) : (
						<Alert severity="info" icon={false}>
							<Stack
								direction="row"
								spacing={1}
								sx={{ alignItems: "center", flexWrap: "wrap" }}
							>
								<span>Following</span>
								<ScheduleLayerChip layer={cap.schedule.layer} />
								<span>
									<ScheduleSummary
										schedule={cap.schedule.schedule}
										zone={cap.schedule.zone}
									/>
									. An override replaces it whole.
								</span>
							</Stack>
						</Alert>
					)}
					<ScheduleEditor state={state} disabled={pending} />
					<ScheduleHistory layer="machine" type={cap.type} machineId={machineId} />
					{error && <Alert severity="error">{error.message}</Alert>}
				</Stack>
			</DialogContent>
			<DialogActions>
				{overridden && (
					<GradedAction
						calls="backups/clear_machine_schedule"
						action={`Reset ${cap.type} schedule${machineName ? ` for ${machineName}` : ""} to inherited`}
					>
						<Button onClick={reset} disabled={pending}>
							Reset to inherited
						</Button>
					</GradedAction>
				)}
				<Button onClick={onClose} disabled={pending}>
					Cancel
				</Button>
				<GradedAction
					calls="backups/set_machine_schedule"
					action={`Save ${cap.type} schedule override${machineName ? ` for ${machineName}` : ""}`}
				>
					<Button
						variant="contained"
						onClick={save}
						disabled={pending || !state.schedule}
					>
						{set.pending ? "Saving…" : "Save override"}
					</Button>
				</GradedAction>
			</DialogActions>
		</Dialog>
	);
}
