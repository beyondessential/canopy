import {
	Alert as MuiAlert,
	Box,
	Button,
	Chip,
	Dialog,
	DialogActions,
	DialogContent,
	DialogTitle,
	FormControlLabel,
	IconButton,
	LinearProgress,
	Stack,
	Switch,
	TextField,
	Typography,
} from "@mui/material";
import AddCommentIcon from "@mui/icons-material/AddComment";
import DeleteIcon from "@mui/icons-material/Delete";
import { useState } from "react";
import { useApi, useApiAction } from "../api";
import { GradedAction } from "./GradedAction";
import { NoteText } from "./Markdown";
import TimeAgo from "./TimeAgo";

/** Shared note shape; both IssueNoteData and IncidentNoteData fit this. */
interface NoteLike {
	id: string;
	author: string;
	body: string;
	created_at: string;
	author_kind: "operator" | "agent";
	deleted_at?: string | null;
	deleted_by?: string | null;
}

type ApiModule = "issues" | "incidents";
type ParentKey = "issue_id" | "incident_id";

/** Read-only list of notes scoped by `(apiModule, parentKey, parentId)`.
 * Pair with `AddNoteButton` for write access; bump `refreshKey` after a
 * successful add to refetch.
 */
export default function NotesList({
	apiModule,
	parentKey,
	parentId,
	refreshKey = 0,
	canEdit = false,
}: {
	apiModule: ApiModule;
	parentKey: ParentKey;
	parentId: string;
	refreshKey?: number;
	/** Show the per-note delete button. Admins only — the API enforces it
	 * too, but hiding the button keeps the UI honest. */
	canEdit?: boolean;
}) {
	// Administrators can ask for the notes that were deleted too.
	const [showDeleted, setShowDeleted] = useState(false);
	const list = useApi(
		apiModule,
		"list_notes",
		{ [parentKey]: parentId, include_deleted: canEdit && showDeleted },
		[apiModule, parentId, refreshKey, showDeleted],
	);

	if (list.status === "loading" || list.status === "idle") {
		return <LinearProgress />;
	}
	if (list.status === "error") {
		return <MuiAlert severity="error">{list.error.message}</MuiAlert>;
	}
	const toggle = canEdit && (
		<FormControlLabel
			control={
				<Switch
					size="small"
					checked={showDeleted}
					onChange={(e) => setShowDeleted(e.target.checked)}
				/>
			}
			label="Show deleted"
			slotProps={{ typography: { variant: "caption" } }}
		/>
	);
	if (list.data.length === 0) {
		return (
			<Stack spacing={0.5}>
				<Typography variant="caption" color="text.secondary">
					No notes yet.
				</Typography>
				{toggle}
			</Stack>
		);
	}
	return (
		<Stack spacing={1}>
			{toggle}
			{list.data.map((n) => (
				<NoteRow
					key={n.id}
					note={n}
					apiModule={apiModule}
					canEdit={canEdit}
					onChanged={list.reload}
				/>
			))}
		</Stack>
	);
}

/** Button that opens a modal dialog containing the add-note form. */
export function AddNoteButton({
	apiModule,
	parentKey,
	parentId,
	onAdded,
	label = "Add note",
	variant = "text",
}: {
	apiModule: ApiModule;
	parentKey: ParentKey;
	parentId: string;
	onAdded?: () => void;
	label?: string;
	variant?: "text" | "outlined" | "contained";
}) {
	const [open, setOpen] = useState(false);
	const [draft, setDraft] = useState("");
	const add = useApiAction(apiModule, "add_note");
	const parent = apiModule === "issues" ? "issue" : "incident";

	const close = () => {
		setOpen(false);
		setDraft("");
	};
	const submit = async () => {
		const body = draft.trim();
		if (body === "") return;
		try {
			await add.call({ [parentKey]: parentId, body });
			setDraft("");
			setOpen(false);
			onAdded?.();
		} catch {
			/* surfaced via add.error */
		}
	};

	return (
		<>
			<GradedAction
				calls={`${apiModule}/add_note`}
				action={`Add note to this ${parent}`}
			>
				<Button
					size="small"
					variant={variant}
					startIcon={<AddCommentIcon />}
					onClick={() => setOpen(true)}
				>
					{label}
				</Button>
			</GradedAction>
			<Dialog open={open} onClose={close} fullWidth maxWidth="sm">
				<DialogTitle>{label}</DialogTitle>
				<DialogContent>
					<TextField
						autoFocus
						fullWidth
						multiline
						minRows={3}
						value={draft}
						onChange={(e) => setDraft(e.target.value)}
						disabled={add.pending}
						sx={{ mt: 1 }}
					/>
					{add.error && (
						<MuiAlert severity="error" sx={{ mt: 1 }}>
							{add.error.message}
						</MuiAlert>
					)}
				</DialogContent>
				<DialogActions>
					<Button onClick={close} disabled={add.pending}>
						Cancel
					</Button>
					<GradedAction
						calls={`${apiModule}/add_note`}
						action={`Add note to this ${parent}`}
					>
						<Button
							variant="contained"
							onClick={submit}
							disabled={add.pending || draft.trim() === ""}
						>
							{add.pending ? "Adding…" : "Add"}
						</Button>
					</GradedAction>
				</DialogActions>
			</Dialog>
		</>
	);
}

function NoteRow({
	note,
	apiModule,
	canEdit,
	onChanged,
}: {
	note: NoteLike;
	apiModule: ApiModule;
	canEdit: boolean;
	onChanged: () => void;
}) {
	const del = useApiAction(apiModule, "delete_note");
	const parent = apiModule === "issues" ? "issue" : "incident";

	const remove = async () => {
		try {
			await del.call({ note_id: note.id });
			onChanged();
		} catch {
			/* surfaced via del.error */
		}
	};

	return (
		<Box
			sx={{
				p: 1,
				border: 1,
				borderColor: "divider",
				borderRadius: 1,
				bgcolor: "background.paper",
				opacity: note.deleted_at ? 0.6 : 1,
			}}
		>
			<Stack
				direction="row"
				spacing={1}
				sx={{ alignItems: "center", justifyContent: "space-between" }}
			>
				<Stack direction="row" spacing={0.5} sx={{ alignItems: "center" }}>
					<Typography variant="caption" color="text.secondary">
						{note.author}
					</Typography>
					{note.author_kind === "agent" && (
						<Chip size="small" variant="outlined" label="Agent" />
					)}
				</Stack>
				<Stack direction="row" spacing={0.5} sx={{ alignItems: "center" }}>
					<Typography variant="caption" color="text.secondary">
						<TimeAgo timestamp={note.created_at} />
					</Typography>
					{canEdit && !note.deleted_at && (
						<GradedAction
							calls={`${apiModule}/delete_note`}
							action={`Delete note by ${note.author} from this ${parent}`}
						>
							<IconButton
								size="small"
								aria-label="Delete"
								onClick={remove}
								disabled={del.pending}
							>
								<DeleteIcon fontSize="inherit" />
							</IconButton>
						</GradedAction>
					)}
				</Stack>
			</Stack>
			<Box sx={{ mt: 0.5 }}>
				<NoteText>{note.body}</NoteText>
			</Box>
			{note.deleted_at && (
				<Typography variant="caption" color="text.secondary">
					Deleted by {note.deleted_by} <TimeAgo timestamp={note.deleted_at} />
				</Typography>
			)}
			{del.error && (
				<MuiAlert severity="error" sx={{ mt: 1 }}>
					{del.error.message}
				</MuiAlert>
			)}
		</Box>
	);
}
