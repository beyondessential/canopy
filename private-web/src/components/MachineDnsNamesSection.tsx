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
import type {
	AskedFor,
	MachineApplicationView as MachineApplication,
	MachineDnsNamesView as View,
	UndeclaredView as Undeclared,
} from "../types";
import { GradedAction } from "./GradedAction";
import TimeAgo from "./TimeAgo";

const ASKED_FOR_LABELS: Record<AskedFor, string> = {
	addresses: "Addresses",
	certificate: "Certificate",
};

/// The DNS names asked about from a machine: the requests that resolved to none
/// of its applications, waiting on an operator to declare or deny each, the
/// DNS names denied to it, and, on a box hosting several applications, which of
/// them declares what.
///
/// Absent altogether on a machine with nothing to show, so a box that never
/// asks about a DNS name carries no empty section.
// spec: NAM#on-a-machine
export default function MachineDnsNamesSection({
	machineId,
	isAdmin,
	refreshKey,
	onChanged,
}: {
	machineId: string;
	isAdmin: boolean;
	refreshKey: number;
	onChanged: () => void;
}) {
	const view = useApi("certificates", "for_machine", { machine_id: machineId }, [
		machineId,
		refreshKey,
	]);

	if (view.status !== "ok") return null;
	const data: View = view.data;
	const showDeclared = data.applications.length > 1 && data.declared.length > 0;
	if (data.undeclared.length === 0 && data.denied.length === 0 && !showDeclared)
		return null;

	return (
		<Paper variant="outlined" sx={{ p: 2 }} data-testid="machine-dns-names">
			<Typography variant="h6" component="h2" gutterBottom>
				DNS names
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
									<TableCell>Certificate</TableCell>
								</TableRow>
							</TableHead>
							<TableBody>
								{data.declared.map((row) => (
									<TableRow key={row.name}>
										<TableCell sx={{ fontFamily: "monospace" }}>{row.name}</TableCell>
										<TableCell>{row.application_name}</TableCell>
										<TableCell>
											{row.certificate ? (
												<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
													<Chip
														size="small"
														variant="outlined"
														color={row.certificate.collectable ? "success" : "default"}
														label={row.certificate.state}
													/>
													{row.certificate.not_after && (
														<Typography variant="caption" color="text.secondary">
															expires <TimeAgo timestamp={row.certificate.not_after} />
														</Typography>
													)}
												</Stack>
											) : (
												<Typography variant="caption" color="text.secondary">
													none
												</Typography>
											)}
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

function UndeclaredRow({
	machineId,
	row,
	applications,
	isAdmin,
	onChanged,
}: {
	machineId: string;
	row: Undeclared;
	applications: MachineApplication[];
	isAdmin: boolean;
	onChanged: () => void;
}) {
	const [applicationId, setApplicationId] = useState(applications[0]?.id ?? "");
	const [denying, setDenying] = useState(false);
	const declare = useApiAction("certificates", "declare");

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
					{ASKED_FOR_LABELS[row.asked_for]}, <TimeAgo timestamp={row.last_asked_at} />
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
								slotProps={{ htmlInput: { "aria-label": "Application to declare it on" } }}
								sx={{ minWidth: 180 }}
							>
								{applications.map((application) => (
									<MenuItem key={application.id} value={application.id}>
										{application.name}
									</MenuItem>
								))}
							</TextField>
							<GradedAction
								calls="certificates/declare"
								action={`Declare DNS name ${row.name}`}
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
								calls="certificates/deny"
								action={`Deny DNS name ${row.name}`}
							>
								<Button size="small" color="error" onClick={() => setDenying(true)}>
									Deny
								</Button>
							</GradedAction>
						</Stack>
						{declare.error && <Alert severity="error">{declare.error.message}</Alert>}
					</Stack>
				)}
				<DenyDialog
					open={denying}
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
	machineId,
	name,
	onClose,
	onDenied,
}: {
	open: boolean;
	machineId: string;
	name: string;
	onClose: () => void;
	onDenied: () => void;
}) {
	const [note, setNote] = useState("");
	const deny = useApiAction("certificates", "deny");

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
					This machine's requests for it are refused until the denial is lifted.
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
					calls="certificates/deny"
					action={`Deny DNS name ${name}`}
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
	machineId,
	name,
	deniedBy,
	deniedAt,
	note,
	isAdmin,
	onChanged,
}: {
	machineId: string;
	name: string;
	deniedBy: string;
	deniedAt: string;
	note: string | null;
	isAdmin: boolean;
	onChanged: () => void;
}) {
	const lift = useApiAction("certificates", "lift_denial");

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
						calls="certificates/lift_denial"
						action={`Lift denial of DNS name ${name}`}
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
