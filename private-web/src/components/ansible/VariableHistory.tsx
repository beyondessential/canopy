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
import type { ScopeChange, VariableHistoryEntry } from "../../types";
import { GradedAction } from "../GradedAction";
import TimeAgo from "../TimeAgo";

/// The history of one variable, or of every variable at a scope, removed ones
/// among them, newest first: each change, who made it and when, and putting back
/// what a change replaced or removed where that can still be done.
// spec: ANSVAR#presentation
export default function VariableHistory({
	scope,
	name,
	title,
	onClose,
	onChanged,
}: {
	/** The scope arguments of the call. */
	scope: Record<string, unknown>;
	/** The variable, or none for the whole scope's history. */
	name: string | null;
	/** What the scope is called, for its history's title. */
	title: string;
	onClose: () => void;
	onChanged: () => void;
}) {
	const deps = [JSON.stringify(scope), name];
	const ofVariable = useApi(
		"ansible_variables",
		"history",
		{ ...scope, name },
		deps,
		{ skip: name === null },
	);
	const ofScope = useApi("ansible_variables", "scope_history", scope, deps, {
		skip: name !== null,
	});
	const history = name === null ? ofScope : ofVariable;
	const changes: ReadonlyArray<ScopeChange> =
		name === null
			? ofScope.status === "ok"
				? ofScope.data
				: []
			: ofVariable.status === "ok"
				? ofVariable.data.map((entry) => ({ ...entry, name }))
				: [];
	const restore = useApiAction("ansible_variables", "restore");

	const putBack = (change: ScopeChange) =>
		restore
			.call({ ...scope, name: change.name, entry_id: change.entry_id })
			.then(() => {
				onChanged();
				onClose();
			})
			.catch(() => {
				// Surfaced by the alert below.
			});

	return (
		<Dialog open onClose={onClose} fullWidth maxWidth="sm">
			<DialogTitle sx={name === null ? undefined : { fontFamily: "monospace" }}>
				{name ?? `${title} history`}
			</DialogTitle>
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
				{history.status === "ok" && changes.length === 0 && (
					<Typography variant="body2" color="text.secondary">
						No changes in the last 90 days
					</Typography>
				)}
				<Stack spacing={1.5}>
					{changes.map((change) => (
						<Stack
							key={change.entry_id}
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
									{name === null && <strong>{change.name} </strong>}
									{describe(change)}
								</Typography>
								<Typography variant="caption" color="text.secondary">
									{actorLabel(change.actor).name} ·{" "}
									<TimeAgo timestamp={change.recorded_at} />
								</Typography>
							</Stack>
							{change.undoable && (
								<GradedAction
									calls="ansible_variables/restore"
									action={`Put back ${change.name}`}
								>
									<Button
										size="small"
										disabled={restore.pending}
										onClick={() => putBack(change)}
										data-testid="history-restore"
									>
										{change.change === "removed" ? "Put back" : "Undo"}
									</Button>
								</GradedAction>
							)}
						</Stack>
					))}
				</Stack>
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
