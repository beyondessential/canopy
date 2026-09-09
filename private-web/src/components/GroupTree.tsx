import { Box, Chip, Link as MuiLink, Stack, Tooltip, Typography } from "@mui/material";
import { Link as RouterLink } from "react-router-dom";

import {
	applicationName,
	type GroupMachine,
	groupServersByRank,
	rankMachines,
	type ServerInfo,
} from "../types";
import ApplicationTypeChip from "./ApplicationTypeChip";
import MachineEnclosure from "./MachineEnclosure";
import StatusDot from "./StatusDot";

/// The group as an operator navigates it: rank, then the boxes at that rank,
/// then the workloads on each box.
///
/// The group page and both detail pages end with this, so an operator learns
/// one arrangement and reads it everywhere, and moving sideways never goes back
/// through the group. Whichever page it is rendered on is marked in place
/// rather than omitted, so the tree reads as a map.
/// spec: FLT#navigating-the-two-grains
export default function GroupTree({
	machines,
	applications,
	currentMachineId,
	currentApplicationId,
}: {
	machines: GroupMachine[];
	applications: ServerInfo[];
	/// The machine whose page this is, if any.
	currentMachineId?: string;
	/// The application whose page this is, if any.
	currentApplicationId?: string;
}) {
	const ranked = rankMachines(machines, applications);

	return (
		<Box data-testid="group-tree">
			{groupServersByRank(ranked).map(([rank, boxes], index) => (
				<Box key={rank ?? "_unranked"}>
					<Typography
						variant="overline"
						color="text.secondary"
						sx={{ display: "block", mt: index === 0 ? 0 : 1.5, mb: 0.5 }}
					>
						{rank ?? "unranked"}
					</Typography>
					<Stack spacing={1}>
						{boxes.map((box) => (
							<MachineBlock
								key={box.machine.id}
								machine={box.machine}
								applications={box.applications}
								currentMachineId={currentMachineId}
								currentApplicationId={currentApplicationId}
							/>
						))}
					</Stack>
				</Box>
			))}
		</Box>
	);
}

/// One box and what runs on it, as a single bordered block.
///
/// The box and its workloads are one card rather than a heading over a list,
/// because the sharing is the thing being shown: two rows inside one border is
/// a fact about the host, while two rows under a label is a coincidence of
/// indentation.
function MachineBlock({
	machine,
	applications,
	currentMachineId,
	currentApplicationId,
}: {
	machine: GroupMachine;
	applications: ServerInfo[];
	currentMachineId?: string;
	currentApplicationId?: string;
}) {
	const current = machine.id === currentMachineId;
	const name = machine.name ?? "Unnamed machine";
	return (
		<Box sx={{ border: 1, borderColor: "divider", borderRadius: 1, overflow: "hidden" }}>
			<Row
				current={current}
				sx={{ p: 1.5, gap: 1.5 }}
				data-testid="tree-machine"
			>
				{/* The enclosure holds the dots for the applications below it, so
				    the head says what is on the box as well as how the box is. */}
				<MachineEnclosure
					up={machine.up}
					health={machine.health}
					name={machine.name}
					maintained={machine.maintained}
					settling={machine.maintenance_settling}
					ownWindow={machine.own_window}
					describes={applications.map((application) =>
						[
							applicationName(application),
							application.own_window ? "under maintenance" : null,
							application.is_monitored === false ? "unmonitored" : null,
						]
							.filter(Boolean)
							.join(" · "),
					)}
				>
					{applications.map((application) => (
						<Box key={application.id} component="span" sx={dotCellSx}>
							<StatusDot
								up={application.up ?? "gone"}
								health={application.health ?? undefined}
								monitored={application.is_monitored !== false}
								maintained={application.own_window ?? false}
								quiet
								size={DOT_SIZE}
							/>
						</Box>
					))}
				</MachineEnclosure>
				<Name to={current ? null : `/fleet/machines/${machine.id}`}>{name}</Name>
				<Meta>{machine.platform ?? ""}</Meta>
			</Row>
			{applications.length === 0 ? (
				<Box sx={{ borderTop: 1, borderColor: "divider", px: 1.5, py: 1 }}>
					<Typography variant="body2" color="text.secondary">
						Awaiting check-in.
					</Typography>
				</Box>
			) : (
				<Box sx={{ borderTop: 1, borderColor: "divider" }}>
					{applications.map((application, index) => (
						<Row
							key={application.id}
							current={application.id === currentApplicationId}
							data-testid="tree-application"
							sx={{
								// Indented past the enclosure above, so the
								// workloads read as sitting on the box.
								pl: 3.75,
								pr: 1.5,
								py: 1,
								gap: 1.5,
								// Lighter than the rule under the head: the break
								// between two workloads on one box is subordinate
								// to the break between the box and its workloads.
								...(index > 0
									? { borderTop: 1, borderColor: DIVIDER_LIGHT }
									: {}),
							}}
						>
							<StatusDot
								up={application.up ?? "gone"}
								health={application.health ?? undefined}
								monitored={application.is_monitored !== false}
								maintained={application.own_window ?? false}
								size={DOT_SIZE}
							/>
							<Name
								to={
									application.id === currentApplicationId
										? null
										: `/fleet/applications/${application.id}`
								}
							>
								{applicationName(application)}
							</Name>
							<ApplicationTypeChip type={application.type} />
							{application.is_monitored === false && (
								<Tooltip title="Status alerts are off for this application — canopy isn't watching it.">
									<Chip size="small" variant="outlined" label="unmonitored" />
								</Tooltip>
							)}
							<Meta>{application.display_host}</Meta>
						</Row>
					))}
				</Box>
			)}
		</Box>
	);
}

const DIVIDER_LIGHT = "rgba(0, 0, 0, 0.06)";

// Every dot sits in an identical fixed-size cell, so the enclosure's dots
// align however many there are. Spacing comes from the enclosure's own gap.
/// The dot is sized to its cell, since a flex item wider than the cell holding
/// it is squeezed on one axis only and draws as an oval.
const DOT_SIZE = "0.9em";

const dotCellSx = {
	display: "inline-flex",
	width: DOT_SIZE,
	height: DOT_SIZE,
	alignItems: "center",
	justifyContent: "center",
	flex: "none",
	"& > span": { marginRight: 0 },
} as const;

function Row({
	current,
	sx,
	children,
	...rest
}: {
	current?: boolean;
	sx?: object;
	children: React.ReactNode;
} & Record<string, unknown>) {
	return (
		<Box
			sx={{
				display: "flex",
				alignItems: "center",
				minWidth: 0,
				bgcolor: current ? "action.hover" : undefined,
				...sx,
			}}
			{...rest}
		>
			{children}
		</Box>
	);
}

/// The row's own name. Muted and unlinked on the row an operator is already
/// on, so the tree reads as a map rather than a list of everything else.
function Name({ to, children }: { to: string | null; children: string }) {
	if (!to) {
		return (
			<Typography
				component="span"
				variant="body2"
				color="text.secondary"
				sx={{ fontWeight: 500 }}
				noWrap
			>
				{children}
			</Typography>
		);
	}
	return (
		<MuiLink
			component={RouterLink}
			to={to}
			variant="body2"
			underline="hover"
			color="text.primary"
			sx={{ fontWeight: 500 }}
			noWrap
		>
			{children}
		</MuiLink>
	);
}

/// The one fact each row carries besides its name: a box's platform, a
/// workload's address. Right-aligned, so the column reads down the block.
function Meta({ children }: { children: string }) {
	if (!children) return <Box sx={{ ml: "auto" }} />;
	return (
		<Typography
			variant="caption"
			color="text.secondary"
			sx={{ ml: "auto", pl: 1, flexShrink: 0 }}
		>
			{children}
		</Typography>
	);
}
