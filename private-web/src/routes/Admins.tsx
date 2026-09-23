import {
	Alert,
	Box,
	Button,
	IconButton,
	LinearProgress,
	List,
	ListItem,
	ListItemText,
	Paper,
	Snackbar,
	Stack,
	TextField,
	Typography,
} from "@mui/material";
import DeleteIcon from "@mui/icons-material/Delete";
import { type FormEvent, useState } from "react";
import { ApiError, callApi, useApi } from "../api";
import { GradedAction } from "../components/GradedAction";
import { usePageTitle } from "../hooks/usePageTitle";

export default function Admins() {
	usePageTitle("Admins");
	const list = useApi("admins", "list");
	const [email, setEmail] = useState("");
	const [pending, setPending] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const [toast, setToast] = useState<string | null>(null);

	const onAdd = async (e: FormEvent) => {
		e.preventDefault();
		const trimmed = email.trim();
		if (!trimmed) return setError("Email cannot be empty");
		if (!trimmed.includes("@")) return setError("Please enter a valid email address");
		setPending(true);
		setError(null);
		try {
			await callApi("admins", "add", { email: trimmed });
			setEmail("");
			setToast("Admin added");
			list.reload();
		} catch (err) {
			setError(formatError(err));
		} finally {
			setPending(false);
		}
	};

	const onDelete = async (target: string) => {
		try {
			await callApi("admins", "delete", { email: target });
			list.reload();
		} catch (err) {
			setError(formatError(err));
		}
	};

	return (
		<Stack spacing={3}>
			<Paper variant="outlined" sx={{ p: 2 }}>
				<Box component="form" onSubmit={onAdd}>
					<Stack
						direction="row"
						spacing={1}
						sx={{ alignItems: "flex-start" }}
					>
						<TextField
							type="email"
							size="small"
							fullWidth
							label="Email"
							placeholder="admin@example.com"
							value={email}
							onChange={(e) => setEmail(e.target.value)}
							disabled={pending}
						/>
						<GradedAction calls="admins/add">
							<Button
								type="submit"
								variant="contained"
								disabled={pending}
								sx={{ whiteSpace: "nowrap", flexShrink: 0 }}
							>
								{pending ? "Adding…" : "Add admin"}
							</Button>
						</GradedAction>
					</Stack>
					{error && (
						<Alert severity="error" sx={{ mt: 2 }}>
							{error}
						</Alert>
					)}
				</Box>
			</Paper>

			<Box>
				<Typography variant="h6" component="h2" gutterBottom>
					Admins
				</Typography>
				{list.status === "loading" || list.status === "idle" ? (
					<LinearProgress />
				) : list.status === "error" ? (
					<Alert severity="error">{list.error.message}</Alert>
				) : list.data.length === 0 ? (
					<Alert severity="info">No admins configured.</Alert>
				) : (
					<List>
						{list.data.map((admin) => (
							<ListItem
								key={admin}
								divider
								secondaryAction={
									<GradedAction calls="admins/delete">
										<IconButton
											edge="end"
											aria-label={`delete ${admin}`}
											onClick={() => onDelete(admin)}
										>
											<DeleteIcon />
										</IconButton>
									</GradedAction>
								}
							>
								<ListItemText
									slotProps={{
										primary: { sx: { fontFamily: "monospace" } },
									}}
									primary={admin}
								/>
							</ListItem>
						))}
					</List>
				)}
			</Box>

			<Snackbar
				open={!!toast}
				autoHideDuration={3000}
				onClose={() => setToast(null)}
				message={toast ?? ""}
			/>
		</Stack>
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
