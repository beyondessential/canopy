import {
	Alert,
	Box,
	Button,
	Chip,
	CircularProgress,
	Dialog,
	DialogActions,
	DialogContent,
	DialogContentText,
	DialogTitle,
	IconButton,
	LinearProgress,
	Snackbar,
	Stack,
	Table,
	TableBody,
	TableCell,
	TableHead,
	TableRow,
	TextField,
	Tooltip,
	Typography,
} from "@mui/material";
import ContentCopyIcon from "@mui/icons-material/ContentCopy";
import DeleteIcon from "@mui/icons-material/Delete";
import DownloadIcon from "@mui/icons-material/Download";
import ReplayIcon from "@mui/icons-material/Replay";
import { type FormEvent, useEffect, useState } from "react";
import { ApiError, callApi, useApi } from "../api";
import type { ProvisionedCredential } from "../types";
import { usePageTitle } from "../hooks/usePageTitle";

type ClusterView = {
	id: string;
	name: string;
	relay_identity_id: string;
	registered: boolean;
	registered_at?: string | null;
	last_answered_at?: string | null;
	answering: boolean;
};

/// How often the wizard re-checks whether the draft's relay has connected. The
/// hub stamps on connect and on its own probe cadence, so this only needs to be
/// responsive enough to feel live.
const POLL_MS = 3000;

export default function KubernetesClusters() {
	usePageTitle("Clusters");
	const list = useApi("kubernetes_clusters", "list");
	const [wizardOpen, setWizardOpen] = useState(false);
	const [toast, setToast] = useState<string | null>(null);
	const [error, setError] = useState<string | null>(null);
	const [confirmRemove, setConfirmRemove] = useState<ClusterView | null>(null);
	const [reissued, setReissued] = useState<ProvisionedCredential | null>(null);

	const onRemove = async (cluster: ClusterView) => {
		setConfirmRemove(null);
		try {
			await callApi("kubernetes_clusters", "remove", { id: cluster.id });
			setToast(`Removed "${cluster.name}"`);
			list.reload();
		} catch (err) {
			setError(formatError(err));
		}
	};

	const onReissue = async (cluster: ClusterView) => {
		try {
			const res = await callApi("kubernetes_clusters", "reissue", {
				id: cluster.id,
			});
			setReissued(res as ProvisionedCredential);
		} catch (err) {
			setError(formatError(err));
		}
	};

	return (
		<Stack spacing={3}>
			<Typography variant="body2" color="text.secondary">
				Kubernetes clusters Canopy monitors through a relay running inside
				each one. Registering a cluster mints its relay's credential; the
				cluster is saved once that relay connects and answers.
			</Typography>

			<Box>
				<Button variant="contained" onClick={() => setWizardOpen(true)}>
					Register cluster
				</Button>
			</Box>

			{error && <Alert severity="error">{error}</Alert>}

			{list.status === "loading" || list.status === "idle" ? (
				<LinearProgress />
			) : list.status === "error" ? (
				<Alert severity="error">{list.error.message}</Alert>
			) : (
				<>
					<ClusterSection
						title="Registered"
						empty="No clusters registered."
						clusters={list.data.registered}
						onRemove={setConfirmRemove}
						onReissue={onReissue}
					/>
					{list.data.drafts.length > 0 && (
						<ClusterSection
							title="Drafts"
							subtitle="Registrations waiting on the relay to connect and answer."
							empty=""
							clusters={list.data.drafts}
							onRemove={setConfirmRemove}
							onReissue={onReissue}
							onCheck={async (c) => {
								try {
									const res = (await callApi(
										"kubernetes_clusters",
										"confirm",
										{ id: c.id },
									)) as ClusterView;
									if (res.registered) setToast(`"${c.name}" registered`);
									else setToast(`"${c.name}" is not answering yet`);
									list.reload();
								} catch (err) {
									setError(formatError(err));
								}
							}}
						/>
					)}
				</>
			)}

			<RegisterWizard
				open={wizardOpen}
				onClose={() => {
					setWizardOpen(false);
					list.reload();
				}}
			/>

			<Dialog open={!!reissued} maxWidth="sm" fullWidth>
				<DialogTitle>New relay credential</DialogTitle>
				<DialogContent>
					{reissued && <CredentialReveal credential={reissued} />}
				</DialogContent>
				<DialogActions>
					<Button onClick={() => setReissued(null)}>Done</Button>
				</DialogActions>
			</Dialog>

			<Dialog open={!!confirmRemove} onClose={() => setConfirmRemove(null)}>
				<DialogTitle>Remove "{confirmRemove?.name}"?</DialogTitle>
				<DialogContent>
					<DialogContentText>
						Canopy stops monitoring this cluster. The relay's identity is
						removed with it.
					</DialogContentText>
				</DialogContent>
				<DialogActions>
					<Button onClick={() => setConfirmRemove(null)}>Cancel</Button>
					<Button
						color="error"
						onClick={() => confirmRemove && onRemove(confirmRemove)}
					>
						Remove
					</Button>
				</DialogActions>
			</Dialog>

			<Snackbar
				open={!!toast}
				autoHideDuration={3000}
				onClose={() => setToast(null)}
				message={toast ?? ""}
			/>
		</Stack>
	);
}

function ClusterSection({
	title,
	subtitle,
	empty,
	clusters,
	onRemove,
	onReissue,
	onCheck,
}: {
	title: string;
	subtitle?: string;
	empty: string;
	clusters: ClusterView[];
	onRemove: (c: ClusterView) => void;
	onReissue: (c: ClusterView) => void;
	onCheck?: (c: ClusterView) => void;
}) {
	return (
		<Box>
			<Typography variant="h6" component="h2" gutterBottom>
				{title}
			</Typography>
			{subtitle && (
				<Typography variant="body2" color="text.secondary" sx={{ mb: 1 }}>
					{subtitle}
				</Typography>
			)}
			{clusters.length === 0 ? (
				empty ? (
					<Alert severity="info">{empty}</Alert>
				) : null
			) : (
				<Table size="small">
					<TableHead>
						<TableRow>
							<TableCell>Name</TableCell>
							<TableCell>Relay</TableCell>
							<TableCell>Last answered</TableCell>
							<TableCell align="right" />
						</TableRow>
					</TableHead>
					<TableBody>
						{clusters.map((c) => (
							<TableRow key={c.id} hover>
								<TableCell>{c.name}</TableCell>
								<TableCell>
									<ConnectionChip cluster={c} />
								</TableCell>
								<TableCell>
									{c.last_answered_at
										? formatDate(c.last_answered_at)
										: "never"}
								</TableCell>
								<TableCell align="right">
									{onCheck && (
										<Tooltip title="Check connection">
											<IconButton
												size="small"
												aria-label={`check ${c.name}`}
												onClick={() => onCheck(c)}
											>
												<ReplayIcon fontSize="small" />
											</IconButton>
										</Tooltip>
									)}
									<Tooltip title="Re-issue credential">
										<IconButton
											size="small"
											aria-label={`reissue ${c.name}`}
											onClick={() => onReissue(c)}
										>
											<DownloadIcon fontSize="small" />
										</IconButton>
									</Tooltip>
									<Tooltip title="Remove">
										<IconButton
											size="small"
											aria-label={`remove ${c.name}`}
											onClick={() => onRemove(c)}
										>
											<DeleteIcon fontSize="small" />
										</IconButton>
									</Tooltip>
								</TableCell>
							</TableRow>
						))}
					</TableBody>
				</Table>
			)}
		</Box>
	);
}

function ConnectionChip({ cluster }: { cluster: ClusterView }) {
	if (cluster.answering) {
		return <Chip size="small" color="success" label="answering" />;
	}
	if (cluster.registered) {
		return <Chip size="small" color="warning" label="not answering" />;
	}
	return <Chip size="small" label="awaiting relay" />;
}

function RegisterWizard({
	open,
	onClose,
}: {
	open: boolean;
	onClose: () => void;
}) {
	const [name, setName] = useState("");
	const [pending, setPending] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const [started, setStarted] = useState<{
		clusterId: string;
		credential: ProvisionedCredential;
	} | null>(null);
	const [registered, setRegistered] = useState(false);

	const reset = () => {
		setName("");
		setPending(false);
		setError(null);
		setStarted(null);
		setRegistered(false);
	};

	const close = () => {
		reset();
		onClose();
	};

	const onRegister = async (e: FormEvent) => {
		e.preventDefault();
		const trimmed = name.trim();
		if (!trimmed) return setError("A cluster needs a name");
		setPending(true);
		setError(null);
		try {
			const res = await callApi("kubernetes_clusters", "register", {
				name: trimmed,
			});
			setStarted({
				clusterId: res.cluster.id,
				credential: res.credential as ProvisionedCredential,
			});
		} catch (err) {
			setError(formatError(err));
		} finally {
			setPending(false);
		}
	};

	// While a draft is waiting, poll for the relay connecting and answering.
	useEffect(() => {
		if (!started || registered) return;
		const clusterId = started.clusterId;
		const timer = setInterval(async () => {
			try {
				const res = (await callApi("kubernetes_clusters", "confirm", {
					id: clusterId,
				})) as ClusterView;
				if (res.registered) setRegistered(true);
			} catch {
				/* keep polling; a transient error is not fatal to the wizard */
			}
		}, POLL_MS);
		return () => clearInterval(timer);
	}, [started, registered]);

	return (
		<Dialog open={open} onClose={close} fullWidth maxWidth="sm">
			<DialogTitle>Register cluster</DialogTitle>
			<DialogContent>
				{!started ? (
					<Box component="form" onSubmit={onRegister}>
						<Stack spacing={2} sx={{ mt: 1 }}>
							<Typography variant="body2" color="text.secondary">
								Name the cluster and Canopy mints its relay's credential.
								Install the credential into the cluster; Canopy saves the
								cluster once the relay connects and answers.
							</Typography>
							<TextField
								autoFocus
								label="Cluster name"
								size="small"
								value={name}
								onChange={(e) => setName(e.target.value)}
								placeholder="e.g. Nauru production"
								disabled={pending}
							/>
							{error && <Alert severity="error">{error}</Alert>}
						</Stack>
					</Box>
				) : (
					<Stack spacing={2} sx={{ mt: 1 }}>
						<CredentialReveal credential={started.credential} />
						{registered ? (
							<Alert severity="success">
								The relay connected and answered. The cluster is registered.
							</Alert>
						) : (
							<Stack
								direction="row"
								spacing={1}
								sx={{ alignItems: "center" }}
							>
								<CircularProgress size={18} />
								<Typography variant="body2" color="text.secondary">
									Waiting for the relay to connect and answer…
								</Typography>
							</Stack>
						)}
					</Stack>
				)}
			</DialogContent>
			<DialogActions>
				{!started ? (
					<>
						<Button onClick={close} disabled={pending}>
							Cancel
						</Button>
						<Button variant="contained" onClick={onRegister} disabled={pending}>
							{pending ? "Registering…" : "Register"}
						</Button>
					</>
				) : (
					<Button onClick={close}>{registered ? "Done" : "Close"}</Button>
				)}
			</DialogActions>
		</Dialog>
	);
}

/// Present a minted relay credential once: the passphrase to copy and the
/// encrypted key file to download. Canopy never keeps the private key.
function CredentialReveal({
	credential,
}: {
	credential: ProvisionedCredential;
}) {
	const download = () => {
		const binary = atob(credential.key_age_base64);
		const bytes = new Uint8Array(binary.length);
		for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
		const url = URL.createObjectURL(
			new Blob([bytes], { type: "application/octet-stream" }),
		);
		const a = document.createElement("a");
		a.href = url;
		a.download = credential.filename;
		document.body.appendChild(a);
		a.click();
		a.remove();
		URL.revokeObjectURL(url);
	};

	return (
		<Stack spacing={2}>
			<Alert severity="warning">
				This key is shown once. Download it and copy the passphrase now —
				Canopy does not keep the private key and cannot show it again.
			</Alert>
			<Box>
				<Typography variant="caption" color="text.secondary">
					Passphrase (share out of band)
				</Typography>
				<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
					<Typography variant="body1" sx={{ fontFamily: "monospace" }}>
						{credential.passphrase}
					</Typography>
					<Tooltip title="Copy passphrase">
						<IconButton
							size="small"
							onClick={() =>
								navigator.clipboard?.writeText(credential.passphrase)
							}
						>
							<ContentCopyIcon fontSize="small" />
						</IconButton>
					</Tooltip>
				</Stack>
			</Box>
			<Button variant="contained" startIcon={<DownloadIcon />} onClick={download}>
				Download key file
			</Button>
			<Typography variant="body2" color="text.secondary">
				Decrypt on the host with{" "}
				<code>bestool crypto reveal {credential.filename}</code> to recover the
				PEM.
			</Typography>
		</Stack>
	);
}

function formatDate(iso: string): string {
	return new Date(iso).toLocaleString(undefined, {
		year: "numeric",
		month: "short",
		day: "numeric",
		hour: "2-digit",
		minute: "2-digit",
	});
}

function formatError(err: unknown): string {
	if (err instanceof ApiError) {
		const detail = err.detail as { title?: string } | null;
		if (detail?.title) return detail.title;
		return err.message;
	}
	if (err instanceof Error) return err.message;
	return String(err);
}
