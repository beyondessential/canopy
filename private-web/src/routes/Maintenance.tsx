import {
	Alert,
	Button,
	LinearProgress,
	Paper,
	Stack,
	Table,
	TableBody,
	TableCell,
	TableHead,
	TableRow,
	Typography,
} from "@mui/material";
import { Link as RouterLink } from "react-router-dom";
import { useApi, useApiAction } from "../api";
import TimeAgo from "../components/TimeAgo";
import { useIsAdmin } from "../hooks/useIsAdmin";
import { usePageTitle } from "../hooks/usePageTitle";

/// What the fleet is not being watched on right now: every maintenance
/// window currently holding, what it covers, and when it ends.
// spec: MNT#presentation
export default function Maintenance() {
	usePageTitle("Maintenance");
	const isAdmin = useIsAdmin() === true;
	const list = useApi("maintenance", "list_open");
	const lift = useApiAction("maintenance", "lift");

	if (list.status === "loading" || list.status === "idle") {
		return <LinearProgress />;
	}
	if (list.status === "error") {
		return <Alert severity="error">{list.error.message}</Alert>;
	}

	const rows = list.data;

	return (
		<Stack spacing={2}>
			<Typography variant="h5" component="h1">
				Maintenance
			</Typography>
			<Typography variant="body2" color="text.secondary">
				Targets being worked on. Every check on them is recorded and shown,
				and raises nothing until the window ends and its settle period has
				passed.
			</Typography>
			{lift.error && <Alert severity="error">{lift.error.message}</Alert>}
			{rows.length === 0 ? (
				<Paper variant="outlined" sx={{ p: 3 }}>
					<Typography color="text.secondary">
						Nothing is under maintenance. Declare a window from a server or
						a group.
					</Typography>
				</Paper>
			) : (
				<Paper variant="outlined">
					<Table size="small">
						<TableHead>
							<TableRow>
								<TableCell>Target</TableCell>
								<TableCell>Ends</TableCell>
								<TableCell>Declared</TableCell>
								<TableCell>What's being done</TableCell>
								{isAdmin && <TableCell align="right" />}
							</TableRow>
						</TableHead>
						<TableBody>
							{rows.map(({ window, target }) => (
								<TableRow key={window.id} hover>
									<TableCell>
										{window.server_group_id ? (
											<RouterLink to={`/fleet/groups/${window.server_group_id}`}>
												{target}
											</RouterLink>
										) : window.machine_id ? (
											<RouterLink to={`/fleet/machines/${window.machine_id}`}>
												{target}
											</RouterLink>
										) : window.application_id ? (
											<RouterLink
												to={`/fleet/applications/${window.application_id}`}
											>
												{target}
											</RouterLink>
										) : (
											target
										)}
									</TableCell>
									<TableCell>
										<TimeAgo timestamp={window.expected_end} />
									</TableCell>
									<TableCell>
										<TimeAgo timestamp={window.declared_at} />
										{window.declared_by && ` by ${window.declared_by}`}
									</TableCell>
									<TableCell>{window.note ?? "—"}</TableCell>
									{isAdmin && (
										<TableCell align="right">
											<Button
												size="small"
												disabled={lift.pending}
												onClick={async () => {
													try {
														await lift.call({ id: window.id });
														list.reload();
													} catch {
														/* surfaced above */
													}
												}}
											>
												Lift
											</Button>
										</TableCell>
									)}
								</TableRow>
							))}
						</TableBody>
					</Table>
				</Paper>
			)}
		</Stack>
	);
}
