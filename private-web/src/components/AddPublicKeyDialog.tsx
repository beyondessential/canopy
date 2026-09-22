import {
	Alert,
	Button,
	Dialog,
	DialogActions,
	DialogContent,
	DialogTitle,
	Stack,
	TextField,
	Typography,
} from "@mui/material";
import { useState } from "react";
import { useApiAction } from "../api";
import { GradedAction } from "./GradedAction";

/**
 * Register an externally-generated public key on a device. Unlike generating a
 * key (where Canopy mints the keypair), the operator supplies the public half
 * of a keypair they already hold — Canopy never sees the private key.
 */
export default function AddPublicKeyDialog({
	open,
	onClose,
	deviceId,
	onAdded,
}: {
	open: boolean;
	onClose: () => void;
	deviceId: string;
	onAdded?: () => void;
}) {
	const action = useApiAction("devices", "add_key");
	const [pem, setPem] = useState("");
	const [name, setName] = useState("");

	const close = () => {
		setPem("");
		setName("");
		action.reset();
		onClose();
	};

	const onAdd = async () => {
		try {
			await action.call({
				device_id: deviceId,
				public_key_pem: pem,
				name: name.trim() === "" ? null : name.trim(),
			});
			onAdded?.();
			close();
		} catch {
			/* surfaced via action.error */
		}
	};

	return (
		<Dialog open={open} onClose={close} fullWidth maxWidth="sm">
			<DialogTitle>Add from public key</DialogTitle>
			<DialogContent>
				<Stack spacing={2} sx={{ mt: 1 }}>
					<Typography variant="body2" color="text.secondary">
						Register a public key the device already holds. Paste the
						PEM (<code>-----BEGIN PUBLIC KEY-----</code>). Canopy stores
						only the public key.
					</Typography>
					<TextField
						label="Public key (PEM)"
						multiline
						minRows={5}
						value={pem}
						onChange={(e) => setPem(e.target.value)}
						placeholder={"-----BEGIN PUBLIC KEY-----\n…\n-----END PUBLIC KEY-----"}
						disabled={action.pending}
						slotProps={{ htmlInput: { style: { fontFamily: "monospace" } } }}
					/>
					<TextField
						label="Key name (optional)"
						size="small"
						value={name}
						onChange={(e) => setName(e.target.value)}
						placeholder="Added key"
						disabled={action.pending}
					/>
					{action.error && (
						<Alert severity="error">{action.error.message}</Alert>
					)}
				</Stack>
			</DialogContent>
			<DialogActions>
				<Button onClick={close} disabled={action.pending}>
					Cancel
				</Button>
				<GradedAction calls="devices/add_key">
					<Button
						variant="contained"
						onClick={onAdd}
						disabled={action.pending || pem.trim() === ""}
					>
						{action.pending ? "Adding…" : "Add key"}
					</Button>
				</GradedAction>
			</DialogActions>
		</Dialog>
	);
}
