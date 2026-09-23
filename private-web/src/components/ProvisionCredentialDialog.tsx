import {
	Alert,
	Box,
	Button,
	Dialog,
	DialogActions,
	DialogContent,
	DialogTitle,
	IconButton,
	MenuItem,
	Stack,
	TextField,
	Tooltip,
	Typography,
} from "@mui/material";
import ContentCopyIcon from "@mui/icons-material/ContentCopy";
import DownloadIcon from "@mui/icons-material/Download";
import { useState } from "react";
import { useApiAction } from "../api";
import { GradedAction } from "./GradedAction";
import type { DeviceRole, ProvisionedCredential } from "../types";

const TRUSTABLE_ROLES: DeviceRole[] = [
	"machine",
	"releaser",
	"admin",
	"backup-restore",
	"relay",
];

/**
 * Mint a device credential (spec DPK) and present it once. The private key is
 * shown only inside the returned encrypted download and is never retrievable
 * again, so the dialog makes the "shown once" nature explicit and discards the
 * material on close.
 *
 * Two modes:
 * - `deviceId` set: provision an additional key onto that device at `role`.
 * - `deviceId` omitted: create a new device; the operator picks the role.
 */
export default function ProvisionCredentialDialog({
	open,
	onClose,
	deviceId,
	role: fixedRole,
	onProvisioned,
}: {
	open: boolean;
	onClose: () => void;
	deviceId?: string;
	role?: DeviceRole;
	onProvisioned?: (result: ProvisionedCredential) => void;
}) {
	const action = useApiAction("devices", "provision_credential");
	const [role, setRole] = useState<DeviceRole>(fixedRole ?? "machine");
	const [keyName, setKeyName] = useState("");
	const [result, setResult] = useState<ProvisionedCredential | null>(null);
	const [downloaded, setDownloaded] = useState(false);

	const roleSelectable = !fixedRole;
	const effectiveRole = fixedRole ?? role;

	const reset = () => {
		setResult(null);
		setDownloaded(false);
		setKeyName("");
		setRole(fixedRole ?? "machine");
		action.reset();
	};

	const close = () => {
		reset();
		onClose();
	};

	const onProvision = async () => {
		try {
			const res = (await action.call({
				role: effectiveRole,
				device_id: deviceId ?? null,
				key_name: keyName.trim() === "" ? null : keyName.trim(),
			})) as ProvisionedCredential;
			setResult(res);
			onProvisioned?.(res);
		} catch {
			/* surfaced via action.error */
		}
	};

	const download = () => {
		if (!result) return;
		const binary = atob(result.key_age_base64);
		const bytes = new Uint8Array(binary.length);
		for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
		const url = URL.createObjectURL(
			new Blob([bytes], { type: "application/octet-stream" }),
		);
		const a = document.createElement("a");
		a.href = url;
		a.download = result.filename;
		document.body.appendChild(a);
		a.click();
		a.remove();
		URL.revokeObjectURL(url);
		setDownloaded(true);
	};

	return (
		<Dialog open={open} onClose={close} fullWidth maxWidth="sm">
			<DialogTitle>
				{deviceId ? "Generate new key" : "Create device"}
			</DialogTitle>
			<DialogContent>
				{result ? (
					<Stack spacing={2} sx={{ mt: 1 }}>
						<Alert severity="warning">
							This key is shown once. Download it and copy the passphrase
							now — Canopy does not keep the private key and cannot show it
							again.
						</Alert>
						<Box>
							<Typography variant="caption" color="text.secondary">
								Passphrase (share out of band)
							</Typography>
							<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
								<Typography
									variant="body1"
									sx={{ fontFamily: "monospace" }}
								>
									{result.passphrase}
								</Typography>
								<Tooltip title="Copy passphrase">
									<IconButton
										size="small"
										onClick={() =>
											navigator.clipboard?.writeText(result.passphrase)
										}
									>
										<ContentCopyIcon fontSize="small" />
									</IconButton>
								</Tooltip>
							</Stack>
						</Box>
						<Button
							variant="contained"
							startIcon={<DownloadIcon />}
							onClick={download}
						>
							Download key file
						</Button>
						<Typography variant="body2" color="text.secondary">
							Decrypt on the host with{" "}
							<code>bestool crypto reveal {result.filename}</code> to recover
							the PEM.
						</Typography>
					</Stack>
				) : (
					<Stack spacing={2} sx={{ mt: 1 }}>
						<Typography variant="body2" color="text.secondary">
							Canopy mints the keypair, stores the public key, and hands back
							the private key once, encrypted under a generated passphrase.
						</Typography>
						{roleSelectable ? (
							<TextField
								select
								label="Role"
								size="small"
								value={role}
								onChange={(e) => setRole(e.target.value as DeviceRole)}
								disabled={action.pending}
							>
								{TRUSTABLE_ROLES.map((r) => (
									<MenuItem key={r} value={r}>
										{r}
									</MenuItem>
								))}
							</TextField>
						) : (
							<Typography variant="body2">
								Role: <strong>{effectiveRole}</strong>
							</Typography>
						)}
						<TextField
							label="Key name (optional)"
							size="small"
							value={keyName}
							onChange={(e) => setKeyName(e.target.value)}
							placeholder="Provisioned key"
							disabled={action.pending}
						/>
						{action.error && (
							<Alert severity="error">{action.error.message}</Alert>
						)}
					</Stack>
				)}
			</DialogContent>
			<DialogActions>
				{result ? (
					<Button onClick={close}>{downloaded ? "Done" : "Close"}</Button>
				) : (
					<>
						<Button onClick={close} disabled={action.pending}>
							Cancel
						</Button>
						<GradedAction calls="devices/provision_credential">
							<Button
								variant="contained"
								onClick={onProvision}
								disabled={action.pending}
							>
								{action.pending ? "Provisioning…" : "Provision"}
							</Button>
						</GradedAction>
					</>
				)}
			</DialogActions>
		</Dialog>
	);
}
