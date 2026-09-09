import {
	Alert,
	Button,
	Dialog,
	DialogActions,
	DialogContent,
	DialogContentText,
	DialogTitle,
	Stack,
	TextField,
	ToggleButton,
	ToggleButtonGroup,
} from "@mui/material";
import { useEffect, useState } from "react";
import { useApiAction } from "../api";
import type { MaintenanceScope, MaintenanceWindow, ServerRank } from "../types";

const PRESETS = [1, 2, 4, 8];

/** `datetime-local` wants a local wall clock with no zone, which is what
 * the operator is thinking in when they say "back by six". */
function toLocalInput(at: Date): string {
	const pad = (n: number) => String(n).padStart(2, "0");
	return `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())}T${pad(at.getHours())}:${pad(at.getMinutes())}`;
}

function hoursFromNow(hours: number): string {
	return toLocalInput(new Date(Date.now() + hours * 3600_000));
}

/** Declare a maintenance window over an application, a machine, a group, or one
 * of a group's environments, or amend the one it already has. Everything on the
 * target grades to skipped while the window holds, so nothing on it opens or
 * joins an incident. */
export default function DeclareMaintenanceDialog({
	open,
	onClose,
	scope,
	id,
	rank,
	targetLabel,
	existing,
	prefill,
	onDone,
}: {
	open: boolean;
	onClose: () => void;
	scope: MaintenanceScope;
	id: string;
	/** With a group, narrows the window to the machines serving the group's
	 * applications at this rank. */
	rank?: ServerRank;
	targetLabel?: string;
	/** The target's open window, when this is an amendment. */
	existing?: MaintenanceWindow | null;
	/** Starting values where something else knows them, such as an upgrade
	 * plan's window and note. */
	prefill?: { expectedEnd?: string; note?: string };
	onDone: () => void;
}) {
	const declare = useApiAction("maintenance", "declare");
	const [endsAt, setEndsAt] = useState(() => hoursFromNow(2));
	const [note, setNote] = useState("");

	useEffect(() => {
		if (!open) return;
		const end = existing?.expected_end ?? prefill?.expectedEnd;
		setEndsAt(end ? toLocalInput(new Date(end)) : hoursFromNow(2));
		setNote(existing?.note ?? prefill?.note ?? "");
		declare.reset();
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [open, existing?.id]);

	const submit = async () => {
		const at = new Date(endsAt);
		if (Number.isNaN(at.getTime())) return;
		try {
			await declare.call({
				...(scope === "application"
					? { application_id: id }
					: scope === "machine"
						? { machine_id: id }
						: { server_group_id: id, rank: rank ?? existing?.rank ?? null }),
				expected_end: at.toISOString(),
				note: note.trim() === "" ? null : note.trim(),
			});
			onDone();
			onClose();
		} catch {
			/* surfaced via declare.error */
		}
	};

	const amending = Boolean(existing);
	return (
		<Dialog open={open} onClose={onClose} fullWidth maxWidth="sm">
			<DialogTitle>
				{amending ? "Amend maintenance" : "Declare maintenance"}
				{targetLabel ? ` — ${targetLabel}` : ""}
			</DialogTitle>
			<DialogContent>
				<DialogContentText sx={{ mb: 2 }}>
					{scope === "group" && rank
						? `Every check on the group's ${rank} machines is suspended: nothing opens or joins an incident, and nothing notifies. The rest of the group stays watched.`
						: scope === "group"
							? "Every check on this group and its machines is suspended: nothing opens or joins an incident, and nothing notifies."
							: scope === "machine"
								? "Every check on this machine and the applications on it is suspended: nothing opens or joins an incident, and nothing notifies."
								: "Every check on this application is suspended: nothing opens or joins an incident, and nothing notifies. The machine it runs on, and anything else on it, stays watched."}{" "}
					The window ends itself at the time below, and watching resumes a
					few minutes later once the reporters have been heard from.
				</DialogContentText>
				<Stack spacing={2}>
					<ToggleButtonGroup
						size="small"
						exclusive
						value={null}
						onChange={(_, hours: number | null) => {
							if (hours) setEndsAt(hoursFromNow(hours));
						}}
					>
						{PRESETS.map((hours) => (
							<ToggleButton key={hours} value={hours}>
								{hours}h
							</ToggleButton>
						))}
					</ToggleButtonGroup>
					<TextField
						size="small"
						type="datetime-local"
						label="Expected to end"
						value={endsAt}
						onChange={(e) => setEndsAt(e.target.value)}
						slotProps={{ inputLabel: { shrink: true } }}
					/>
					<TextField
						size="small"
						label="What's being done"
						placeholder="Upgrading to 2.62"
						multiline
						minRows={2}
						value={note}
						onChange={(e) => setNote(e.target.value)}
					/>
					{declare.error && (
						<Alert severity="error">{declare.error.message}</Alert>
					)}
				</Stack>
			</DialogContent>
			<DialogActions>
				<Button onClick={onClose}>Cancel</Button>
				<Button
					variant="contained"
					onClick={submit}
					disabled={declare.pending || endsAt === ""}
				>
					{amending ? "Amend" : "Declare"}
				</Button>
			</DialogActions>
		</Dialog>
	);
}
