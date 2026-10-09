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
import WarningAmberIcon from "@mui/icons-material/WarningAmber";
import { useState } from "react";
import { useApi, useApiAction } from "../api";
import { KIND_HEADINGS } from "../dnsNames";
import { useIsAdmin } from "../hooks/useIsAdmin";
import type { CertificateNameView, CertificateView } from "../types";
import CertificateStateChip from "./CertificateStateChip";
import DeclareField from "./DeclareField";
import { GradedAction } from "./GradedAction";
import GrantChip from "./GrantChip";
import { PauseBanner, PauseButton } from "./PauseControls";
import TimeAgo from "./TimeAgo";
import TimeLeft from "./TimeLeft";

/// The DNS names an application declares for certificates and the certificates
/// Canopy holds for it, on the application's page. A separate section from the
/// application's DNS names: the two are separate features that share
/// infrastructure, and this one shows nothing about addresses. Also where an
/// operator sets the profile its certificates are issued under, pauses and
/// unpauses Canopy's work on its behalf, and revokes a certificate.
///
/// A pause is shown first and loudly: it suppresses the alerting that would
/// otherwise chase a certificate running out, so it has to be the thing an
/// operator sees before reading anything below it.
///
/// Absent while the application neither may obtain certificates, declares a DNS
/// name for them, nor holds one.
// spec: CRT#presentation
export default function ServerCertificatesSection({
	serverId,
	refreshKey,
	onChanged,
}: {
	serverId: string;
	/// Changes when the other of the application's two sections changed
	/// something they share, such as the pause.
	refreshKey: number;
	onChanged: () => void;
}) {
	const isAdmin = useIsAdmin() === true;
	const [tick, setTick] = useState(0);
	const reload = () => {
		setTick((t) => t + 1);
		onChanged();
	};

	const detail = useApi("certificates", "for_server", { server_id: serverId }, [
		serverId,
		tick,
		refreshKey,
	]);
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

	// No grant, no declaration, nothing held: this application does not use the
	// feature, so keep the page short rather than showing an empty box on every
	// application in the fleet. A pause is something to show, since this
	// section is where it is seen and lifted.
	if (
		!data.may_manage_tls &&
		data.names.length === 0 &&
		data.certificates.length === 0 &&
		!data.pause
	)
		return null;

	const profiles = authority.status === "ok" ? authority.data.profiles : [];

	return (
		<Paper
			variant="outlined"
			sx={{ p: 2 }}
			data-testid="application-names-certificate"
		>
			<SectionHeading />
			<Stack spacing={2}>
				{data.pause && (
					<PauseBanner
						serverId={serverId}
						pause={data.pause}
						isAdmin={isAdmin}
						onChanged={reload}
					/>
				)}

				<Stack
					direction="row"
					spacing={1}
					sx={{ alignItems: "center", flexWrap: "wrap", rowGap: 1 }}
				>
					<GrantChip label="TLS certificates" granted={data.may_manage_tls} />
					<Typography variant="caption" color="text.secondary">
						{data.domains.length > 0
							? `within ${data.domains.join(", ")}`
							: "its group controls no domain, so it is entitled to no name"}
					</Typography>
					<Box sx={{ flex: 1 }} />
					{!data.pause && isAdmin && (
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

				<NamesList
					serverId={serverId}
					names={data.names}
					isAdmin={isAdmin}
					onChanged={reload}
				/>
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
			{KIND_HEADINGS.certificate}
			<Typography
				component="span"
				variant="body2"
				color="text.secondary"
				sx={{ ml: 1 }}
			>
				— the DNS names this application obtains certificates for, and the
				certificates Canopy holds for them.
			</Typography>
		</Typography>
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
				<GradedAction
					calls="certificates/set_profile"
					action="Set certificate lifetime for this server"
				>
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

/// The DNS names an application declares for certificates. A declaration is
/// routing and ownership only: the application requests the certificate itself,
/// and one declared with none held shows none below.
// spec: DNS#on-an-application
function NamesList({
	serverId,
	names,
	isAdmin,
	onChanged,
}: {
	serverId: string;
	names: CertificateNameView[];
	isAdmin: boolean;
	onChanged: () => void;
}) {
	return (
		<Box>
			<Stack
				direction="row"
				spacing={1}
				sx={{ alignItems: "flex-start", mb: 1, flexWrap: "wrap", rowGap: 1 }}
			>
				<Typography variant="subtitle2" sx={{ pt: 1 }}>
					Declared DNS names
				</Typography>
				<Box sx={{ flex: 1 }} />
				{isAdmin && (
					<DeclareField
						kind="certificate"
						serverId={serverId}
						onChanged={onChanged}
					/>
				)}
			</Stack>
			{names.length === 0 ? (
				<Alert severity="info">
					This application declares no DNS names for certificates.
				</Alert>
			) : (
				<Stack spacing={1}>
					{names.map((row) => (
						<NameRowView
							key={row.id}
							serverId={serverId}
							row={row}
							isAdmin={isAdmin}
							onChanged={onChanged}
						/>
					))}
				</Stack>
			)}
		</Box>
	);
}

function NameRowView({
	serverId,
	row,
	isAdmin,
	onChanged,
}: {
	serverId: string;
	row: CertificateNameView;
	isAdmin: boolean;
	onChanged: () => void;
}) {
	const release = useApiAction("certificates", "release");

	const onRelease = async () => {
		if (
			!confirm(
				`Release ${row.name} for certificates? Canopy stops renewing its certificates. Certificates already held stay.`,
			)
		)
			return;
		try {
			await release.call({ application_id: serverId, name: row.name });
			onChanged();
		} catch {
			/* surfaced via release.error */
		}
	};

	return (
		<Box data-testid="declared-name-row">
			<Stack
				direction="row"
				spacing={1}
				sx={{ alignItems: "center", flexWrap: "wrap", rowGap: 0.5 }}
			>
				<Typography variant="body2" sx={{ fontFamily: "monospace" }}>
					{row.name}
				</Typography>
				{!row.within_domains && (
					<Tooltip title="Nothing can be certified for it until the group controls a domain covering it.">
						<Chip
							size="small"
							variant="outlined"
							color="warning"
							icon={<WarningAmberIcon />}
							label="outside the group's domains"
						/>
					</Tooltip>
				)}
				<Box sx={{ flex: 1 }} />
				{isAdmin && (
					<GradedAction
						calls="certificates/release"
						action={`Release DNS name ${row.name} for certificates`}
					>
						<Button
							size="small"
							color="error"
							onClick={onRelease}
							disabled={release.pending}
						>
							Release
						</Button>
					</GradedAction>
				)}
			</Stack>
			{release.error && (
				<Alert severity="error" sx={{ mt: 0.5 }}>
					{release.error.message}
				</Alert>
			)}
		</Box>
	);
}

function CertificatesTable({
	certificates,
	isAdmin,
	onChanged,
}: {
	certificates: CertificateView[];
	isAdmin: boolean;
	onChanged: () => void;
}) {
	if (certificates.length === 0) {
		return (
			<Alert severity="info">
				Canopy holds no certificates for this application.
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
	cert: CertificateView;
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
				<CertificateStateChip cert={cert} />
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
					<TimeLeft notAfter={cert.not_after} risk={cert.risk} />
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
			<GradedAction
				calls="certificates/revoke"
				action={`Revoke certificate for ${name}`}
			>
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
					<GradedAction
						calls="certificates/revoke"
						action={`Revoke certificate for ${name}`}
					>
						<Button
							variant="contained"
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
