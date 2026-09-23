import {
	Alert,
	AlertTitle,
	Box,
	Button,
	Chip,
	Dialog,
	DialogActions,
	DialogContent,
	DialogContentText,
	DialogTitle,
	LinearProgress,
	MenuItem,
	Paper,
	Stack,
	TextField,
	Tooltip,
	Typography,
} from "@mui/material";
import ErrorOutlineIcon from "@mui/icons-material/ErrorOutlineOutlined";
import PauseCircleIcon from "@mui/icons-material/PauseCircle";
import PlayCircleIcon from "@mui/icons-material/PlayCircle";
import WarningAmberIcon from "@mui/icons-material/WarningAmber";
import { useState } from "react";
import { useApi, useApiAction } from "../api";
import { GradedAction } from "./GradedAction";
import { useIsAdmin } from "../hooks/useIsAdmin";
import TimeAgo from "./TimeAgo";

/// The public names a server has registered and the certificates Canopy holds
/// for it, on the server detail page. Also where an operator sets the profile
/// its certificates are issued under, pauses and unpauses Canopy's work on its
/// behalf, and revokes a certificate.
///
/// A pause is shown first and loudly: it suppresses the alerting that would
/// otherwise chase a certificate running out, so it has to be the thing an
/// operator sees before reading anything below it.
// spec: CRT#presentation
export default function ServerCertificatesSection({
	serverId,
}: {
	serverId: string;
}) {
	const isAdmin = useIsAdmin() === true;
	const [tick, setTick] = useState(0);
	const reload = () => setTick((t) => t + 1);

	const detail = useApi(
		"certificates",
		"for_server",
		{ server_id: serverId },
		[serverId, tick],
	);
	const authority = useApi("certificates", "authority", {}, []);

	if (detail.status === "loading" || detail.status === "idle") {
		return (
			<Paper variant="outlined" sx={{ p: 2 }}>
				<SectionHeading />
				<LinearProgress />
			</Paper>
		);
	}
	if (detail.status === "error") {
		return (
			<Paper variant="outlined" sx={{ p: 2 }}>
				<SectionHeading />
				<Alert severity="error">{detail.error.message}</Alert>
			</Paper>
		);
	}

	const data = detail.data;
	const anyGrant = data.may_manage_dns || data.may_manage_tls;

	// Neither grant, nothing registered, nothing held: this server does not use
	// the feature, so keep the page short rather than showing an empty box on
	// every server in the fleet.
	if (
		!anyGrant &&
		data.names.length === 0 &&
		data.certificates.length === 0 &&
		!data.paused
	)
		return null;

	const profiles = authority.status === "ok" ? authority.data.profiles : [];

	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<SectionHeading />
			<Stack spacing={2}>
				{data.paused && (
					<PauseBanner
						serverId={serverId}
						pausedAt={data.paused_at}
						pausedBy={data.paused_by}
						reason={data.pause_reason}
						isAdmin={isAdmin}
						onChanged={reload}
					/>
				)}

				<Stack
					direction="row"
					spacing={1}
					sx={{ alignItems: "center", flexWrap: "wrap", rowGap: 1 }}
				>
					<GrantChip label="DNS records" granted={data.may_manage_dns} />
					<GrantChip label="TLS certificates" granted={data.may_manage_tls} />
					{data.domains.length > 0 ? (
						<Typography variant="caption" color="text.secondary">
							within {data.domains.join(", ")}
						</Typography>
					) : (
						<Typography variant="caption" color="text.secondary">
							its group controls no domain, so it is entitled to no name
						</Typography>
					)}
					<Box sx={{ flex: 1 }} />
					{!data.paused && isAdmin && (
						<PauseButton serverId={serverId} onChanged={reload} />
					)}
				</Stack>

				{isAdmin && data.may_manage_tls && (
					<ProfilePicker
						serverId={serverId}
						current={data.certificate_profile}
						profiles={profiles}
						authorityKnown={authority.status === "ok"}
						onChanged={reload}
					/>
				)}

				{authority.status === "ok" && authority.data.problem && (
					<Alert severity="warning">
						<AlertTitle>Canopy cannot issue certificates right now</AlertTitle>
						{authority.data.problem}
					</Alert>
				)}

				<NamesTable names={data.names} />
				<CertificatesTable
					certificates={data.certificates}
					isAdmin={isAdmin}
					onChanged={reload}
				/>
			</Stack>
		</Paper>
	);
}

function SectionHeading() {
	return (
		<Typography variant="h6" component="h2" gutterBottom>
			Names and certificates
			<Typography
				component="span"
				variant="body2"
				color="text.secondary"
				sx={{ ml: 1 }}
			>
				— the public names this server has registered, and the TLS certificates
				Canopy holds for them.
			</Typography>
		</Typography>
	);
}

function GrantChip({ label, granted }: { label: string; granted: boolean }) {
	return (
		<Chip
			size="small"
			variant="outlined"
			color={granted ? "success" : "default"}
			label={granted ? `may manage ${label}` : `may not manage ${label}`}
		/>
	);
}

function PauseBanner({
	serverId,
	pausedAt,
	pausedBy,
	reason,
	isAdmin,
	onChanged,
}: {
	serverId: string;
	pausedAt: string | null;
	pausedBy: string | null;
	reason: string | null;
	isAdmin: boolean;
	onChanged: () => void;
}) {
	const resume = useApiAction("certificates", "resume");
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
					<GradedAction calls="certificates/resume">
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
					{pausedAt && (
						<>
							since <TimeAgo timestamp={pausedAt} />
						</>
					)}
					{pausedBy && ` by ${pausedBy}`}
					{reason && ` — ${reason}`}
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

function PauseButton({
	serverId,
	onChanged,
}: {
	serverId: string;
	onChanged: () => void;
}) {
	const [open, setOpen] = useState(false);
	const [reason, setReason] = useState("");
	const pause = useApiAction("certificates", "pause");

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
			<GradedAction calls="certificates/pause">
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
					<GradedAction calls="certificates/pause">
						<Button
							variant="contained"
							color="warning"
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

/// The authority's default is its longest-lived, which is what every server
/// takes until an operator chooses otherwise — so a short lifetime is adopted
/// deliberately per server rather than inherited.
// spec: CRT#lifetime
/// A non-empty sentinel: an empty select value reads as "nothing chosen" to MUI
/// and renders blank, which would hide the fact that a default is in force.
const AUTHORITY_DEFAULT = "\u0000default";

function ProfilePicker({
	serverId,
	current,
	profiles,
	authorityKnown,
	onChanged,
}: {
	serverId: string;
	current: string | null;
	profiles: string[];
	authorityKnown: boolean;
	onChanged: () => void;
}) {
	const setProfile = useApiAction("certificates", "set_profile");

	const onSelect = async (value: string) => {
		try {
			await setProfile.call({
				server_id: serverId,
				profile: value === AUTHORITY_DEFAULT ? null : value,
			});
			onChanged();
		} catch {
			/* surfaced via setProfile.error */
		}
	};

	// A profile the authority no longer advertises still needs to appear, or the
	// picker would silently show the wrong current value.
	const options = [...profiles];
	if (current && !options.includes(current)) options.push(current);

	return (
		<Box>
			<Stack direction="row" spacing={1} sx={{ alignItems: "flex-start" }}>
				<GradedAction calls="certificates/set_profile">
					<TextField
						select
						size="small"
						label="Certificate lifetime"
						value={current ?? AUTHORITY_DEFAULT}
						onChange={(e) => onSelect(e.target.value)}
						disabled={setProfile.pending || !authorityKnown}
						sx={{ minWidth: 260 }}
						helperText={
							authorityKnown && profiles.length === 0
								? "The authority advertises no profiles, so it decides the lifetime."
								: "Takes effect at the next issuance or renewal. A certificate already held keeps its own lifetime."
						}
					>
						<MenuItem value={AUTHORITY_DEFAULT}>
							Authority default (longest-lived)
						</MenuItem>
						{options.map((profile) => (
							<MenuItem key={profile} value={profile}>
								{profile}
								{!profiles.includes(profile) && " (no longer offered)"}
							</MenuItem>
						))}
					</TextField>
				</GradedAction>
			</Stack>
			{setProfile.error && (
				<Alert severity="error" sx={{ mt: 1 }}>
					{setProfile.error.message}
				</Alert>
			)}
		</Box>
	);
}

type NameRow = {
	id: string;
	name: string;
	addresses: string[];
	published_addresses: string[];
	published: boolean;
	published_at: string | null;
	last_error: string | null;
	zone: string | null;
};

function NamesTable({ names }: { names: NameRow[] }) {
	if (names.length === 0) {
		return (
			<Alert severity="info">
				This server has registered no public names.
			</Alert>
		);
	}

	return (
		<Box>
			<Typography variant="subtitle2" gutterBottom>
				Registered names
			</Typography>
			<Stack spacing={1}>
				{names.map((row) => (
					<Box key={row.id}>
						<Stack
							direction="row"
							spacing={1}
							sx={{ alignItems: "center", flexWrap: "wrap", rowGap: 0.5 }}
						>
							<Typography variant="body2" sx={{ fontFamily: "monospace" }}>
								{row.name}
							</Typography>
							{row.published ? (
								<Chip
									size="small"
									variant="outlined"
									color="success"
									label="published"
								/>
							) : (
								<Tooltip title="Canopy has not yet written what this server asked for into the zone. It retries every pass.">
									<Chip
										size="small"
										variant="outlined"
										color="warning"
										label="waiting to publish"
									/>
								</Tooltip>
							)}
							{!row.zone && (
								<Tooltip title="No configured DNS zone covers this name, so Canopy can publish nothing for it.">
									<Chip
										size="small"
										variant="outlined"
										color="error"
										icon={<WarningAmberIcon />}
										label="no matching zone"
									/>
								</Tooltip>
							)}
							<Box sx={{ flex: 1 }} />
							<Typography variant="caption" color="text.secondary">
								{row.published_at ? (
									<>
										published <TimeAgo timestamp={row.published_at} />
									</>
								) : (
									"never published"
								)}
							</Typography>
						</Stack>
						<Typography
							variant="caption"
							color="text.secondary"
							sx={{ fontFamily: "monospace" }}
						>
							{row.addresses.length > 0 ? row.addresses.join(", ") : "withdrawn"}
							{!row.published &&
								row.published_addresses.length > 0 &&
								` (currently ${row.published_addresses.join(", ")})`}
						</Typography>
						{row.last_error && (
							<Alert severity="error" sx={{ mt: 0.5 }} icon={<ErrorOutlineIcon />}>
								{row.last_error}
							</Alert>
						)}
					</Box>
				))}
			</Stack>
		</Box>
	);
}

type CertificateRow = {
	id: string;
	name: string;
	state: string;
	profile: string | null;
	not_after: string | null;
	remaining_seconds: number | null;
	issued_at: string | null;
	renewing: boolean;
	collectable: boolean;
	risk: string;
	attempts: number;
	last_error: string | null;
	revoked_at: string | null;
	revoked_by: string | null;
	revocation_reason: string | null;
	key_fingerprint: string;
};

/// A duration in the units an operator thinks in. Rendered from the seconds the
/// API gives rather than recomputed from the expiry, so the relative and
/// absolute readings can never disagree.
function humaniseRemaining(seconds: number): string {
	const past = seconds < 0;
	const total = Math.abs(seconds);
	const days = Math.floor(total / 86400);
	const hours = Math.floor((total % 86400) / 3600);
	const spelled =
		days > 0
			? `${days} day${days === 1 ? "" : "s"}`
			: hours > 0
				? `${hours} hour${hours === 1 ? "" : "s"}`
				: `${Math.floor(total / 60)} minutes`;
	return past ? `expired ${spelled} ago` : `${spelled} left`;
}

function CertificatesTable({
	certificates,
	isAdmin,
	onChanged,
}: {
	certificates: CertificateRow[];
	isAdmin: boolean;
	onChanged: () => void;
}) {
	if (certificates.length === 0) {
		return (
			<Alert severity="info">
				Canopy holds no certificates for this server.
			</Alert>
		);
	}

	return (
		<Box>
			<Typography variant="subtitle2" gutterBottom>
				Certificates
			</Typography>
			<Stack spacing={1}>
				{certificates.map((cert) => (
					<CertificateRowView
						key={cert.id}
						cert={cert}
						isAdmin={isAdmin}
						onChanged={onChanged}
					/>
				))}
			</Stack>
		</Box>
	);
}

function CertificateRowView({
	cert,
	isAdmin,
	onChanged,
}: {
	cert: CertificateRow;
	isAdmin: boolean;
	onChanged: () => void;
}) {
	return (
		<Box>
			<Stack
				direction="row"
				spacing={1}
				sx={{ alignItems: "center", flexWrap: "wrap", rowGap: 0.5 }}
			>
				<Typography variant="body2" sx={{ fontFamily: "monospace" }}>
					{cert.name}
				</Typography>
				<StateChip cert={cert} />
				{cert.profile && (
					<Chip size="small" variant="outlined" label={cert.profile} />
				)}
				{cert.renewing && cert.state === "pending" && (
					<Tooltip title="A renewal is in flight. The certificate already held stays valid and collectable until it arrives.">
						<Chip size="small" variant="outlined" label="renewing" />
					</Tooltip>
				)}
				<Box sx={{ flex: 1 }} />
				{cert.not_after && (
					<Tooltip title={cert.not_after}>
						<Typography variant="caption" color="text.secondary">
							expires <TimeAgo timestamp={cert.not_after} />
							{cert.remaining_seconds !== null &&
								` — ${humaniseRemaining(cert.remaining_seconds)}`}
						</Typography>
					</Tooltip>
				)}
				{isAdmin && cert.collectable && (
					<RevokeButton
						id={cert.id}
						name={cert.name}
						onChanged={onChanged}
					/>
				)}
			</Stack>
			{cert.revoked_at && (
				<Typography variant="caption" color="text.secondary">
					revoked <TimeAgo timestamp={cert.revoked_at} />
					{cert.revoked_by && ` by ${cert.revoked_by}`}
					{cert.revocation_reason &&
						` — ${cert.revocation_reason.replace(/_/g, " ")}`}
				</Typography>
			)}
			{cert.last_error && (
				<Alert severity="error" sx={{ mt: 0.5 }} icon={<ErrorOutlineIcon />}>
					{cert.attempts > 0 && `after ${cert.attempts} attempt(s): `}
					{cert.last_error}
				</Alert>
			)}
		</Box>
	);
}

function StateChip({ cert }: { cert: CertificateRow }) {
	if (cert.state === "revoked")
		return <Chip size="small" color="error" label="revoked" />;
	if (cert.state === "failed")
		return <Chip size="small" color="error" variant="outlined" label="failed" />;
	if (cert.state === "pending" && !cert.collectable)
		return <Chip size="small" variant="outlined" label="pending" />;
	if (cert.risk === "critical")
		return <Chip size="small" color="error" label="expiring" />;
	if (cert.risk === "at_risk")
		return <Chip size="small" color="warning" label="due for renewal" />;
	return (
		<Chip size="small" color="success" variant="outlined" label="valid" />
	);
}

const REVOCATION_REASONS: Array<{ value: string; label: string; note: string }> =
	[
		{
			value: "unspecified",
			label: "Unspecified",
			note: "No reason given. The key stays usable.",
		},
		{
			value: "key_compromise",
			label: "Key compromise",
			note: "The private key is known to be exposed. This key will never be certified again, for any name by any server — the server has to generate a new one.",
		},
		{
			value: "superseded",
			label: "Superseded",
			note: "Replaced by another certificate. The key stays usable.",
		},
		{
			value: "cessation_of_operation",
			label: "No longer in service",
			note: "The name is retired. The key stays usable.",
		},
	];

function RevokeButton({
	id,
	name,
	onChanged,
}: {
	id: string;
	name: string;
	onChanged: () => void;
}) {
	const [open, setOpen] = useState(false);
	const [reason, setReason] = useState("unspecified");
	const revoke = useApiAction("certificates", "revoke");

	const onConfirm = async () => {
		try {
			await revoke.call({ id, reason: reason as never });
			setOpen(false);
			onChanged();
		} catch {
			/* surfaced via revoke.error */
		}
	};

	const chosen = REVOCATION_REASONS.find((r) => r.value === reason);

	return (
		<>
			<GradedAction calls="certificates/revoke">
				<Button size="small" color="error" onClick={() => setOpen(true)}>
					Revoke
				</Button>
			</GradedAction>
			<Dialog open={open} onClose={() => setOpen(false)} fullWidth maxWidth="sm">
				<DialogTitle>Revoke the certificate for {name}?</DialogTitle>
				<DialogContent>
					<DialogContentText sx={{ mb: 2 }}>
						This cannot be undone: a revoked certificate stays revoked, and the
						remedy is a new one. Clients will reject whatever this server is
						serving on that name until it obtains a replacement.
						<br />
						<br />
						Revoking also <strong>pauses this server</strong>, so a replacement
						is not requested behind your back while you look into what happened.
						You decide when to resume it.
					</DialogContentText>
					<TextField
						select
						fullWidth
						size="small"
						label="Reason"
						value={reason}
						onChange={(e) => setReason(e.target.value)}
						disabled={revoke.pending}
						helperText={chosen?.note}
					>
						{REVOCATION_REASONS.map((r) => (
							<MenuItem key={r.value} value={r.value}>
								{r.label}
							</MenuItem>
						))}
					</TextField>
					{revoke.error && (
						<Alert severity="error" sx={{ mt: 2 }}>
							{revoke.error.message}
						</Alert>
					)}
				</DialogContent>
				<DialogActions>
					<Button onClick={() => setOpen(false)}>Cancel</Button>
					<GradedAction calls="certificates/revoke">
						<Button
							variant="contained"
							color="error"
							onClick={onConfirm}
							disabled={revoke.pending}
						>
							Revoke
						</Button>
					</GradedAction>
				</DialogActions>
			</Dialog>
		</>
	);
}
