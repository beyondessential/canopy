import { Alert, Button, LinearProgress, Stack } from "@mui/material";
import AddIcon from "@mui/icons-material/Add";
import { Link as RouterLink } from "react-router-dom";
import GroupShorty from "../components/GroupShorty";
import { useApi } from "../api";
import { useIsAdmin } from "../hooks/useIsAdmin";
import { usePageTitle } from "../hooks/usePageTitle";
import { GradedAction } from "../components/GradedAction";

export default function GroupsList() {
	usePageTitle("Server groups");
	const groups = useApi("fleet/groups", "list", {}, []);
	const counts = useApi("fleet/groups", "server_counts", {}, []);
	const admin = useIsAdmin() === true;

	if (groups.status === "loading" || groups.status === "idle") {
		return <LinearProgress />;
	}
	if (groups.status === "error") {
		return <Alert severity="error">{groups.error.message}</Alert>;
	}

	// Live-member counts keyed by group id. Null until loaded (so no chip
	// flashes a premature 0); once loaded, groups absent from the map are 0.
	const countById =
		counts.status === "ok"
			? new Map(counts.data.map((c) => [c.server_group_id, c.server_count]))
			: null;

	return (
		<Stack spacing={2}>
			{admin && (
				<Stack direction="row" spacing={1} sx={{ justifyContent: "flex-end" }}>
					<GradedAction opens="fleet/groups/create">
						<Button
							component={RouterLink}
							to="/fleet/groups/new"
							variant="contained"
							startIcon={<AddIcon />}
						>
							New group
						</Button>
					</GradedAction>
				</Stack>
			)}
			{groups.data.length === 0 ? (
				<Alert severity="info">
					No server groups yet. Create one above — adding a server starts from
					its group.
				</Alert>
			) : (
				<Stack spacing={1}>
					{groups.data.map((g) => (
						<GroupShorty
							key={g.id}
							group={g}
							memberCount={countById ? (countById.get(g.id) ?? 0) : undefined}
						/>
					))}
				</Stack>
			)}
		</Stack>
	);
}
