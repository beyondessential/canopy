import {
	Alert,
	Box,
	Button,
	LinearProgress,
	Link as MuiLink,
	Pagination,
	Paper,
	Stack,
	TextField,
	Typography,
} from "@mui/material";
import { useState } from "react";
import { Link as RouterLink } from "react-router-dom";
import { useApi, useApiAction } from "../api";
import { GradedAction } from "../components/GradedAction";
import SqlEditor from "../components/SqlEditor";
import { usePageTitle } from "../hooks/usePageTitle";

const PAGE_SIZE = 10;

export default function BestoolSnippets() {
	usePageTitle("PSQL Snippets");
	const [page, setPage] = useState(0);
	const [showCreate, setShowCreate] = useState(false);

	const list = useApi(
		"bestool",
		"list_snippets",
		{ offset: page * PAGE_SIZE, limit: PAGE_SIZE },
		[page],
	);

	const refresh = () => list.reload();

	const total = list.status === "ok" ? list.data.total : 0;
	const pageCount = Math.max(1, Math.ceil(total / PAGE_SIZE));

	return (
		<Stack spacing={3}>
			<Stack
				direction="row"
				spacing={1}
				sx={{ alignItems: "center", justifyContent: "space-between" }}
			>
				<Typography variant="h4" component="h1">
					PSQL Snippets
				</Typography>
				<GradedAction calls={showCreate ? [] : "bestool/save_snippet"}>
					<Button
						variant={showCreate ? "outlined" : "contained"}
						color={showCreate ? "error" : undefined}
						onClick={() => setShowCreate((s) => !s)}
					>
						{showCreate ? "Cancel" : "Add"}
					</Button>
				</GradedAction>
			</Stack>

			{showCreate && (
				<CreateSnippetForm
					onCreated={() => {
						setShowCreate(false);
						refresh();
					}}
				/>
			)}

			{list.status === "loading" || list.status === "idle" ? (
				<LinearProgress />
			) : list.status === "error" ? (
				<Alert severity="error">{list.error.message}</Alert>
			) : list.data.items.length === 0 ? (
				<Alert severity="info">No snippets found.</Alert>
			) : (
				<Stack spacing={1}>
					{list.data.items.map((s) => (
						<MuiLink
							key={s.id}
							component={RouterLink}
							to={`/bestool/snippets/${s.id}`}
							underline="none"
							color="inherit"
						>
							<Paper
								variant="outlined"
								sx={{
									p: 1.5,
									transition: "background-color 150ms",
									"&:hover": { bgcolor: "action.hover" },
								}}
							>
								<Typography variant="body1" sx={{ fontFamily: "monospace" }}>
									{s.name}
								</Typography>
								{s.description && (
									<Typography variant="body2" color="text.secondary">
										{s.description}
									</Typography>
								)}
							</Paper>
						</MuiLink>
					))}
				</Stack>
			)}
			{pageCount > 1 && (
				<Box sx={{ display: "flex", justifyContent: "center" }}>
					<Pagination
						count={pageCount}
						page={page + 1}
						onChange={(_, p) => setPage(p - 1)}
					/>
				</Box>
			)}
		</Stack>
	);
}

function CreateSnippetForm({ onCreated }: { onCreated: () => void }) {
	const [name, setName] = useState("");
	const [description, setDescription] = useState("");
	const [sql, setSql] = useState("");
	const action = useApiAction("bestool", "save_snippet");

	const submit = async (e: React.FormEvent) => {
		e.preventDefault();
		try {
			await action.call({
				supersedes: null,
				name,
				description: description.trim() === "" ? null : description,
				sql,
			});
			setName("");
			setDescription("");
			setSql("");
			onCreated();
		} catch {
			/* surfaced via action.error */
		}
	};

	return (
		<Paper variant="outlined" sx={{ p: 2 }} component="form" onSubmit={submit}>
			<Stack spacing={2}>
				<TextField
					label="Name (will become the filename/snipname)"
					placeholder="example_snippet_name"
					value={name}
					onChange={(e) => setName(e.target.value)}
					disabled={action.pending}
					required
				/>
				<TextField
					label="Description"
					placeholder="Sentence about what it does and ${variables} required"
					value={description}
					onChange={(e) => setDescription(e.target.value)}
					disabled={action.pending}
					multiline
					minRows={2}
				/>
				<Box>
					<Typography variant="caption" color="text.secondary">
						SQL (no sensitive info! everything here may be read by anyone with bestool)
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
							readOnly={action.pending}
							minHeight="8em"
						/>
					</Box>
				</Box>
				{action.error && (
					<Alert severity="error">{action.error.message}</Alert>
				)}
				<Box>
					<GradedAction calls="bestool/save_snippet">
						<Button
							type="submit"
							variant="contained"
							disabled={action.pending}
						>
							{action.pending ? "Saving…" : "Save"}
						</Button>
					</GradedAction>
				</Box>
			</Stack>
		</Paper>
	);
}
