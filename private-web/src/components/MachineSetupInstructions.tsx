import {
	Alert,
	Box,
	Button,
	Chip,
	CircularProgress,
	IconButton,
	Paper,
	Stack,
	Tooltip,
	Typography,
} from "@mui/material";
import CheckCircleIcon from "@mui/icons-material/CheckCircle";
import ContentCopyIcon from "@mui/icons-material/ContentCopy";
import RefreshIcon from "@mui/icons-material/Refresh";
import { useEffect, useRef, useState } from "react";
import { useApi, useApiAction } from "../api";
import { GradedAction, useGrade } from "./GradedAction";
import { useReloadInterval } from "../hooks/useReloadInterval";
import TimeAgo from "./TimeAgo";
import type { EnrollmentTicket } from "../types";

/// Setup / enrollment instructions for a machine. Mints an encrypted enrollment
/// ticket plus its 4-word passphrase and shows the `bestool canopy register`
/// command for the operator to run (with the passphrase shared out-of-band),
/// plus a live "waiting for check-in" → "registered" indicator.
///
/// Enrolment admits the box, so it is keyed on the machine. What runs on the
/// box comes into being from the enrolled agent's first report, and so has
/// nothing to do here.
///
/// The component is driven by `machines.enrollment_status`, not local state: if
/// a ticket is already outstanding (e.g. after a page reload, or a re-enroll
/// someone else started) it shows that *pending* state rather than minting a
/// fresh ticket (which would invalidate the outstanding one). The ticket and
/// passphrase are only displayable in the session that minted them — after a
/// reload we can show that one is outstanding, but not its secret; the operator
/// reissues (deliberately) or cancels.
export default function MachineSetupInstructions({
	machineId,
	onRegistered,
	reEnroll = false,
}: {
	machineId: string;
	/// Fired once when enrollment completes. For initial setup that's the first
	/// `registered_at`; for re-enroll it's `registered_at` *changing* from its
	/// value at mount (a new device completing the handshake).
	onRegistered?: () => void;
	/// Re-enrollment of an already-registered machine. Initial setup auto-mints a
	/// ticket; re-enroll waits for the operator to press "Re-enroll a device".
	reEnroll?: boolean;
}) {
	const mint = useApiAction("fleet/machines", "mint_enrollment");
	const revoke = useApiAction("fleet/machines", "revoke_enrollment");
	const [ticket, setTicket] = useState<EnrollmentTicket | null>(null);
	const [copied, setCopied] = useState(false);
	const [copiedPassphrase, setCopiedPassphrase] = useState(false);

	// Poll enrollment status — the source of truth for whether a ticket is
	// outstanding and whether the machine has (re-)registered.
	const tick = useReloadInterval(5000);
	const status = useApi(
		"fleet/machines",
		"enrollment_status",
		{ machine_id: machineId },
		[machineId, tick],
	);
	const statusLoaded = status.status === "ok";
	const registeredAt = statusLoaded ? status.data.registered_at : null;
	const tokenExpiresAt = statusLoaded ? status.data.token_expires_at : null;
	const tokenIssuedAt = statusLoaded ? status.data.token_issued_at : null;
	const outstanding = tokenExpiresAt != null;
	const issuedOn = tokenIssuedAt
		? new Date(tokenIssuedAt).toLocaleString()
		: null;

	// In re-enroll mode the machine is already registered, so "done" means the
	// `registered_at` timestamp has *changed* since we opened. Capture the value
	// at first status load as the baseline.
	const baselineRegisteredAt = useRef<string | null | undefined>(undefined);
	useEffect(() => {
		if (statusLoaded && baselineRegisteredAt.current === undefined) {
			baselineRegisteredAt.current = registeredAt;
		}
	}, [statusLoaded, registeredAt]);
	const registeredView = reEnroll
		? registeredAt != null &&
			baselineRegisteredAt.current !== undefined &&
			registeredAt !== baselineRegisteredAt.current
		: registeredAt != null;

	const doMint = () => {
		setTicket(null);
		mint
			.call({ machine_id: machineId })
			.then(setTicket)
			.catch(() => {
				/* surfaced via mint.error */
			});
	};

	// Initial setup auto-mints once — but only when nothing is outstanding, so a
	// reload mid-enrollment shows the pending ticket instead of clobbering it.
	// Minting is danger, so a session below it waits for the operator to raise
	// rather than asking and being refused.
	const autoMinted = useRef(false);
	const minting = useGrade("fleet/machines/mint_enrollment");
	useEffect(() => {
		if (reEnroll || !statusLoaded || autoMinted.current || ticket) return;
		if (outstanding || minting.blocked) return;
		autoMinted.current = true;
		doMint();
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [reEnroll, statusLoaded, outstanding, ticket, minting.blocked]);

	// Notify the parent once when (re-)registration completes.
	const [notified, setNotified] = useState(false);
	useEffect(() => {
		if (registeredView && !notified) {
			setNotified(true);
			onRegistered?.();
		}
	}, [registeredView, notified, onRegistered]);

	const onCancel = async () => {
		try {
			await revoke.call({ machine_id: machineId });
			setTicket(null);
			status.reload();
		} catch {
			/* surfaced via revoke.error */
		}
	};

	// The ticket is encrypted, so the whole command is one safe copy-paste;
	// bestool only prompts for the passphrase.
	const command = ticket ? `bestool canopy register ${ticket.ticket}` : "";

	const onCopy = async () => {
		if (!ticket) return;
		try {
			await navigator.clipboard.writeText(command);
			setCopied(true);
			window.setTimeout(() => setCopied(false), 2000);
		} catch {
			/* clipboard may be unavailable; ignore */
		}
	};

	const onCopyPassphrase = async () => {
		if (!ticket) return;
		try {
			await navigator.clipboard.writeText(ticket.passphrase);
			setCopiedPassphrase(true);
			window.setTimeout(() => setCopiedPassphrase(false), 2000);
		} catch {
			/* clipboard may be unavailable; ignore */
		}
	};

	// Initial setup that cannot mint yet: the ticket it would have minted on its
	// own, offered as the control it needs a higher mode for.
	if (
		!reEnroll &&
		minting.blocked &&
		statusLoaded &&
		!outstanding &&
		!ticket &&
		!mint.pending
	) {
		return (
			<Box>
				<GradedAction calls="fleet/machines/mint_enrollment">
					<Button variant="outlined" onClick={doMint}>
						Issue enrollment ticket
					</Button>
				</GradedAction>
			</Box>
		);
	}

	// Idle re-enroll: nothing minted in this session and nothing outstanding —
	// just offer the button.
	if (reEnroll && statusLoaded && !outstanding && !ticket && !mint.pending) {
		return (
			<Box>
				<GradedAction calls="fleet/machines/mint_enrollment">
					<Button variant="outlined" onClick={doMint}>
						Re-enroll a device
					</Button>
				</GradedAction>
				<Typography
					variant="caption"
					color="text.secondary"
					sx={{ display: "block", mt: 1 }}
				>
					Issue a new enrollment ticket to bind this machine to a replacement
					device. The current device keeps working until the new one checks
					in.
				</Typography>
				{mint.error && (
					<Alert severity="error" sx={{ mt: 1 }}>
						{mint.error.message}
					</Alert>
				)}
			</Box>
		);
	}

	const reissueButton = (
		<GradedAction calls="fleet/machines/mint_enrollment">
			<Tooltip title="Generates a new ticket and passphrase; the current ones immediately stop working.">
				<Button
					size="small"
					startIcon={<RefreshIcon />}
					onClick={doMint}
					// A tooltip keeps its child's own props over the ones the
					// wrapper passes in, so the blocked state is named here too.
					disabled={mint.pending || minting.blocked}
				>
					{mint.pending ? "Reissuing…" : "Reissue"}
				</Button>
			</Tooltip>
		</GradedAction>
	);

	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<Stack spacing={2}>
				<Stack
					direction="row"
					spacing={1}
					sx={{ alignItems: "center", justifyContent: "space-between" }}
				>
					<Typography variant="h6" component="h2">
						{reEnroll ? "Re-enroll a device" : "Set up this machine"}
					</Typography>
					<RegistrationState
						registered={registeredView}
						tokenExpiresAt={tokenExpiresAt}
					/>
				</Stack>

				{ticket ? (
					<>
						<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
							<Typography
								variant="body2"
								color="text.secondary"
								sx={{ flex: 1 }}
							>
								Run this on the {reEnroll ? "replacement " : ""}machine; it
								will prompt for the passphrase shown below.
							</Typography>
							<Tooltip title={copied ? "Copied" : "Copy command"}>
								<IconButton
									size="small"
									onClick={onCopy}
									aria-label="Copy register command"
								>
									<ContentCopyIcon fontSize="small" />
								</IconButton>
							</Tooltip>
							{reissueButton}
						</Stack>

						<Box
							component="pre"
							sx={{
								m: 0,
								p: 1.5,
								borderRadius: 1,
								bgcolor: "action.hover",
								overflow: "auto",
								fontSize: "0.85em",
								fontFamily: "monospace",
								whiteSpace: "pre-wrap",
								wordBreak: "break-all",
							}}
						>
							{command}
						</Box>

						<Typography variant="caption" color="text.secondary">
							Copy the command and passphrase now — they won't be shown
							again.
						</Typography>

						<Box>
							<Typography variant="subtitle2" gutterBottom>
								Passphrase
							</Typography>
							<Stack
								direction="row"
								spacing={1}
								sx={{ alignItems: "center" }}
							>
								<Box
									component="code"
									sx={{
										flex: 1,
										p: 1.5,
										borderRadius: 1,
										bgcolor: "action.selected",
										fontFamily: "monospace",
										fontSize: "1.1em",
										fontWeight: 600,
										letterSpacing: "0.02em",
										userSelect: "all",
										wordBreak: "break-all",
									}}
								>
									{ticket.passphrase}
								</Box>
								<Tooltip
									title={copiedPassphrase ? "Copied" : "Copy passphrase"}
								>
									<IconButton
										size="small"
										onClick={onCopyPassphrase}
										aria-label="Copy passphrase"
									>
										<ContentCopyIcon fontSize="small" />
									</IconButton>
								</Tooltip>
							</Stack>
							<Typography
								variant="caption"
								color="text.secondary"
								sx={{ display: "block", mt: 1 }}
							>
								Share the passphrase over a separate channel (e.g. a call).
							</Typography>
						</Box>
					</>
				) : mint.pending ? (
					<Box
						component="pre"
						sx={{
							m: 0,
							p: 1.5,
							borderRadius: 1,
							bgcolor: "action.hover",
							fontSize: "0.85em",
							fontFamily: "monospace",
						}}
					>
						Minting enrollment ticket…
					</Box>
				) : outstanding ? (
					<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
						<Typography
							variant="body2"
							color="text.secondary"
							sx={{ flex: 1 }}
						>
							A {reEnroll ? "re-enrollment" : "enrollment"} ticket has been
							issued{issuedOn ? ` on ${issuedOn}` : ""}. You can't see it
							again: reissue to generate a new one (will cancel the other).
						</Typography>
						{reissueButton}
					</Stack>
				) : (
					<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
						<CircularProgress size={16} />
						<Typography variant="body2" color="text.secondary">
							Loading…
						</Typography>
					</Stack>
				)}

				{reEnroll && (ticket != null || outstanding) && (
					<Box>
						<GradedAction calls="fleet/machines/revoke_enrollment">
							<Button
								size="small"
								color="error"
								onClick={onCancel}
								disabled={revoke.pending}
							>
								{revoke.pending ? "Cancelling…" : "Cancel re-enrollment"}
							</Button>
						</GradedAction>
					</Box>
				)}

				{mint.error && <Alert severity="error">{mint.error.message}</Alert>}
				{revoke.error && (
					<Alert severity="error">{revoke.error.message}</Alert>
				)}
			</Stack>
		</Paper>
	);
}

function RegistrationState({
	registered,
	tokenExpiresAt,
}: {
	registered: boolean;
	tokenExpiresAt: string | null;
}) {
	if (registered) {
		return (
			<Chip
				size="small"
				color="success"
				icon={<CheckCircleIcon />}
				label="registered ✓"
			/>
		);
	}
	return (
		<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
			<CircularProgress size={14} />
			<Typography variant="body2" color="text.secondary">
				waiting for this machine to check in…
				{tokenExpiresAt && (
					<>
						{" "}
						(ticket expires <TimeAgo timestamp={tokenExpiresAt} />)
					</>
				)}
			</Typography>
		</Stack>
	);
}
