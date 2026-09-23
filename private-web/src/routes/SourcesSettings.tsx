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
	LinearProgress,
	Paper,
	Stack,
	Table,
	TableBody,
	TableCell,
	TableContainer,
	TableHead,
	TableRow,
	ToggleButton,
	ToggleButtonGroup,
	Typography,
} from "@mui/material";
import { useState } from "react";
import { Link as RouterLink } from "react-router-dom";
import { ApiError, useApi, useApiAction } from "../api";
import { GradedAction } from "../components/GradedAction";
import TimeAgo from "../components/TimeAgo";
import { useIsAdmin } from "../hooks/useIsAdmin";
import { usePageTitle } from "../hooks/usePageTitle";
import type { IngestMode, ReachabilityMode, SourceData } from "../types";

const REACHABILITY_MODES: ReachabilityMode[] = ["on", "quiet", "off"];
const INGEST_MODES: IngestMode[] = ["allow", "ignore", "deny"];

// What switching a source to each mode does, in operator terms. Shown in
// the confirmation dialog so a rarely-used, high-danger change is never
// applied without spelling out the consequence.
const REACHABILITY_CONSEQUENCE: Record<ReachabilityMode, string> = {
	on: "Canopy will warn when this source goes quiet, and count it toward marking a server unreachable once all of that server's sources have gone stale.",
	quiet: "This source going quiet will no longer raise a warning, but it still counts toward marking a server unreachable when it is the last source reporting.",
	off: "This source is excluded from reachability entirely. Its silence will never raise a warning or mark any server unreachable.",
};

const INGEST_CONSEQUENCE: Record<IngestMode, string> = {
	allow: "The device API will ingest this source's reports normally.",
	ignore: "The device API keeps accepting this source's pushes but discards their data before ingestion. The source's checks stop updating, and it cannot count toward reachability.",
	deny: "The device API rejects this source's pushes outright. Reporters receive an error, and the source cannot count toward reachability.",
};

export default function SourcesSettings() {
	usePageTitle("Sources");
	const isAdmin = useIsAdmin() === true;
	const sources = useApi("healthchecks", "sources");
	const rows = sources.status === "ok" ? sources.data : [];

	return (
		<Stack spacing={2}>
			<Box>
				<Typography variant="body2" color="text.secondary">
					<RouterLink to="/settings/healthchecks">← All healthchecks</RouterLink>
				</Typography>
				<Typography variant="h6" component="h2" gutterBottom>
					Sources
				</Typography>
				<Typography variant="body2" color="text.secondary">
					Reporters canopy expects to hear from. <strong>Reachability</strong>{" "}
					controls how a source going quiet affects its servers: <em>on</em>{" "}
					warns, <em>quiet</em> never warns but still marks a server unreachable
					when it's the only source left, <em>off</em> ignores it.{" "}
					<strong>Ingest</strong> controls the device API: <em>allow</em> accepts
					reports, <em>ignore</em> discards them, <em>deny</em> rejects the push.
					A source that isn't ingested can't count for reachability. These are
					fleet-wide settings — each change is confirmed before it takes effect.
				</Typography>
			</Box>

			{sources.status === "loading" || sources.status === "idle" ? (
				<LinearProgress />
			) : sources.status === "error" ? (
				<Alert severity="error">{sources.error.message}</Alert>
			) : rows.length === 0 ? (
				<Alert severity="info">
					No sources yet — a source appears here once it has reported a check.
				</Alert>
			) : (
				<Paper variant="outlined">
					<TableContainer>
						<Table size="small">
							<TableHead>
								<TableRow>
									<TableCell>Source</TableCell>
									<TableCell>Last seen</TableCell>
									<TableCell>Reachability</TableCell>
									<TableCell>Ingest</TableCell>
								</TableRow>
							</TableHead>
							<TableBody>
								{rows.map((row) => (
									<SourceRow
										key={row.source}
										row={row}
										canEdit={isAdmin}
										onChanged={() => sources.reload()}
									/>
								))}
							</TableBody>
						</Table>
					</TableContainer>
				</Paper>
			)}
		</Stack>
	);
}

function SourceRow({
	row,
	canEdit,
	onChanged,
}: {
	row: SourceData;
	canEdit: boolean;
	onChanged: () => void;
}) {
	const setReach = useApiAction("healthchecks", "set_source_reachability");
	const setIngest = useApiAction("healthchecks", "set_source_ingest");
	const [reach, setReachLocal] = useState<ReachabilityMode>(row.reachability);
	const [ingest, setIngestLocal] = useState<IngestMode>(row.ingest);

	// A pending change awaiting confirmation. The toggle stays put on its
	// confirmed value until the operator confirms in the dialog.
	const [pending, setPending] = useState<
		| { kind: "reachability"; to: ReachabilityMode }
		| { kind: "ingest"; to: IngestMode }
		| null
	>(null);

	// A source that isn't ingested is excluded from reachability; show the
	// reachability control as a disabled "off".
	const ingested = ingest === "allow";
	const reachShown: ReachabilityMode = ingested ? reach : "off";

	const requestReach = (mode: ReachabilityMode | null) => {
		if (!mode || mode === reach) return;
		setPending({ kind: "reachability", to: mode });
	};
	const requestIngest = (mode: IngestMode | null) => {
		if (!mode || mode === ingest) return;
		setPending({ kind: "ingest", to: mode });
	};

	const confirm = async () => {
		if (!pending) return;
		try {
			if (pending.kind === "reachability") {
				await setReach.call({ source: row.source, reachability: pending.to });
				setReachLocal(pending.to);
			} else {
				await setIngest.call({ source: row.source, ingest: pending.to });
				setIngestLocal(pending.to);
			}
			setPending(null);
			onChanged();
		} catch {
			// Dismiss the dialog; the failure surfaces beneath the row's
			// toggles, which stay on their unchanged confirmed value.
			setPending(null);
		}
	};

	return (
		<TableRow hover>
			<TableCell sx={{ fontFamily: "monospace" }}>{row.source}</TableCell>
			<TableCell>
				{row.last_seen ? (
					<TimeAgo timestamp={row.last_seen} />
				) : (
					<Typography variant="caption" color="text.secondary">
						never
					</Typography>
				)}
			</TableCell>
			<TableCell>
				{canEdit ? (
					<GradedAction calls="healthchecks/set_source_reachability">
						<ToggleButtonGroup
							size="small"
							exclusive
							value={reachShown}
							onChange={(_, v) => requestReach(v as ReachabilityMode | null)}
							disabled={setReach.pending || !ingested}
						>
							{REACHABILITY_MODES.map((mode) => (
								<ToggleButton key={mode} value={mode}>
									{mode}
								</ToggleButton>
							))}
						</ToggleButtonGroup>
					</GradedAction>
				) : (
					<Chip size="small" label={reachShown} />
				)}
				{setReach.error && (
					<Typography variant="caption" color="error" sx={{ display: "block" }}>
						{formatError(setReach.error)}
					</Typography>
				)}
			</TableCell>
			<TableCell>
				{canEdit ? (
					<GradedAction calls="healthchecks/set_source_ingest">
						<ToggleButtonGroup
							size="small"
							exclusive
							value={ingest}
							onChange={(_, v) => requestIngest(v as IngestMode | null)}
							disabled={setIngest.pending}
						>
							{INGEST_MODES.map((mode) => (
								<ToggleButton key={mode} value={mode}>
									{mode}
								</ToggleButton>
							))}
						</ToggleButtonGroup>
					</GradedAction>
				) : (
					<Chip size="small" label={ingest} />
				)}
				{setIngest.error && (
					<Typography variant="caption" color="error" sx={{ display: "block" }}>
						{formatError(setIngest.error)}
					</Typography>
				)}
			</TableCell>
			{pending && (
				<ConfirmModeDialog
					source={row.source}
					kind={pending.kind}
					to={pending.to}
					pending={setReach.pending || setIngest.pending}
					onConfirm={confirm}
					onCancel={() => setPending(null)}
				/>
			)}
		</TableRow>
	);
}

function ConfirmModeDialog({
	source,
	kind,
	to,
	pending,
	onConfirm,
	onCancel,
}: {
	source: string;
	kind: "reachability" | "ingest";
	to: ReachabilityMode | IngestMode;
	pending: boolean;
	onConfirm: () => void;
	onCancel: () => void;
}) {
	const label = kind === "reachability" ? "reachability" : "ingest";
	const consequence =
		kind === "reachability"
			? REACHABILITY_CONSEQUENCE[to as ReachabilityMode]
			: INGEST_CONSEQUENCE[to as IngestMode];
	return (
		<Dialog open onClose={onCancel} maxWidth="sm">
			<DialogTitle>
				Set {source} {label} to “{to}”?
			</DialogTitle>
			<DialogContent>
				<DialogContentText>{consequence}</DialogContentText>
			</DialogContent>
			<DialogActions>
				<Button onClick={onCancel} disabled={pending}>
					Cancel
				</Button>
				<GradedAction calls={
					kind === "reachability"
						? "healthchecks/set_source_reachability"
						: "healthchecks/set_source_ingest"
				}>
					<Button variant="contained" onClick={onConfirm} disabled={pending}>
						Confirm
					</Button>
				</GradedAction>
			</DialogActions>
		</Dialog>
	);
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
