import {
	Alert,
	Box,
	Button,
	LinearProgress,
	Link as MuiLink,
	Paper,
	Stack,
	Typography,
} from "@mui/material";
import RestoreIcon from "@mui/icons-material/RestoreFromTrash";
import { Link as RouterLink } from "react-router-dom";
import ServerShorty, { type ServerInfo } from "../components/ServerShorty";
import { useApi, useApiAction } from "../api";
import { GradedAction } from "../components/GradedAction";
import { useIsAdmin } from "../hooks/useIsAdmin";
import { usePageTitle } from "../hooks/usePageTitle";
import { compareServersByRankThenType } from "../types";

/// Archived (soft-deleted) servers and groups, with Restore. The only place to
/// find them — every other listing filters archived rows out.
export default function ArchivedList() {
	usePageTitle("Archived");
	const admin = useIsAdmin() === true;
	const groups = useApi("fleet/groups", "list_archived", {}, []);
	const servers = useApi("fleet/applications", "list_archived", {}, []);

	if (
		groups.status === "loading" ||
		groups.status === "idle" ||
		servers.status === "loading" ||
		servers.status === "idle"
	) {
		return <LinearProgress />;
	}
	if (groups.status === "error")
		return <Alert severity="error">{groups.error.message}</Alert>;
	if (servers.status === "error")
		return <Alert severity="error">{servers.error.message}</Alert>;

	const archivedGroups = groups.data;
	const archivedServers = [...servers.data].sort(compareServersByRankThenType);

	if (archivedGroups.length === 0 && archivedServers.length === 0) {
		return <Alert severity="success">Nothing archived.</Alert>;
	}

	return (
		<Stack spacing={3}>
			{archivedGroups.length > 0 && (
				<Stack spacing={1}>
					<Typography variant="subtitle1">Archived groups</Typography>
					{archivedGroups.map((g) => (
						<ArchivedGroupRow
							key={g.id}
							id={g.id}
							name={g.name}
							admin={admin}
							onRestored={groups.reload}
						/>
					))}
				</Stack>
			)}
			{archivedServers.length > 0 && (
				<Stack spacing={1}>
					<Typography variant="subtitle1">Archived servers</Typography>
					{archivedServers.map((s) => (
						<ArchivedServerRow
							key={s.id}
							server={s}
							admin={admin}
							onRestored={servers.reload}
						/>
					))}
				</Stack>
			)}
		</Stack>
	);
}

function RestoreButton({
	pending,
	onClick,
	disabled,
}: {
	pending: boolean;
	onClick: () => void;
	/** Forwarded to the button, so a caller that blocks this control (see
	 * `GradedAction`) takes it out of the keyboard's reach as well. */
	disabled?: boolean;
}) {
	return (
		<Button
			size="small"
			startIcon={<RestoreIcon />}
			onClick={onClick}
			disabled={pending || disabled}
		>
			{pending ? "Restoring…" : "Restore"}
		</Button>
	);
}

function ArchivedGroupRow({
	id,
	name,
	admin,
	onRestored,
}: {
	id: string;
	name: string;
	admin: boolean;
	onRestored: () => void;
}) {
	const action = useApiAction("fleet/groups", "restore");
	const onRestore = async () => {
		try {
			await action.call({ server_group_id: id });
			onRestored();
		} catch {
			/* surfaced via action.error */
		}
	};
	return (
		<Paper
			variant="outlined"
			sx={{ p: 1.5, display: "flex", alignItems: "center", gap: 2 }}
		>
			<MuiLink
				component={RouterLink}
				to={`/fleet/groups/${id}`}
				underline="hover"
				color="text.primary"
				sx={{ fontWeight: 500 }}
			>
				{name}
			</MuiLink>
			<Box sx={{ ml: "auto" }}>
				{admin && (
					<GradedAction calls="fleet/groups/restore">
						<RestoreButton pending={action.pending} onClick={onRestore} />
					</GradedAction>
				)}
			</Box>
			{action.error && (
				<Typography color="error" variant="caption">
					{action.error.message}
				</Typography>
			)}
		</Paper>
	);
}

function ArchivedServerRow({
	server,
	admin,
	onRestored,
}: {
	server: ServerInfo;
	admin: boolean;
	onRestored: () => void;
}) {
	const action = useApiAction("fleet/applications", "restore");
	const onRestore = async () => {
		try {
			await action.call({ server_id: server.id });
			onRestored();
		} catch {
			/* surfaced via action.error */
		}
	};
	return (
		<Box sx={{ display: "flex", alignItems: "center", gap: 1 }}>
			<Box sx={{ flex: 1 }}>
				<ServerShorty server={server} />
			</Box>
			{admin && (
				<GradedAction calls="fleet/applications/restore">
					<RestoreButton pending={action.pending} onClick={onRestore} />
				</GradedAction>
			)}
		</Box>
	);
}
