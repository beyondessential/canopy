import {
	Alert,
	Box,
	Button,
	Chip,
	Dialog,
	DialogActions,
	DialogContent,
	DialogTitle,
	MenuItem,
	Paper,
	Stack,
	Table,
	TableBody,
	TableCell,
	TableHead,
	TableRow,
	TextField,
	Typography,
} from "@mui/material";
import { useState } from "react";
import { useApi, useApiAction } from "../api";
import {
	type DnsNameKind,
	KIND_HEADINGS,
	KIND_MODULES,
	KIND_NOUNS,
} from "../dnsNames";
import type {
	MachineNamesView as View,
	UndeclaredView as Undeclared,
} from "../types";
import CertificateStateChip from "./CertificateStateChip";
import { GradedAction } from "./GradedAction";
import TimeAgo from "./TimeAgo";
import TimeLeft from "./TimeLeft";

type MachineApplication = View["applications"][number];
type Declared = View["declared"][number];

/// The DNS names of one kind asked about from a machine: the requests that
/// resolved to none of its applications, waiting on an operator to declare or
/// deny each, the DNS names denied to it, and, on a box hosting several
/// applications, which of them declares what.
///
/// One component for both kinds, fed from the endpoint of the kind it is given,
/// so the "DNS names" and "TLS certificates" sections read alike. Absent
/// altogether on a machine with nothing to show for its kind, so a box that
/// never asks about one carries no empty section.
// spec: DNS#on-a-machine
export default function MachineNamesSection({
	kind,
	machineId,
	isAdmin,
	refreshKey,
	onChanged,
}: {
	kind: DnsNameKind;
	machineId: string;
	isAdmin: boolean;
	refreshKey: number;
	onChanged: () => void;
}) {
	const module = KIND_MODULES[kind];
	const view = useApi(module, "for_machine", { machine_id: machineId }, [
		machineId,
		kind,
		refreshKey,
	]);

	if (view.status !== "ok") return null;
	const data: View = view.data;
	const showDeclared = data.applications.length > 1 && data.declared.length > 0;
	if (data.undeclared.length === 0 && data.denied.length === 0 && !showDeclared)
		return null;

	return (
		<Paper
			variant="outlined"
			sx={{ p: 2 }}
			data-testid={`machine-names-${kind}`}
		>
			<Typography variant="h6" component="h2" gutterBottom>
				{KIND_HEADINGS[kind]}
			</Typography>
			<Stack spacing={2}>
				{data.undeclared.length > 0 && (
					<Box>
						<Typography variant="subtitle2" gutterBottom>
							Not yet declared
						</Typography>
						<Alert severity="warning" sx={{ mb: 1 }}>
							Canopy refuses these until each is declared on one application, or
							denied.
						</Alert>
						<Table size="small">
							<TableHead>
								<TableRow>
									<TableCell>DNS name</TableCell>
									<TableCell>Last asked</TableCell>
									<TableCell />
								</TableRow>
							</TableHead>
							<TableBody>
								{data.undeclared.map((row) => (
									<UndeclaredRow
										key={row.name}
										kind={kind}
										machineId={machineId}
										row={row}
										applications={data.applications}
										isAdmin={isAdmin}
										onChanged={onChanged}
									/>
								))}
							</TableBody>
						</Table>
					</Box>
				)}

				{showDeclared && (
					<Box>
						<Typography variant="subtitle2" gutterBottom>
							Declared
						</Typography>
						<Table size="small">
							<TableHead>
								<TableRow>
									<TableCell>DNS name</TableCell>
									<TableCell>Application</TableCell>
									<TableCell>
										{kind === "addresses" ? "Records" : "Certificate"}
									</TableCell>
								</TableRow>
							</TableHead>
							<TableBody>
								{data.declared.map((row) => (
									<TableRow key={row.name}>
										<TableCell sx={{ fontFamily: "monospace" }}>
											{row.name}
										</TableCell>
										<TableCell>{row.application_name}</TableCell>
										<TableCell>
											<DeclaredState kind={kind} row={row} />
										</TableCell>
									</TableRow>
								))}
							</TableBody>
						</Table>
					</Box>
				)}

				{data.denied.length > 0 && (
					<Box>
						<Typography variant="subtitle2" gutterBottom>
							Denied
						</Typography>
						<Table size="small">
							<TableHead>
								<TableRow>
									<TableCell>DNS name</TableCell>
									<TableCell>Denied</TableCell>
									<TableCell />
								</TableRow>
							</TableHead>
							<TableBody>
								{data.denied.map((row) => (
									<DeniedRow
										key={row.name}
										kind={kind}
										machineId={machineId}
										name={row.name}
										deniedBy={row.denied_by}
										deniedAt={row.denied_at}
										note={row.note ?? null}
										isAdmin={isAdmin}
										onChanged={onChanged}
									/>
								))}
							</TableBody>
						</Table>
					</Box>
				)}
			</Stack>
		</Paper>
	);
}

/// What a declared DNS name shows for its kind: whether its records are
/// published, or the state of its certificate and how long is left.
function DeclaredState({ kind, row }: { kind: DnsNameKind; row: Declared }) {
	if (kind === "addresses") {
		if (row.published === null) return <Chip size="small" label="declared" />;
		return row.published ? (
			<Chip size="small" variant="outlined" color="success" label="published" />
		) : (
			<Chip
				size="small"
				variant="outlined"
				color="warning"
				label="waiting to publish"
			/>
		);
	}
	if (!row.certificate) {
		return (
			<Typography variant="caption" color="text.secondary">
				none
			</Typography>
		);
	}
	return (
		<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
			<CertificateStateChip cert={row.certificate} />
			{row.certificate.not_after && (
				<TimeLeft
					notAfter={row.certificate.not_after}
					risk={row.certificate.risk}
				/>
			)}
		</Stack>
	);
}

function UndeclaredRow({
	kind,
	machineId,
	row,
	applications,
	isAdmin,
	onChanged,
}: {
	kind: DnsNameKind;
	machineId: string;
	row: Undeclared;
	applications: MachineApplication[];
	isAdmin: boolean;
	onChanged: () => void;
}) {
	const module = KIND_MODULES[kind];
	const [applicationId, setApplicationId] = useState(applications[0]?.id ?? "");
	const [denying, setDenying] = useState(false);
	const declare = useApiAction(module, "declare");

	const onDeclare = async () => {
		try {
			await declare.call({ application_id: applicationId, name: row.name });
			onChanged();
		} catch {
			/* surfaced via declare.error */
		}
	};

	return (
		<TableRow data-testid="undeclared-row">
			<TableCell sx={{ fontFamily: "monospace" }}>{row.name}</TableCell>
			<TableCell>
				<Typography variant="caption" color="text.secondary">
					<TimeAgo timestamp={row.last_asked_at} />
				</Typography>
			</TableCell>
			<TableCell align="right">
				{isAdmin && (
					<Stack spacing={1} sx={{ alignItems: "flex-end" }}>
						<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
							<TextField
								select
								size="small"
								value={applicationId}
								onChange={(e) => setApplicationId(e.target.value)}
								disabled={declare.pending}
								slotProps={{
									htmlInput: { "aria-label": "Application to declare it on" },
								}}
								sx={{ minWidth: 180 }}
							>
								{applications.map((application) => (
									<MenuItem key={application.id} value={application.id}>
										{application.name}
									</MenuItem>
								))}
							</TextField>
							<GradedAction
								calls={`${module}/declare`}
								action={`Declare DNS name ${row.name} for ${KIND_NOUNS[kind]}`}
							>
								<Button
									variant="contained"
									size="small"
									onClick={onDeclare}
									disabled={declare.pending || applicationId === ""}
								>
									Declare
								</Button>
							</GradedAction>
							<GradedAction
								calls={`${module}/deny`}
								action={`Deny DNS name ${row.name} for ${KIND_NOUNS[kind]}`}
							>
								<Button
									size="small"
									color="error"
									onClick={() => setDenying(true)}
								>
									Deny
								</Button>
							</GradedAction>
						</Stack>
						{declare.error && (
							<Alert severity="error">{declare.error.message}</Alert>
						)}
					</Stack>
				)}
				<DenyDialog
					open={denying}
					kind={kind}
					machineId={machineId}
					name={row.name}
					onClose={() => setDenying(false)}
					onDenied={() => {
						setDenying(false);
						onChanged();
					}}
				/>
			</TableCell>
		</TableRow>
	);
}

function DenyDialog({
	open,
	kind,
	machineId,
	name,
	onClose,
	onDenied,
}: {
	open: boolean;
	kind: DnsNameKind;
	machineId: string;
	name: string;
	onClose: () => void;
	onDenied: () => void;
}) {
	const module = KIND_MODULES[kind];
	const [note, setNote] = useState("");
	const deny = useApiAction(module, "deny");

	const onConfirm = async () => {
		try {
			await deny.call({
				machine_id: machineId,
				name,
				note: note.trim() === "" ? null : note.trim(),
			});
			setNote("");
			onDenied();
		} catch {
			/* surfaced via deny.error */
		}
	};

	return (
		<Dialog open={open} onClose={onClose} fullWidth maxWidth="sm">
			<DialogTitle>Deny {name}</DialogTitle>
			<DialogContent>
				<Typography variant="body2" sx={{ mb: 2 }}>
					This machine's requests for {KIND_NOUNS[kind]} for it are refused
					until the denial is lifted.
				</Typography>
				<TextField
					autoFocus
					fullWidth
					label="Note"
					size="small"
					value={note}
					onChange={(e) => setNote(e.target.value)}
					disabled={deny.pending}
				/>
				{deny.error && (
					<Alert severity="error" sx={{ mt: 2 }}>
						{deny.error.message}
					</Alert>
				)}
			</DialogContent>
			<DialogActions>
				<Button onClick={onClose}>Cancel</Button>
				<GradedAction
					calls={`${module}/deny`}
					action={`Deny DNS name ${name} for ${KIND_NOUNS[kind]}`}
				>
					<Button
						variant="contained"
						color="error"
						onClick={onConfirm}
						disabled={deny.pending}
					>
						Deny
					</Button>
				</GradedAction>
			</DialogActions>
		</Dialog>
	);
}

function DeniedRow({
	kind,
	machineId,
	name,
	deniedBy,
	deniedAt,
	note,
	isAdmin,
	onChanged,
}: {
	kind: DnsNameKind;
	machineId: string;
	name: string;
	deniedBy: string;
	deniedAt: string;
	note: string | null;
	isAdmin: boolean;
	onChanged: () => void;
}) {
	const module = KIND_MODULES[kind];
	const lift = useApiAction(module, "lift_denial");

	const onLift = async () => {
		try {
			await lift.call({ machine_id: machineId, name });
			onChanged();
		} catch {
			/* surfaced via lift.error */
		}
	};

	return (
		<TableRow data-testid="denied-row">
			<TableCell sx={{ fontFamily: "monospace" }}>{name}</TableCell>
			<TableCell>
				<Typography variant="caption" color="text.secondary">
					by {deniedBy} <TimeAgo timestamp={deniedAt} />
					{note && `: ${note}`}
				</Typography>
			</TableCell>
			<TableCell align="right">
				{isAdmin && (
					<GradedAction
						calls={`${module}/lift_denial`}
						action={`Lift denial of DNS name ${name} for ${KIND_NOUNS[kind]}`}
					>
						<Button size="small" onClick={onLift} disabled={lift.pending}>
							Lift
						</Button>
					</GradedAction>
				)}
				{lift.error && <Alert severity="error">{lift.error.message}</Alert>}
			</TableCell>
		</TableRow>
	);
}
