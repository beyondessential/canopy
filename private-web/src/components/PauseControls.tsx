import {
	Alert,
	AlertTitle,
	Box,
	Button,
	Dialog,
	DialogActions,
	DialogContent,
	DialogContentText,
	DialogTitle,
	TextField,
	Typography,
} from "@mui/material";
import PauseCircleIcon from "@mui/icons-material/PauseCircle";
import PlayCircleIcon from "@mui/icons-material/PlayCircle";
import { useState } from "react";
import { useApiAction } from "../api";
import type { PauseView } from "../types";
import { GradedAction } from "./GradedAction";
import TimeAgo from "./TimeAgo";

/// The pause on an application, shown in both of its sections (DNS names and
/// TLS certificates): a pause suppresses the alerting that would otherwise
/// chase a certificate running out, so it has to be the thing an operator sees
/// first, wherever the application's DNS names or certificates are presented.
// spec: DNS#pausing-an-application
export function PauseBanner({
	serverId,
	pause,
	isAdmin,
	onChanged,
}: {
	serverId: string;
	pause: PauseView;
	isAdmin: boolean;
	onChanged: () => void;
}) {
	const resume = useApiAction("dns_names", "resume");
	const onResume = async () => {
		if (
			!window.confirm(
				"Resume this server? Canopy will start ordering and renewing its certificates again, and publishing its address records.",
			)
		)
			return;
		try {
			await resume.call({ server_id: serverId });
			onChanged();
		} catch {
			/* surfaced via resume.error */
		}
	};

	return (
		<Alert
			severity="warning"
			icon={<PauseCircleIcon />}
			action={
				isAdmin && (
					<GradedAction
						calls="dns_names/resume"
						action="Resume certificate management for this server"
					>
						<Button
							size="small"
							startIcon={<PlayCircleIcon />}
							onClick={onResume}
							disabled={resume.pending}
						>
							Resume
						</Button>
					</GradedAction>
				)
			}
		>
			<AlertTitle>Paused</AlertTitle>
			Canopy is making no new changes for this server: nothing is ordered,
			renewed, or republished. What is already in place stands and keeps working.
			<Box sx={{ mt: 0.5 }}>
				<Typography variant="caption" color="text.secondary">
					{pause.paused_at && (
						<>
							since <TimeAgo timestamp={pause.paused_at} />
						</>
					)}
					{pause.paused_by && ` by ${pause.paused_by}`}
					{pause.reason && ` — ${pause.reason}`}
				</Typography>
			</Box>
			{resume.error && (
				<Alert severity="error" sx={{ mt: 1 }}>
					{resume.error.message}
				</Alert>
			)}
		</Alert>
	);
}

export function PauseButton({
	serverId,
	onChanged,
}: {
	serverId: string;
	onChanged: () => void;
}) {
	const [open, setOpen] = useState(false);
	const [reason, setReason] = useState("");
	const pause = useApiAction("dns_names", "pause");

	const onConfirm = async () => {
		try {
			await pause.call({ server_id: serverId, reason: reason.trim() });
			setOpen(false);
			setReason("");
			onChanged();
		} catch {
			/* surfaced via pause.error */
		}
	};

	return (
		<>
			<GradedAction
				calls="dns_names/pause"
				action="Pause certificate management for this server"
			>
				<Button
					size="small"
					startIcon={<PauseCircleIcon />}
					onClick={() => setOpen(true)}
				>
					Pause
				</Button>
			</GradedAction>
			<Dialog open={open} onClose={() => setOpen(false)} fullWidth maxWidth="sm">
				<DialogTitle>Pause this server</DialogTitle>
				<DialogContent>
					<DialogContentText sx={{ mb: 2 }}>
						Canopy will stop ordering and renewing certificates for this server,
						and stop changing its address records. Nothing already in place is
						withdrawn — the group keeps working exactly as it does now.
						Canopy never lifts a pause itself.
					</DialogContentText>
					<TextField
						autoFocus
						fullWidth
						label="Reason"
						size="small"
						value={reason}
						onChange={(e) => setReason(e.target.value)}
						disabled={pause.pending}
						helperText="Recorded on the server, so whoever finds the pause later knows what it was for."
					/>
					{pause.error && (
						<Alert severity="error" sx={{ mt: 2 }}>
							{pause.error.message}
						</Alert>
					)}
				</DialogContent>
				<DialogActions>
					<Button onClick={() => setOpen(false)}>Cancel</Button>
					<GradedAction
						calls="dns_names/pause"
						action="Pause certificate management for this server"
					>
						<Button
							variant="contained"
							onClick={onConfirm}
							disabled={pause.pending || reason.trim() === ""}
						>
							Pause
						</Button>
					</GradedAction>
				</DialogActions>
			</Dialog>
		</>
	);
}
