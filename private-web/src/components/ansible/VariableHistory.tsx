import {
	Alert,
	Button,
	Dialog,
	DialogActions,
	DialogContent,
	DialogTitle,
	LinearProgress,
	Stack,
	Typography,
} from "@mui/material";
import { useApi, useApiAction } from "../../api";
import { actorLabel } from "../../lib/audit";
import type { VariableHistoryEntry } from "../../types";
import { GradedAction } from "../GradedAction";
import TimeAgo from "../TimeAgo";

/// A variable's history, newest first: each change, who made it and when, and
/// putting back what a change replaced or removed where that can still be done.
// spec: ANSVAR#presentation
export default function VariableHistory({
	scope,
	name,
	onClose,
	onChanged,
}: {
	/** The scope arguments of the call. */
	scope: Record<string, unknown>;
	name: string;
	onClose: () => void;
	onChanged: () => void;
}) {
	const history = useApi(
		"ansible_variables",
		"history",
		{ ...scope, name },
		[JSON.stringify(scope), name],
	);
	const restore = useApiAction("ansible_variables", "restore");

	const putBack = (entry: VariableHistoryEntry) =>
		restore
			.call({ ...scope, name, entry_id: entry.entry_id })
			.then(() => {
				onChanged();
				onClose();
			})
			.catch(() => {
				// Surfaced by the alert below.
			});

	return (
		<Dialog open onClose={onClose} fullWidth maxWidth="sm">
			<DialogTitle sx={{ fontFamily: "monospace" }}>{name}</DialogTitle>
			<DialogContent data-testid="variable-history">
				{history.status === "loading" && <LinearProgress />}
				{history.status === "error" && (
					<Alert severity="warning">{history.error.message}</Alert>
				)}
				{restore.error && (
					<Alert severity="warning" sx={{ mb: 1 }}>
						{restore.error.message}
					</Alert>
				)}
				{history.status === "ok" && history.data.length === 0 && (
					<Typography variant="body2" color="text.secondary">
						No changes in the last 90 days
					</Typography>
				)}
				{history.status === "ok" && (
					<Stack spacing={1.5}>
						{history.data.map((entry) => (
							<Stack
								key={entry.entry_id}
								direction="row"
								spacing={1}
								sx={{ alignItems: "flex-start" }}
								data-testid="history-entry"
							>
								<Stack sx={{ flex: 1, minWidth: 0 }}>
									<Typography
										variant="body2"
										sx={{ fontFamily: "monospace", wordBreak: "break-all" }}
									>
										{describe(entry)}
									</Typography>
									<Typography variant="caption" color="text.secondary">
										{actorLabel(entry.actor).name} ·{" "}
										<TimeAgo timestamp={entry.recorded_at} />
									</Typography>
								</Stack>
								{entry.undoable && (
									<GradedAction
										calls="ansible_variables/restore"
										action={`Put back ${name}`}
									>
										<Button
											size="small"
											disabled={restore.pending}
											onClick={() => putBack(entry)}
											data-testid="history-restore"
										>
											{entry.change === "removed" ? "Put back" : "Undo"}
										</Button>
									</GradedAction>
								)}
							</Stack>
						))}
					</Stack>
				)}
			</DialogContent>
			<DialogActions>
				<Button onClick={onClose}>Close</Button>
			</DialogActions>
		</Dialog>
	);
}

function describe(entry: VariableHistoryEntry): string {
	if (entry.change === "removed") {
		return entry.secret ? "Removed, secret" : `Removed ${show(entry.value)}`;
	}
	const set = entry.secret ? "Set, secret" : `Set to ${show(entry.value)}`;
	if (entry.replaced_secret) return `${set}, over a secret`;
	if (entry.replaced !== undefined && entry.replaced !== null)
		return `${set}, was ${show(entry.replaced)}`;
	return set;
}

function show(value: unknown): string {
	return JSON.stringify(value);
}
