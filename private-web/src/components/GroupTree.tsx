import {
	Box,
	Chip,
	Link as MuiLink,
	Stack,
	type Theme,
	Tooltip,
	Typography,
} from "@mui/material";
import { alpha } from "@mui/material/styles";
import { Link as RouterLink } from "react-router-dom";

import {
	applicationName,
	type GroupEnvironment,
	type GroupMachine,
	groupServersByRank,
	type RankedMachine,
	rankMachines,
	resolveHeldBy,
	type ServerInfo,
	type ServerRank,
} from "../types";
import ApplicationTypeChip from "./ApplicationTypeChip";
import { waveWhileHolding } from "./MachineEnclosure";
import MachineMark from "./MachineMark";
import StatusDot from "./StatusDot";

/// The group as an operator navigates it: rank, then the boxes at that rank,
/// then the workloads on each box. A box serving no environment yet is listed
/// apart after them: awaiting a rank once something on it has reported,
/// awaiting check-in while nothing has.
///
/// The group page and both detail pages end with this, so an operator learns
/// one arrangement and reads it everywhere, and moving sideways never goes back
/// through the group. Whichever page it is rendered on is marked in place
/// rather than omitted, so the tree reads as a map.
/// spec: FLT#navigating-the-two-grains
export default function GroupTree({
	machines,
	applications,
	environments,
	groupName,
	currentMachineId,
	currentApplicationId,
}: {
	machines: GroupMachine[];
	applications: ServerInfo[];
	groupName?: string | null;
	/// The group's environments and whether a window holds over each, so the
	/// row a window was declared over carries the mark rather than only the
	/// boxes it caught.
	// spec: MNT#presentation
	environments?: GroupEnvironment[];
	/// The machine whose page this is, if any.
	currentMachineId?: string;
	/// The application whose page this is, if any.
	currentApplicationId?: string;
}) {
	const ranked = rankMachines(machines, applications);
	const sections: Array<[Section, RankedMachine[]]> = [];
	for (const [rank, boxes] of groupServersByRank(ranked)) {
		if (rank !== null) {
			sections.push([rank, boxes]);
			continue;
		}
		const reported = boxes.filter((box) => box.applications.length > 0);
		const silent = boxes.filter((box) => box.applications.length === 0);
		if (reported.length > 0) sections.push(["pending", reported]);
		if (silent.length > 0) sections.push(["awaiting-check-in", silent]);
	}

	return (
		<Box data-testid="group-tree">
			{sections.map(([section, boxes], index) => {
				const rank =
					section === "pending" || section === "awaiting-check-in"
						? null
						: section;
				const environment = environments?.find((e) => e.rank === rank);
				const held = environment?.maintained === true;
				const settling = environment?.maintenance_settling === true;
				return (
					<Box
						key={section}
						data-testid="tree-environment"
						data-rank={section}
						data-maintenance={
							held ? (settling ? "settling" : "holding") : undefined
						}
						sx={{ mt: index === 0 ? 0 : 1.5 }}
					>
						<EnvironmentHeading section={section} />
						<Stack spacing={1} data-testid="tree-boxes">
							{boxes.map((box) => (
								<MachineBlock
									key={box.machine.id}
									machine={box.machine}
									applications={box.applications}
									environmentWindow={held ? { settling } : null}
									heldBy={resolveHeldBy({
										rank,
										environmentHeld: held,
										groupName,
									})}
									currentMachineId={currentMachineId}
									currentApplicationId={currentApplicationId}
								/>
							))}
						</Stack>
					</Box>
				);
			})}
		</Box>
	);
}

/// A row of the tree: an environment, or one of the two places a box serving
/// none waits.
type Section = ServerRank | "pending" | "awaiting-check-in";

const SECTION_HEADINGS: Record<"pending" | "awaiting-check-in", string> = {
	pending: "awaiting a rank",
	"awaiting-check-in": "awaiting check-in",
};

/// A section's row. A window over an environment is drawn on the boxes under
/// it rather than here.
// spec: MNT#presentation
function EnvironmentHeading({ section }: { section: Section }) {
	return (
		<Typography
			variant="overline"
			color="text.secondary"
			sx={{ display: "block", mb: 0.5 }}
		>
			{section === "pending" || section === "awaiting-check-in"
				? SECTION_HEADINGS[section]
				: section}
		</Typography>
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
	environmentWindow,
	heldBy,
	currentMachineId,
	currentApplicationId,
}: {
	machine: GroupMachine;
	applications: ServerInfo[];
	environmentWindow?: { settling: boolean } | null;
	/// What holds a window this box did not have declared over it.
	// spec: MNT#presentation
	heldBy: string;
	currentMachineId?: string;
	currentApplicationId?: string;
}) {
	const current = machine.id === currentMachineId;
	const name = machine.name;
	const own = machine.own_window === true;
	const boxHeldBy = machine.maintained ? heldBy : null;
	const applicationHeldBy = own
		? resolveHeldBy({ ownWindow: true, machineName: name })
		: boxHeldBy;
	return (
		<Box
			data-testid="tree-block"
			sx={{
				border: 1,
				borderColor: "divider",
				borderRadius: 1,
				overflow: "hidden",
				...(environmentWindow
					? {
							backgroundImage: (theme: Theme) =>
								environmentHatch(theme, environmentWindow.settling),
							backgroundClip: "padding-box",
							...waveWhileHolding(!environmentWindow.settling, "&::before"),
						}
					: {}),
			}}
		>
			<Row
				current={current}
				sx={{ p: 1.5, gap: 1.5 }}
				data-testid="tree-machine"
			>
				{/* The rows below list the applications, so the box is drawn alone. */}
				<MachineMark
					up={machine.up}
					health={machine.health}
					name={machine.name}
					maintained={machine.maintained}
					settling={machine.maintenance_settling}
					ownWindow={machine.own_window}
					heldBy={boxHeldBy}
				/>
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
								// Indented past the machine's mark above, so the
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
								settling={application.maintenance_settling === true}
								suspended={application.maintained ?? false}
								heldBy={applicationHeldBy}
								title={applicationName(application)}
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

/// The wash over each box in an environment while a window over it holds:
/// light enough that the cards inside stay readable through it.
// spec: MNT#presentation
function environmentHatch(theme: Theme, settling: boolean): string {
	const ink = alpha(theme.palette.text.primary, settling ? 0.035 : 0.09);
	return `repeating-linear-gradient(45deg, ${ink} 0 1px, transparent 1px 7px)`;
}

/// The dot is sized in em so it follows the row's text.
const DOT_SIZE = "0.9em";

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
