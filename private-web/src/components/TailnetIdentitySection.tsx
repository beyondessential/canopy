import {
	Alert,
	Button,
	Chip,
	Dialog,
	DialogActions,
	DialogContent,
	DialogTitle,
	LinearProgress,
	Paper,
	Stack,
	TextField,
	Typography,
} from "@mui/material";
import { useEffect, useState } from "react";
import { callApi, useApiAction } from "../api";
import { GradedAction } from "./GradedAction";
import type { DeviceInfo, TailnetLiveInfo } from "../types";
import TimeAgo from "./TimeAgo";

/// Shared between the device detail page and the server detail page.
/// On the server view, the device underlying a server is the natural
/// object to attach a Tailscale identity to — the operator shouldn't
/// have to dig through to the device admin page just to do that.
export default function TailnetIdentitySection({
	device,
	refresh,
}: {
	device: DeviceInfo;
	refresh: () => void;
}) {
	const [attachOpen, setAttachOpen] = useState(false);
	const [mergeOpen, setMergeOpen] = useState(false);
	const [confirmDetach, setConfirmDetach] = useState(false);
	const detachAction = useApiAction("devices", "detach_tailscale");

	const nodeId = device.device.tailscale_node_id;
	const live = device.tailnet_live;

	const onDetach = async () => {
		try {
			await detachAction.call({ device_id: device.device.id });
			setConfirmDetach(false);
			refresh();
		} catch {
			/* surfaced via detachAction.error */
		}
	};

	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<Stack spacing={1.5}>
				<Stack
					direction="row"
					spacing={1}
					sx={{ alignItems: "center", justifyContent: "space-between" }}
				>
					<Typography variant="h6" component="h2">
						Tailscale identity
					</Typography>
					<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
						{nodeId && live && (
							<Chip
								size="small"
								label={live.online ? "Online" : "Offline"}
								color={live.online ? "success" : "default"}
								variant={live.online ? "filled" : "outlined"}
							/>
						)}
						{nodeId ? (
							<Chip
								size="small"
								label="Attached"
								color="success"
								variant="outlined"
							/>
						) : (
							<Chip size="small" label="Unknown" variant="outlined" />
						)}
					</Stack>
				</Stack>

				{nodeId ? (
					<TailnetAttachedView device={device} live={live} />
				) : (
					<Typography variant="body2" color="text.secondary">
						This device has no Tailscale identity attached. Attach one
						to let the device authenticate over the tailnet without an
						mTLS cert.
					</Typography>
				)}

				<Stack direction="row" spacing={1} useFlexGap>
					<GradedAction calls="devices/attach_tailscale">
						<Button
							variant="contained"
							onClick={() => setAttachOpen(true)}
						>
							{nodeId ? "Replace identity" : "Attach Tailscale identity"}
						</Button>
					</GradedAction>
					{nodeId &&
						(confirmDetach ? (
							<Stack direction="row" spacing={1}>
								<GradedAction calls="devices/detach_tailscale">
									<Button
										variant="contained"
										color="error"
										onClick={onDetach}
										disabled={detachAction.pending}
									>
										{detachAction.pending
											? "Detaching…"
											: "Confirm detach"}
									</Button>
								</GradedAction>
								<Button
									variant="outlined"
									onClick={() => setConfirmDetach(false)}
									disabled={detachAction.pending}
								>
									Cancel
								</Button>
							</Stack>
						) : (
							<GradedAction calls="devices/detach_tailscale">
								<Button
									variant="outlined"
									color="error"
									onClick={() => setConfirmDetach(true)}
								>
									Detach
								</Button>
							</GradedAction>
						))}
					<GradedAction calls="devices/merge_into">
						<Button variant="outlined" onClick={() => setMergeOpen(true)}>
							Merge into existing device…
						</Button>
					</GradedAction>
				</Stack>

				{detachAction.error && (
					<Alert severity="error">{detachAction.error.message}</Alert>
				)}
			</Stack>

			<AttachTailscaleDialog
				open={attachOpen}
				onClose={() => setAttachOpen(false)}
				deviceId={device.device.id}
				onAttached={() => {
					setAttachOpen(false);
					refresh();
				}}
			/>
			<MergeIntoDialog
				open={mergeOpen}
				onClose={() => setMergeOpen(false)}
				sourceId={device.device.id}
				onMerged={() => {
					setMergeOpen(false);
					refresh();
				}}
			/>
		</Paper>
	);
}

function TailnetAttachedView({
	device,
	live,
}: {
	device: DeviceInfo;
	live: TailnetLiveInfo | null;
}) {
	const items: Array<{
		label: string;
		value: React.ReactNode;
		mono?: boolean;
	}> = [];
	items.push({
		label: "Node ID",
		value: device.device.tailscale_node_id ?? "—",
		mono: true,
	});
	items.push({
		label: "Stored name",
		value: device.device.tailscale_node_name ?? "—",
	});
	items.push({
		label: "Tailnet",
		value: device.device.tailscale_tailnet ?? "—",
	});
	if (live) {
		items.push({ label: "Current name", value: live.display_name });
		items.push({
			label: "Current IPs",
			value: live.addresses.join(", "),
			mono: true,
		});
		items.push({
			label: "Last seen",
			value: live.last_seen ? <TimeAgo timestamp={live.last_seen} /> : "—",
		});
		if (live.tags.length > 0) {
			items.push({ label: "Tags", value: live.tags.join(", "), mono: true });
		}
	} else if (device.device.tailscale_node_id) {
		items.push({
			label: "Current",
			value: "Not in directory cache (node may have left the tailnet)",
		});
	}

	return (
		<Stack direction="row" spacing={4} useFlexGap sx={{ flexWrap: "wrap" }}>
			{items.map(({ label, value, mono }) => (
				<Stack key={label} spacing={0.25}>
					<Typography variant="caption" color="text.secondary">
						{label}
					</Typography>
					<Typography
						variant="body2"
						component="div"
						sx={mono ? { fontFamily: "monospace" } : undefined}
					>
						{value}
					</Typography>
				</Stack>
			))}
		</Stack>
	);
}

function AttachTailscaleDialog({
	open,
	onClose,
	deviceId,
	onAttached,
}: {
	open: boolean;
	onClose: () => void;
	deviceId: string;
	onAttached: () => void;
}) {
	const [identifier, setIdentifier] = useState("");
	const [preview, setPreview] = useState<TailnetLiveInfo | null>(null);
	const [previewError, setPreviewError] = useState<string | null>(null);
	const [previewLoading, setPreviewLoading] = useState(false);
	const attachAction = useApiAction("devices", "attach_tailscale");

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
			await attachAction.call({ device_id: deviceId, identifier });
			onAttached();
		} catch {
			/* surfaced via attachAction.error */
		}
	};

	return (
		<Dialog open={open} onClose={onClose} fullWidth maxWidth="sm">
			<DialogTitle>Attach Tailscale identity</DialogTitle>
			<DialogContent>
				<Stack spacing={2} sx={{ mt: 1 }}>
					<TextField
						label="Tailscale identifier"
						placeholder="100.64.0.42 / nodekey:… / device.example.ts.net"
						helperText="Paste a Tailscale IP, node id, or DNS name from the admin console."
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
								{preview.tags.length > 0 && (
									<Typography variant="body2" color="text.secondary">
										Tags: {preview.tags.join(", ")}
									</Typography>
								)}
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
				<GradedAction calls="devices/attach_tailscale">
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

function MergeIntoDialog({
	open,
	onClose,
	sourceId,
	onMerged,
}: {
	open: boolean;
	onClose: () => void;
	sourceId: string;
	onMerged: () => void;
}) {
	const [targetId, setTargetId] = useState("");
	const mergeAction = useApiAction("devices", "merge_into");

	useEffect(() => {
		if (!open) {
			setTargetId("");
		}
	}, [open]);

	const onConfirm = async () => {
		try {
			await mergeAction.call({ source_id: sourceId, target_id: targetId });
			onMerged();
		} catch {
			/* surfaced via mergeAction.error */
		}
	};

	return (
		<Dialog open={open} onClose={onClose} fullWidth maxWidth="sm">
			<DialogTitle>Merge into existing device</DialogTitle>
			<DialogContent>
				<Stack spacing={2} sx={{ mt: 1 }}>
					<Typography variant="body2" color="text.secondary">
						The current device row will be folded into the target. The
						target keeps its role, server attachment, and history; this
						device's Tailscale identity moves to the target and this
						row is deleted.
					</Typography>
					<TextField
						label="Target device ID"
						placeholder="UUID of the existing device to merge into"
						value={targetId}
						onChange={(e) => setTargetId(e.target.value.trim())}
						autoFocus
						fullWidth
					/>
					{mergeAction.error && (
						<Alert severity="error">{mergeAction.error.message}</Alert>
					)}
				</Stack>
			</DialogContent>
			<DialogActions>
				<Button onClick={onClose} disabled={mergeAction.pending}>
					Cancel
				</Button>
				<GradedAction calls="devices/merge_into">
					<Button
						variant="contained"
						color="warning"
						onClick={onConfirm}
						disabled={mergeAction.pending || targetId === ""}
					>
						{mergeAction.pending ? "Merging…" : "Merge"}
					</Button>
				</GradedAction>
			</DialogActions>
		</Dialog>
	);
}
