import {
	Alert,
	Box,
	Button,
	Chip,
	Dialog,
	DialogActions,
	DialogContent,
	DialogContentText,
	DialogTitle,
	IconButton,
	LinearProgress,
	Paper,
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
import BlockIcon from "@mui/icons-material/Block";
import ContentCopyIcon from "@mui/icons-material/ContentCopy";
import { type FormEvent, useState } from "react";
import { ApiError, callApi, useApi } from "../api";
import { GradedAction } from "../components/GradedAction";
import { usePageTitle } from "../hooks/usePageTitle";

const DAY_MS = 24 * 60 * 60 * 1000;
const EXPIRY_WARNING_DAYS = 15;

type TokenRow = {
	id: string;
	name: string;
	created_by: string;
	created_at: string;
	expires_at: string;
	revoked_at?: string | null;
	last_used_at?: string | null;
};

export default function McpTokens() {
	usePageTitle("MCP access");
	const list = useApi("mcp_tokens", "list");
	const [name, setName] = useState("");
	const [pending, setPending] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const [toast, setToast] = useState<string | null>(null);
	const [minted, setMinted] = useState<{ name: string; secret: string } | null>(
		null,
	);
	const [confirmRevoke, setConfirmRevoke] = useState<TokenRow | null>(null);

	const onMint = async (e: FormEvent) => {
		e.preventDefault();
		const trimmed = name.trim();
		if (!trimmed) return setError("Token name cannot be empty");
		setPending(true);
		setError(null);
		try {
			const res = await callApi("mcp_tokens", "mint", { name: trimmed });
			setName("");
			setMinted({ name: res.token.name, secret: res.secret });
			list.reload();
		} catch (err) {
			setError(formatError(err));
		} finally {
			setPending(false);
		}
	};

	const onRevoke = async (token: TokenRow) => {
		setConfirmRevoke(null);
		try {
			await callApi("mcp_tokens", "revoke", { id: token.id });
			setToast(`Token "${token.name}" revoked`);
			list.reload();
		} catch (err) {
			setError(formatError(err));
		}
	};

	return (
		<Stack spacing={3}>
			<Typography variant="body2" color="text.secondary">
				Bearer tokens for the public MCP endpoint (<code>/mcp</code> on the
				device API host), for AI agents run outside the tailnet. Tokens
				expire one year after minting; a fleet-wide alert raises{" "}
				{EXPIRY_WARNING_DAYS} days before expiry.
			</Typography>

			<Paper variant="outlined" sx={{ p: 2 }}>
				<Box component="form" onSubmit={onMint}>
					<Stack direction="row" spacing={1} sx={{ alignItems: "flex-start" }}>
						<TextField
							size="small"
							fullWidth
							label="Token name"
							placeholder="e.g. claude-connections"
							value={name}
							onChange={(e) => setName(e.target.value)}
							disabled={pending}
						/>
						<GradedAction calls="mcp_tokens/mint">
							<Button
								type="submit"
								variant="contained"
								disabled={pending}
								sx={{ whiteSpace: "nowrap", flexShrink: 0 }}
							>
								{pending ? "Minting…" : "Mint token"}
							</Button>
						</GradedAction>
					</Stack>
					{error && (
						<Alert severity="error" sx={{ mt: 2 }}>
							{error}
						</Alert>
					)}
				</Box>
			</Paper>

			<Box>
				<Typography variant="h6" component="h2" gutterBottom>
					Tokens
				</Typography>
				{list.status === "loading" || list.status === "idle" ? (
					<LinearProgress />
				) : list.status === "error" ? (
					<Alert severity="error">{list.error.message}</Alert>
				) : list.data.length === 0 ? (
					<Alert severity="info">No tokens minted.</Alert>
				) : (
					<Table size="small">
						<TableHead>
							<TableRow>
								<TableCell>Name</TableCell>
								<TableCell>Minted by</TableCell>
								<TableCell>Expires</TableCell>
								<TableCell>Last used</TableCell>
								<TableCell>Status</TableCell>
								<TableCell align="right" />
							</TableRow>
						</TableHead>
						<TableBody>
							{list.data.map((token) => (
								<TableRow key={token.id} hover>
									<TableCell sx={{ fontFamily: "monospace" }}>
										{token.name}
									</TableCell>
									<TableCell>{token.created_by}</TableCell>
									<TableCell>{formatDate(token.expires_at)}</TableCell>
									<TableCell>
										{token.last_used_at
											? formatDate(token.last_used_at)
											: "never"}
									</TableCell>
									<TableCell>
										<StatusChip token={token} />
									</TableCell>
									<TableCell align="right">
										{!token.revoked_at && (
											<GradedAction calls="mcp_tokens/revoke">
												<Tooltip title="Revoke">
													<IconButton
														size="small"
														aria-label={`revoke ${token.name}`}
														onClick={() => setConfirmRevoke(token)}
													>
														<BlockIcon fontSize="small" />
													</IconButton>
												</Tooltip>
											</GradedAction>
										)}
									</TableCell>
								</TableRow>
							))}
						</TableBody>
					</Table>
				)}
			</Box>

			<Dialog open={!!minted} maxWidth="sm" fullWidth>
				<DialogTitle>Token "{minted?.name}" minted</DialogTitle>
				<DialogContent>
					<DialogContentText sx={{ mb: 2 }}>
						Copy the token now — it is shown once and cannot be retrieved
						again.
					</DialogContentText>
					<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
						<TextField
							fullWidth
							size="small"
							value={minted?.secret ?? ""}
							slotProps={{
								input: {
									readOnly: true,
									sx: { fontFamily: "monospace" },
								},
							}}
						/>
						<Tooltip title="Copy to clipboard">
							<IconButton
								aria-label="copy token"
								onClick={async () => {
									if (minted) {
										await navigator.clipboard.writeText(minted.secret);
										setToast("Token copied");
									}
								}}
							>
								<ContentCopyIcon />
							</IconButton>
						</Tooltip>
					</Stack>
				</DialogContent>
				<DialogActions>
					<Button onClick={() => setMinted(null)}>Done</Button>
				</DialogActions>
			</Dialog>

			<Dialog open={!!confirmRevoke} onClose={() => setConfirmRevoke(null)}>
				<DialogTitle>Revoke "{confirmRevoke?.name}"?</DialogTitle>
				<DialogContent>
					<DialogContentText>
						Anything using this token loses access immediately. This cannot
						be undone.
					</DialogContentText>
				</DialogContent>
				<DialogActions>
					<Button onClick={() => setConfirmRevoke(null)}>Cancel</Button>
					<GradedAction calls="mcp_tokens/revoke">
						<Button
							color="error"
							onClick={() => confirmRevoke && onRevoke(confirmRevoke)}
						>
							Revoke
						</Button>
					</GradedAction>
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

function StatusChip({ token }: { token: TokenRow }) {
	if (token.revoked_at) {
		return <Chip size="small" label="revoked" />;
	}
	const untilExpiry = new Date(token.expires_at).getTime() - Date.now();
	if (untilExpiry <= 0) {
		return <Chip size="small" color="error" label="expired" />;
	}
	if (untilExpiry <= EXPIRY_WARNING_DAYS * DAY_MS) {
		const days = Math.ceil(untilExpiry / DAY_MS);
		return (
			<Chip size="small" color="warning" label={`expires in ${days}d`} />
		);
	}
	return <Chip size="small" color="success" label="active" />;
}

function formatDate(iso: string): string {
	return new Date(iso).toLocaleDateString(undefined, {
		year: "numeric",
		month: "short",
		day: "numeric",
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
