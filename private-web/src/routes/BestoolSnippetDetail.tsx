import {
	Alert,
	Box,
	Button,
	Dialog,
	DialogActions,
	DialogContent,
	DialogTitle,
	LinearProgress,
	Paper,
	Stack,
	TextField,
	Typography,
} from "@mui/material";
import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { callApi, useApi, useApiAction } from "../api";
import { GradedAction } from "../components/GradedAction";
import SqlEditor from "../components/SqlEditor";
import { usePageTitle } from "../hooks/usePageTitle";
import type { BestoolSnippetDetail as Detail } from "../types";

export default function BestoolSnippetDetail() {
	const { id = "" } = useParams<{ id: string }>();
	const navigate = useNavigate();
	const detail = useApi(
		"bestool",
		"get_snippet",
		{ id },
		[id],
	);
	usePageTitle(detail.status === "ok" ? detail.data.name : "Snippet");

	// If this snippet has been superseded, redirect to the latest version.
	useEffect(() => {
		if (detail.status !== "ok") return;
		const currentId = detail.data.id;
		(async () => {
			try {
				const latest = await callApi("bestool", "get_latest_snippet_id", {
					id: currentId,
				});
				if (latest !== currentId) {
					navigate(`/bestool/snippets/${latest}`, { replace: true });
				}
			} catch {
				/* fall through — keep current */
			}
		})();
	}, [detail, navigate]);

	if (detail.status === "loading" || detail.status === "idle") return <LinearProgress />;
	if (detail.status === "error")
		return <Alert severity="error">{detail.error.message}</Alert>;

	return <View detail={detail.data} />;
}

function View({ detail }: { detail: Detail }) {
	const [editing, setEditing] = useState(false);
	const [confirmDelete, setConfirmDelete] = useState(false);
	const navigate = useNavigate();
	const updateAction = useApiAction("bestool", "save_snippet");
	const deleteAction = useApiAction("bestool", "delete_snippet");

	const [name, setName] = useState(detail.name);
	const [description, setDescription] = useState(detail.description ?? "");
	const [sql, setSql] = useState(detail.sql);

	const onSave = async (e: React.FormEvent) => {
		e.preventDefault();
		try {
			const updated = await updateAction.call({
				supersedes: detail.id,
				name,
				description: description.trim() === "" ? null : description,
				sql,
			});
			setEditing(false);
			navigate(`/bestool/snippets/${updated.id}`, { replace: true });
		} catch {
			/* surfaced via updateAction.error */
		}
	};

	const onDelete = async () => {
		try {
			await deleteAction.call({ id: detail.id });
			navigate("/bestool/snippets");
		} catch {
			/* surfaced via deleteAction.error */
		}
	};

	if (editing) {
		return (
			<Stack spacing={3}>
				<Typography variant="h4" component="h1">
					Edit
				</Typography>
				<Paper
					variant="outlined"
					sx={{ p: 2 }}
					component="form"
					onSubmit={onSave}
				>
					<Stack spacing={2}>
						<TextField
							label="Name"
							placeholder="example_snippet_name"
							value={name}
							onChange={(e) => setName(e.target.value)}
							disabled={updateAction.pending}
							required
						/>
						<TextField
							label="Description"
							placeholder="Optional description"
							value={description}
							onChange={(e) => setDescription(e.target.value)}
							disabled={updateAction.pending}
							multiline
							minRows={2}
						/>
						<Box>
							<Typography variant="caption" color="text.secondary">
								SQL
							</Typography>
							<Box
								sx={{
									mt: 0.5,
									border: 1,
									borderColor: "divider",
									borderRadius: 1,
									overflow: "hidden",
								}}
							>
								<SqlEditor
									value={sql}
									onChange={setSql}
									placeholder="SELECT ..."
									readOnly={updateAction.pending}
									minHeight="14em"
								/>
							</Box>
						</Box>
						{updateAction.error && (
							<Alert severity="error">{updateAction.error.message}</Alert>
						)}
						<Stack direction="row" spacing={1}>
							<GradedAction calls="bestool/save_snippet">
								<Button
									type="submit"
									variant="contained"
									disabled={updateAction.pending}
								>
									{updateAction.pending ? "Saving…" : "Save"}
								</Button>
							</GradedAction>
							<Button
								type="button"
								variant="outlined"
								color="error"
								onClick={() => {
									setName(detail.name);
									setDescription(detail.description ?? "");
									setSql(detail.sql);
									setEditing(false);
								}}
								disabled={updateAction.pending}
							>
								Cancel
							</Button>
						</Stack>
					</Stack>
				</Paper>
			</Stack>
		);
	}

	return (
		<Stack spacing={3}>
			<Stack
				direction="row"
				spacing={2}
				sx={{ alignItems: "center", justifyContent: "space-between" }}
				useFlexGap
			>
				<Typography
					variant="h4"
					component="h1"
					sx={{ fontFamily: "monospace" }}
				>
					{detail.name}
				</Typography>
				<Stack direction="row" spacing={2} sx={{ alignItems: "center" }}>
					<Typography variant="body2" color="text.secondary">
						Last edit by {detail.editor}
					</Typography>
					<GradedAction calls="bestool/save_snippet">
						<Button variant="contained" onClick={() => setEditing(true)}>
							Edit
						</Button>
					</GradedAction>
					<GradedAction calls="bestool/delete_snippet">
						<Button
							variant="outlined"
							color="error"
							onClick={() => setConfirmDelete(true)}
						>
							Delete
						</Button>
					</GradedAction>
				</Stack>
			</Stack>
			<Paper variant="outlined" sx={{ p: 2 }}>
				{detail.description && (
					<Typography variant="body1" sx={{ mb: 2 }}>
						{detail.description}
					</Typography>
				)}
				<Box
					sx={{
						border: 1,
						borderColor: "divider",
						borderRadius: 1,
						overflow: "hidden",
					}}
				>
					<SqlEditor value={detail.sql} onChange={() => {}} readOnly />
				</Box>
			</Paper>
			<Dialog
				open={confirmDelete}
				onClose={() => setConfirmDelete(false)}
				fullWidth
				maxWidth="sm"
			>
				<DialogTitle>Delete Snippet</DialogTitle>
				<DialogContent>
					<Typography>
						Are you sure you want to delete this snippet? This action cannot be
						undone.
					</Typography>
					{deleteAction.error && (
						<Alert severity="error" sx={{ mt: 2 }}>
							{deleteAction.error.message}
						</Alert>
					)}
				</DialogContent>
				<DialogActions>
					<Button
						onClick={() => setConfirmDelete(false)}
						disabled={deleteAction.pending}
					>
						Cancel
					</Button>
					<GradedAction calls="bestool/delete_snippet">
						<Button
							variant="contained"
							color="error"
							onClick={onDelete}
							disabled={deleteAction.pending}
						>
							{deleteAction.pending ? "Deleting…" : "Delete"}
						</Button>
					</GradedAction>
				</DialogActions>
			</Dialog>
		</Stack>
	);
}
