import { Box, Chip, Link as MuiLink, Stack, Tooltip, Typography } from "@mui/material";
import { Link as RouterLink } from "react-router-dom";
import {
	applicationName,
	type ApplicationType,
	type HealthState,
	type ServerRank,
	type ShortStatus,
} from "../types";
import ApplicationTypeChip from "./ApplicationTypeChip";
import TargetName from "./TargetName";
import ServerRankChip from "./ServerRankChip";
import StatusDot from "./StatusDot";

export interface ServerInfo {
	id: string;
	name: string | null;
	host?: string | null;
	display_host: string;
	type: ApplicationType;
	rank: ServerRank | null;
	group_name?: string | null;
	is_monitored?: boolean;
	up?: ShortStatus | null;
	health?: HealthState | null;
	/// Whether a window reaches it, and whether that window is its own. A window
	/// over the box it runs on suspends it without being declared over it.
	// spec: MNT#presentation
	maintained?: boolean | null;
	own_window?: boolean | null;
}

export default function ServerShorty({
	server,
	heldBy,
	current = false,
	withGroup = true,
}: {
	server: ServerInfo;
	/// What holds a window this application did not have declared over it.
	// spec: MNT#presentation
	heldBy?: string | null;
	/// Whether this row is the page the operator is already on. It is marked in
	/// place rather than omitted, so a tree reads as a map rather than as a list
	/// of everything else, and its name doesn't link back to where they are.
	/// spec: FLT
	current?: boolean;
	/// Whether to prefix the name with its group. Off inside a listing that is
	/// already one group's.
	withGroup?: boolean;
}) {
	const name = applicationName(server);
	const unmonitored = server.is_monitored === false;
	// The group is dropped where the caller is already inside one: a row in a
	// group's own tree naming that group again says nothing.
	const trail = [
		...(withGroup && server.group_name ? [{ label: server.group_name }] : []),
		{ label: name },
	];
	return (
		<Stack
			direction="row"
			spacing={2}
			sx={{
				p: 1.5,
				border: 1,
				borderColor: "divider",
				borderRadius: 1,
				alignItems: "center",
				bgcolor: current ? "action.hover" : undefined,
			}}
		>
			{server.up && (
				<StatusDot
					up={server.up}
					health={server.health ?? undefined}
					monitored={!unmonitored}
					maintained={server.own_window ?? false}
					suspended={server.maintained ?? false}
					heldBy={heldBy}
					title={name}
				/>
			)}
			{current ? (
				<Typography
					component="span"
					color="text.secondary"
					sx={{ fontWeight: 500 }}
				>
					<TargetName parts={trail} />
				</Typography>
			) : (
				<MuiLink
					component={RouterLink}
					to={`/fleet/applications/${server.id}`}
					underline="hover"
					color="text.primary"
					sx={{ fontWeight: 500 }}
				>
					<TargetName parts={trail} />
				</MuiLink>
			)}
			{server.rank && <ServerRankChip rank={server.rank} />}
			<ApplicationTypeChip type={server.type} />
			{unmonitored && (
				<Tooltip title="Status alerts are off for this server — canopy isn't watching it.">
					<Chip size="small" variant="outlined" label="unmonitored" />
				</Tooltip>
			)}
			<Box sx={{ ml: "auto" }}>
				<Typography variant="body2" color="text.secondary">
					{server.display_host}
				</Typography>
			</Box>
		</Stack>
	);
}
