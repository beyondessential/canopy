import {
	Accordion,
	AccordionDetails,
	AccordionSummary,
	Alert,
	Button,
	Chip,
	Dialog,
	DialogActions,
	DialogContent,
	DialogTitle,
	LinearProgress,
	Link as MuiLink,
	Paper,
	Stack,
	TextField,
	Typography,
} from "@mui/material";
import ExpandMoreIcon from "@mui/icons-material/ExpandMore";
import { useEffect, useState } from "react";
import { Link as RouterLink } from "react-router-dom";
import { callApi, useApiAction } from "../api";
import { GradedAction } from "./GradedAction";
import MachineSetupInstructions from "./MachineSetupInstructions";
import TailnetIdentitySection from "./TailnetIdentitySection";
import type { DeviceInfo, TailnetLiveInfo } from "../types";

/// The machine's identity: the device that speaks for it, its tailnet detail,
/// and re-enrolment. Folded into a collapsed-by-default accordion, since the
/// device is an internal implementation detail of enrolment rather than
/// something operators set up by hand.
///
/// This is the machine's, not any application's: an identity is bound to the
/// box, and one box can carry several workloads.
export default function MachineIdentitySection({
	machineId,
	deviceInfo,
	isAdmin,
	enrolled,
	refresh,
}: {
	machineId: string;
	deviceInfo: DeviceInfo | null;
	isAdmin: boolean;
	enrolled: boolean;
	refresh: () => void;
}) {
	return (
		<Stack spacing={2}>
			<Accordion variant="outlined" disableGutters>
				<AccordionSummary expandIcon={<ExpandMoreIcon />}>
					<Typography variant="h6" component="h2">
						Identity
					</Typography>
				</AccordionSummary>
				<AccordionDetails>
					<Stack spacing={2}>
						<DeviceCard
							machineId={machineId}
							deviceInfo={deviceInfo}
							refresh={refresh}
						/>
						{deviceInfo && (
							<TailnetIdentitySection
								device={deviceInfo}
								refresh={refresh}
							/>
						)}
						{isAdmin && enrolled && (
							<MachineSetupInstructions
								machineId={machineId}
								reEnroll
								onRegistered={refresh}
							/>
						)}
					</Stack>
				</AccordionDetails>
			</Accordion>
		</Stack>
	);
}

function DeviceCard({
	machineId,
	deviceInfo,
	refresh,
}: {
	machineId: string;
	deviceInfo: DeviceInfo | null;
	refresh: () => void;
}) {
	const [attachOpen, setAttachOpen] = useState(false);
	return (
		<Stack
			direction={{ xs: "column", md: "row" }}
			spacing={2}
			useFlexGap
			sx={{ "& > *": { flex: 1 } }}
		>
			<Paper variant="outlined" sx={{ p: 2 }}>
				<Typography variant="h6" component="h2" gutterBottom>
					Device
				</Typography>
				{deviceInfo ? (
					<Stack
						direction="row"
						spacing={1}
						sx={{ alignItems: "center", flexWrap: "wrap" }}
						useFlexGap
					>
						<MuiLink
							component={RouterLink}
							to={`/devices/${deviceInfo.device.id}`}
							underline="hover"
						>
							{deviceShortName(deviceInfo)}
						</MuiLink>
						{deviceInfo.device.tailscale_node_id != null && (
							<Chip
								size="small"
								variant="outlined"
								color="success"
								label="tailnet"
							/>
						)}
						{deviceInfo.keys.length > 0 && (
							<Chip
								size="small"
								variant="outlined"
								label="mTLS"
							/>
						)}
					</Stack>
				) : (
					<Stack spacing={1} sx={{ alignItems: "flex-start" }}>
						<Typography variant="body2" color="text.secondary">
							No device attached. Bind this machine to a Tailscale
							node and canopy will auto-create the device row if
							it doesn't exist yet.
						</Typography>
						<GradedAction calls="fleet/machines/attach_tailscale_device">
							<Button
								variant="contained"
								onClick={() => setAttachOpen(true)}
							>
								Attach Tailscale device
							</Button>
						</GradedAction>
					</Stack>
				)}
			</Paper>
			<AttachMachineDeviceDialog
				open={attachOpen}
				onClose={() => setAttachOpen(false)}
				machineId={machineId}
				onAttached={() => {
					setAttachOpen(false);
					refresh();
				}}
			/>
		</Stack>
	);
}

function AttachMachineDeviceDialog({
	open,
	onClose,
	machineId,
	onAttached,
}: {
	open: boolean;
	onClose: () => void;
	machineId: string;
	onAttached: () => void;
}) {
	const [identifier, setIdentifier] = useState("");
	const [preview, setPreview] = useState<TailnetLiveInfo | null>(null);
	const [previewError, setPreviewError] = useState<string | null>(null);
	const [previewLoading, setPreviewLoading] = useState(false);
	const attachAction = useApiAction("fleet/machines", "attach_tailscale_device");

	useEffect(() => {
		if (!open) {
			setIdentifier("");
			setPreview(null);
			setPreviewError(null);
		}
	}, [open]);

	useEffect(() => {
		const value = identifier.trim();
		if (!open || value === "") {
			setPreview(null);
			setPreviewError(null);
			return;
		}
		let cancelled = false;
		setPreviewLoading(true);
		const handle = setTimeout(async () => {
			try {
				const r = await callApi(
					"devices",
					"resolve_tailnet_identifier",
					{ identifier: value },
				);
				if (cancelled) return;
				setPreview(r.matched);
				setPreviewError(
					r.matched ? null : "No tailnet node matches that identifier.",
				);
			} catch (err) {
				if (cancelled) return;
				setPreview(null);
				setPreviewError(err instanceof Error ? err.message : String(err));
			} finally {
				if (!cancelled) setPreviewLoading(false);
			}
		}, 250);
		return () => {
			cancelled = true;
			clearTimeout(handle);
		};
	}, [identifier, open]);

	const onConfirm = async () => {
		try {
			await attachAction.call({ machine_id: machineId, identifier });
			onAttached();
		} catch {
			/* surfaced via attachAction.error */
		}
	};

	return (
		<Dialog open={open} onClose={onClose} fullWidth maxWidth="sm">
			<DialogTitle>Attach Tailscale device to machine</DialogTitle>
			<DialogContent>
				<Stack spacing={2} sx={{ mt: 1 }}>
					<Typography variant="body2" color="text.secondary">
						Paste any Tailscale identifier — IP, node ID, or DNS
						name. If a device row exists for that node, it's
						attached to the machine; otherwise canopy creates a new
						device row first.
					</Typography>
					<TextField
						label="Tailscale identifier"
						placeholder="100.64.0.42 / nodekey:… / device.example.ts.net"
						value={identifier}
						onChange={(e) => setIdentifier(e.target.value)}
						autoFocus
						fullWidth
					/>
					{previewLoading && <LinearProgress />}
					{preview && (
						<Paper variant="outlined" sx={{ p: 1.5 }}>
							<Stack spacing={0.5}>
								<Typography variant="caption" color="text.secondary">
									Resolves to
								</Typography>
								<Typography variant="body2">
									{preview.display_name}
								</Typography>
								<Typography
									variant="body2"
									color="text.secondary"
									sx={{ fontFamily: "monospace" }}
								>
									{preview.node_id}
								</Typography>
								<Typography variant="body2" color="text.secondary">
									{preview.addresses.join(", ")}
								</Typography>
							</Stack>
						</Paper>
					)}
					{previewError && identifier.trim() !== "" && (
						<Alert severity="info">{previewError}</Alert>
					)}
					{attachAction.error && (
						<Alert severity="error">{attachAction.error.message}</Alert>
					)}
				</Stack>
			</DialogContent>
			<DialogActions>
				<Button onClick={onClose} disabled={attachAction.pending}>
					Cancel
				</Button>
				<GradedAction calls="fleet/machines/attach_tailscale_device">
					<Button
						variant="contained"
						onClick={onConfirm}
						disabled={
							attachAction.pending ||
							preview === null ||
							identifier.trim() === ""
						}
					>
						{attachAction.pending ? "Attaching…" : "Attach"}
					</Button>
				</GradedAction>
			</DialogActions>
		</Dialog>
	);
}

function deviceShortName(info: DeviceInfo): string {
	const namedKey = info.keys.findLast(
		(k) => k.name && k.name !== "Initial Key",
	);
	if (namedKey?.name) return namedKey.name;
	if (info.latest_connection) return info.latest_connection.ip;
	return info.device.id;
}
