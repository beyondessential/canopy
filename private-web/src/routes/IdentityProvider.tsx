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
import DeleteIcon from "@mui/icons-material/Delete";
import EditIcon from "@mui/icons-material/Edit";
import { type FormEvent, useState } from "react";
import { ApiError, callApi, useApi } from "../api";
import { GradedAction } from "../components/GradedAction";
import TimeAgo from "../components/TimeAgo";
import { usePageTitle } from "../hooks/usePageTitle";
import type { IdentityKeyView as KeyRow, RelyingPartyView as RelyingPartyRow } from "../types";

const DEFAULT_LIFETIME = 5;

/// A token lifetime as typed, when it is a whole number of minutes in range.
// spec: IDP#relying-parties
function wholeMinutes(value: string): number | null {
	const minutes = Number(value);
	return value.trim() !== "" && Number.isInteger(minutes) && minutes >= 1 && minutes <= 60
		? minutes
		: null;
}

/// Canopy as an OpenID Connect issuer: the issuer, its signing keys, and the
/// register of relying parties tokens are issued for.
// spec: IDP#presentation
export default function IdentityProvider() {
	usePageTitle("Identity provider");
	const status = useApi("identity", "status", {}, []);
	const parties = useApi("identity", "relying_parties", {}, []);

	return (
		<Stack spacing={3}>
			<Paper variant="outlined" sx={{ p: 2 }}>
				<Typography variant="h6" component="h2" gutterBottom>
					Issuer
				</Typography>
				{status.status === "loading" || status.status === "idle" ? (
					<LinearProgress />
				) : status.status === "error" ? (
					<Alert severity="error">{status.error.message}</Alert>
				) : !status.data.issues_tokens ? (
					<Alert severity="info">
						No issuer or signing key is configured, so this Canopy instance
						issues no identity tokens.
					</Alert>
				) : (
					<Stack spacing={2}>
						<Box>
							<Typography variant="subtitle2">Issuer</Typography>
							<Typography variant="body2" sx={{ fontFamily: "monospace" }}>
								{status.data.issuer}
							</Typography>
						</Box>
						<Box>
							<Typography variant="subtitle2">Discovery</Typography>
							<Typography
								variant="body2"
								sx={{ fontFamily: "monospace", wordBreak: "break-all" }}
							>
								{status.data.discovery_url}
							</Typography>
						</Box>
					</Stack>
				)}
			</Paper>

			{status.status === "ok" && status.data.keys.length > 0 && (
				<Paper variant="outlined" sx={{ p: 2 }}>
					<Typography variant="h6" component="h2" gutterBottom>
						Signing keys
					</Typography>
					<SigningKeys keys={status.data.keys} />
				</Paper>
			)}

			<Paper variant="outlined" sx={{ p: 2 }}>
				<Typography variant="h6" component="h2" gutterBottom>
					Relying parties
				</Typography>
				<AddRelyingParty onAdded={parties.reload} />
				{parties.status === "loading" || parties.status === "idle" ? (
					<LinearProgress sx={{ mt: 2 }} />
				) : parties.status === "error" ? (
					<Alert severity="error" sx={{ mt: 2 }}>
						{parties.error.message}
					</Alert>
				) : (
					parties.data.length > 0 && (
						<RelyingParties rows={parties.data} onChanged={parties.reload} />
					)
				)}
			</Paper>
		</Stack>
	);
}

function SigningKeys({ keys }: { keys: KeyRow[] }) {
	return (
		<Table size="small">
			<TableHead>
				<TableRow>
					<TableCell>Key</TableCell>
					<TableCell>Status</TableCell>
					<TableCell>Published</TableCell>
					<TableCell>Last signed</TableCell>
					<TableCell align="right">Tokens, 30 days</TableCell>
				</TableRow>
			</TableHead>
			<TableBody>
				{keys.map((key) => (
					<TableRow key={key.key_ref}>
						<TableCell>
							{key.kid && (
								<Tooltip title={key.kid}>
									<Typography variant="body2" sx={{ fontFamily: "monospace" }}>
										{shortKid(key.kid)}
									</Typography>
								</Tooltip>
							)}
							<Typography variant="caption" color="text.secondary">
								{key.key_ref}
							</Typography>
						</TableCell>
						<TableCell>
							<Chip
								size="small"
								color={key.state === "signing" ? "success" : "default"}
								label={key.state}
							/>
						</TableCell>
						<TableCell>
							{key.first_served_at ? formatDateTime(key.first_served_at) : "not yet"}
						</TableCell>
						<TableCell>
							{key.last_signed_at ? <TimeAgo timestamp={key.last_signed_at} /> : "never"}
						</TableCell>
						<TableCell align="right">{key.signed_30d.toLocaleString()}</TableCell>
					</TableRow>
				))}
			</TableBody>
		</Table>
	);
}

function AddRelyingParty({ onAdded }: { onAdded: () => void }) {
	const [name, setName] = useState("");
	const [audience, setAudience] = useState("");
	const [lifetime, setLifetime] = useState(String(DEFAULT_LIFETIME));
	const [pending, setPending] = useState(false);
	const [error, setError] = useState<string | null>(null);

	const onAdd = async (e: FormEvent) => {
		e.preventDefault();
		setPending(true);
		setError(null);
		try {
			await callApi("identity", "add_relying_party", {
				name: name.trim(),
				audience: audience.trim(),
				token_lifetime_minutes: wholeMinutes(lifetime) ?? undefined,
			});
			setName("");
			setAudience("");
			setLifetime(String(DEFAULT_LIFETIME));
			onAdded();
		} catch (err) {
			setError(formatError(err));
		} finally {
			setPending(false);
		}
	};

	return (
		<Box component="form" onSubmit={onAdd}>
			<Stack direction="row" spacing={1} sx={{ alignItems: "flex-start" }}>
				<TextField
					size="small"
					label="Name"
					placeholder="e.g. Anthropic"
					value={name}
					onChange={(e) => setName(e.target.value)}
					disabled={pending}
					sx={{ flex: 1 }}
				/>
				<TextField
					size="small"
					label="Audience"
					placeholder="e.g. https://api.anthropic.com"
					value={audience}
					onChange={(e) => setAudience(e.target.value)}
					disabled={pending}
					sx={{ flex: 2 }}
				/>
				<LifetimeField value={lifetime} onChange={setLifetime} disabled={pending} />
				<GradedAction
					calls="identity/add_relying_party"
					action={`Register relying party ${name.trim()}`}
				>
					<Button
						type="submit"
						variant="contained"
						disabled={
							pending || !name.trim() || !audience.trim() || wholeMinutes(lifetime) === null
						}
						sx={{ flexShrink: 0 }}
					>
						Add
					</Button>
				</GradedAction>
			</Stack>
			{error && (
				<Alert severity="error" sx={{ mt: 2 }}>
					{error}
				</Alert>
			)}
		</Box>
	);
}

function LifetimeField({
	value,
	onChange,
	disabled,
}: {
	value: string;
	onChange: (v: string) => void;
	disabled?: boolean;
}) {
	return (
		<TextField
			size="small"
			type="number"
			label="Token lifetime, minutes"
			value={value}
			onChange={(e) => onChange(e.target.value)}
			disabled={disabled}
			error={wholeMinutes(value) === null}
			helperText={wholeMinutes(value) === null ? "Whole minutes, 1 to 60" : undefined}
			slotProps={{ htmlInput: { min: 1, max: 60, step: 1 } }}
			sx={{ width: 180, flexShrink: 0 }}
		/>
	);
}

function RelyingParties({
	rows,
	onChanged,
}: {
	rows: RelyingPartyRow[];
	onChanged: () => void;
}) {
	const [editing, setEditing] = useState<RelyingPartyRow | null>(null);
	const [removing, setRemoving] = useState<RelyingPartyRow | null>(null);
	const [error, setError] = useState<string | null>(null);

	const onRemove = async (party: RelyingPartyRow) => {
		setRemoving(null);
		try {
			await callApi("identity", "remove_relying_party", { id: party.id });
			onChanged();
		} catch (err) {
			setError(formatError(err));
		}
	};

	return (
		<>
			{error && (
				<Alert severity="error" sx={{ mt: 2 }}>
					{error}
				</Alert>
			)}
			<Table size="small" sx={{ mt: 2 }}>
				<TableHead>
					<TableRow>
						<TableCell>Name</TableCell>
						<TableCell>Audience</TableCell>
						<TableCell>Token lifetime</TableCell>
						<TableCell>Granted to</TableCell>
						<TableCell>Last issued</TableCell>
						<TableCell align="right" />
					</TableRow>
				</TableHead>
				<TableBody>
					{rows.map((party) => (
						<TableRow key={party.id} hover>
							<TableCell>{party.name}</TableCell>
							<TableCell sx={{ fontFamily: "monospace" }}>{party.audience}</TableCell>
							<TableCell>{minutes(party.token_lifetime_minutes)}</TableCell>
							<TableCell>{grantedTo(party)}</TableCell>
							<TableCell>
								{party.last_issued_at ? (
									<TimeAgo timestamp={party.last_issued_at} />
								) : (
									"none in 90 days"
								)}
							</TableCell>
							<TableCell align="right" sx={{ whiteSpace: "nowrap" }}>
								<GradedAction
									calls="identity/edit_relying_party"
									action={`Edit relying party ${party.name}`}
								>
									<Tooltip title="Edit">
										<IconButton
											size="small"
											aria-label={`edit ${party.name}`}
											onClick={() => setEditing(party)}
										>
											<EditIcon fontSize="small" />
										</IconButton>
									</Tooltip>
								</GradedAction>
								<GradedAction
									calls="identity/remove_relying_party"
									action={`Remove relying party ${party.name}`}
								>
									<Tooltip title="Remove">
										<IconButton
											size="small"
											aria-label={`remove ${party.name}`}
											onClick={() => setRemoving(party)}
										>
											<DeleteIcon fontSize="small" />
										</IconButton>
									</Tooltip>
								</GradedAction>
							</TableCell>
						</TableRow>
					))}
				</TableBody>
			</Table>

			{editing && (
				<EditRelyingParty
					party={editing}
					onClose={() => setEditing(null)}
					onSaved={() => {
						setEditing(null);
						onChanged();
					}}
				/>
			)}

			<Dialog open={!!removing} onClose={() => setRemoving(null)}>
				<DialogTitle>Remove {removing?.name}?</DialogTitle>
				<DialogContent>
					<DialogContentText>
						{removing && removalConsequence(removing)}
					</DialogContentText>
				</DialogContent>
				<DialogActions>
					<Button onClick={() => setRemoving(null)}>Cancel</Button>
					<GradedAction
						calls="identity/remove_relying_party"
						action={`Remove relying party ${removing?.name}`}
					>
						<Button color="error" onClick={() => removing && onRemove(removing)}>
							Remove
						</Button>
					</GradedAction>
				</DialogActions>
			</Dialog>
		</>
	);
}

function EditRelyingParty({
	party,
	onClose,
	onSaved,
}: {
	party: RelyingPartyRow;
	onClose: () => void;
	onSaved: () => void;
}) {
	const [name, setName] = useState(party.name);
	const [lifetime, setLifetime] = useState(String(party.token_lifetime_minutes));
	const [pending, setPending] = useState(false);
	const [error, setError] = useState<string | null>(null);

	const onSave = async (e: FormEvent) => {
		e.preventDefault();
		setPending(true);
		setError(null);
		try {
			await callApi("identity", "edit_relying_party", {
				id: party.id,
				name: name.trim(),
				token_lifetime_minutes: wholeMinutes(lifetime)!,
			});
			onSaved();
		} catch (err) {
			setError(formatError(err));
		} finally {
			setPending(false);
		}
	};

	return (
		<Dialog open onClose={onClose} maxWidth="sm" fullWidth>
			<Box component="form" onSubmit={onSave}>
				<DialogTitle>Edit {party.name}</DialogTitle>
				<DialogContent>
					<Stack spacing={2} sx={{ pt: 1 }}>
						<TextField
							size="small"
							label="Name"
							value={name}
							onChange={(e) => setName(e.target.value)}
							disabled={pending}
						/>
						<TextField
							size="small"
							label="Audience"
							value={party.audience}
							disabled
							slotProps={{ htmlInput: { sx: { fontFamily: "monospace" } } }}
						/>
						<LifetimeField value={lifetime} onChange={setLifetime} disabled={pending} />
						{error && <Alert severity="error">{error}</Alert>}
					</Stack>
				</DialogContent>
				<DialogActions>
					<Button onClick={onClose}>Cancel</Button>
					<GradedAction
						calls="identity/edit_relying_party"
						action={`Edit relying party ${party.name}`}
					>
						<Button
							type="submit"
							disabled={pending || !name.trim() || wholeMinutes(lifetime) === null}
						>
							Save
						</Button>
					</GradedAction>
				</DialogActions>
			</Box>
		</Dialog>
	);
}

function removalConsequence(party: RelyingPartyRow): string {
	const grants = party.machine_grants + party.application_grants;
	const revokes =
		grants === 0
			? "No machine or application holds a grant for it."
			: `This revokes ${grants} ${grants === 1 ? "grant" : "grants"}.`;
	return `${revokes} Requests for its audience are then refused. This cannot be undone.`;
}

function grantedTo(party: RelyingPartyRow): string {
	const parts = [];
	if (party.machine_grants > 0) {
		parts.push(plural(party.machine_grants, "machine", "machines"));
	}
	if (party.application_grants > 0) {
		parts.push(plural(party.application_grants, "application", "applications"));
	}
	return parts.length === 0 ? "none" : parts.join(", ");
}

function plural(n: number, one: string, many: string): string {
	return `${n.toLocaleString()} ${n === 1 ? one : many}`;
}

function minutes(n: number): string {
	return plural(n, "minute", "minutes");
}

/** A key identifier is a thumbprint: its ends tell keys apart at a glance. */
function shortKid(kid: string): string {
	return kid.length > 12 ? `${kid.slice(0, 4)}…${kid.slice(-4)}` : kid;
}

function formatDateTime(iso: string): string {
	return new Intl.DateTimeFormat(undefined, {
		dateStyle: "medium",
		timeStyle: "short",
	}).format(new Date(iso));
}

function formatError(err: unknown): string {
	if (err instanceof ApiError) {
		const detail = err.detail as { title?: string; detail?: string } | null;
		if (detail?.detail) return detail.detail;
		if (detail?.title) return detail.title;
		return err.message;
	}
	if (err instanceof Error) return err.message;
	return String(err);
}
