import {
	Alert,
	Box,
	Button,
	Card,
	CardContent,
	LinearProgress,
	Link as MuiLink,
	Stack,
	Tooltip,
	Typography,
	useTheme,
	type Theme,
} from "@mui/material";
import { alpha } from "@mui/material/styles";
import BarChartIcon from "@mui/icons-material/BarChart";
import PersonIcon from "@mui/icons-material/Person";
import { useMemo } from "react";
import { Link as RouterLink } from "react-router-dom";
import MachineEnclosure from "../components/MachineEnclosure";
import StatusDot from "../components/StatusDot";
import VersionIndicator from "../components/VersionIndicator";
import { useVersionTrackingAcross } from "../hooks/useApplicationTypes";
import {
	HealthLegend,
	OperatorLegend,
	StatusLegend,
	VersionLegend,
} from "../components/Legends";
import { useApi } from "../api";
import { usePageTitle } from "../hooks/usePageTitle";
import { useReloadInterval } from "../hooks/useReloadInterval";
import {
	type AggregatedOperator,
	type FacilityServerStatus,
	type ServerGroupCard,
	type ServerRank,
	SERVER_RANK_ORDER,
	aggregateOperators,
	compareServersByRankThenType,
	groupServersByRank,
	isIncidentLingering,
} from "../types";

/// How an open incident should read at a glance:
/// - "loud": failing, and the Slack notice has fired (or been given up on);
/// - "held": failing, but the notice is still inside the per-group cooldown
///   window — nobody has been paged yet;
/// - "lingering": every failure has recovered, and the incident is waiting
///   out the group's linger window in case one comes back.
/// Drives the error/warning/info colouring on the Status page and elsewhere.
export type IncidentLoudness = "held" | "loud" | "lingering";

/// Loudest first, for rolling several of a group's incidents into one mark.
const LOUDNESS_ORDER: IncidentLoudness[] = ["loud", "held", "lingering"];

/// A group's open incidents by target: one entry per environment in trouble,
/// and `null` for the group's own.
type GroupIncidents = Map<ServerRank | null, IncidentLoudness>;

function loudest(incidents: GroupIncidents | null): IncidentLoudness | null {
	let worst: IncidentLoudness | null = null;
	for (const loudness of incidents?.values() ?? []) {
		if (
			worst == null ||
			LOUDNESS_ORDER.indexOf(loudness) < LOUDNESS_ORDER.indexOf(worst)
		) {
			worst = loudness;
		}
	}
	return worst;
}

/// A group's incidents in reading order: its environments highest rank first,
/// the group's own last.
function incidentTargets(
	incidents: GroupIncidents | null,
): Array<[ServerRank | null, IncidentLoudness]> {
	const order: Array<ServerRank | null> = [...SERVER_RANK_ORDER, null];
	const targets: Array<[ServerRank | null, IncidentLoudness]> = [];
	for (const rank of order) {
		const loudness = incidents?.get(rank);
		if (loudness) targets.push([rank, loudness]);
	}
	return targets;
}

export default function Status() {
	usePageTitle("Status");
	const tick = useReloadInterval(60_000, "canopy-reload-status");
	const incidents = useApi("incidents", "list_active", {}, [tick]);
	// `held` only counts while the deliver-after deadline is still ahead:
	// once it slips into the past the worker has shipped (or is about to)
	// and this page is just behind on the data — fall through to "loud"
	// rather than asserting a held state we can't verify. The 60s tick
	// re-evaluates this naturally.
	const now = Date.now();
	const openIncidentGroups = new Map<string, GroupIncidents>();
	if (incidents.status === "ok") {
		for (const i of incidents.data) {
			// Canopy-wide incidents (no group) surface via the self-alerts
			// banner, not the group cards.
			if (i.server_group_id == null) continue;
			// Lingering wins over held and loud for one incident: it is
			// currently green, so painting it red/yellow would overstate the
			// trouble.
			const loudness: IncidentLoudness = isIncidentLingering(i)
				? "lingering"
				: i.notification_held_until &&
						Date.parse(i.notification_held_until) > now
					? "held"
					: "loud";
			// A group holds one open incident per environment plus its own, and
			// the card marks each of them on the row it belongs to.
			let byRank = openIncidentGroups.get(i.server_group_id);
			if (byRank == null) {
				byRank = new Map();
				openIncidentGroups.set(i.server_group_id, byRank);
			}
			byRank.set(i.rank ?? null, loudness);
		}
	}
	return (
		<Stack spacing={3}>
			<ReleaseSummary tick={tick} />
			<GroupCards tick={tick} openIncidentGroups={openIncidentGroups} />
			<Box>
				<VersionLegend />
				<Box sx={{ mt: 1 }}>
					<StatusLegend />
				</Box>
				<Box sx={{ mt: 1 }}>
					<HealthLegend />
				</Box>
				<Box sx={{ mt: 1 }}>
					<OperatorLegend />
				</Box>
			</Box>
		</Stack>
	);
}

function ReleaseSummary({ tick }: { tick: number }) {
	const result = useApi("statuses", "summary", {}, [tick]);
	if (result.status === "loading" || result.status === "idle") {
		return <LinearProgress />;
	}
	if (result.status === "error") {
		return <Alert severity="error">{result.error.message}</Alert>;
	}
	const { releases, versions, bracket } = result.data;
	return (
		<Card variant="outlined">
			<CardContent>
				<Stack
					direction="row"
					spacing={2}
					sx={{
						alignItems: "center",
						justifyContent: "space-between",
						flexWrap: "wrap",
					}}
					useFlexGap
				>
					<Typography variant="body1">
						{releases.length} release{" "}
						{releases.length === 1 ? "branch" : "branches"} in active use:{" "}
						{releases
							.map(([major, minor]) => `${major}.${minor}`)
							.join(", ")}{" "}
						({versions.length}{" "}
						{versions.length === 1 ? "version" : "versions"}: {bracket.min} —{" "}
						{bracket.max})
					</Typography>
					{/* The version spread is one of several figures the fleet
					    reports; this card answers "which branches", the figures
					    page answers "which servers, and what else are they on". */}
					<Button
						component={RouterLink}
						to="/fleet/figures"
						variant="outlined"
						size="small"
						startIcon={<BarChartIcon />}
						sx={{ flexShrink: 0 }}
					>
						Fleet figures
					</Button>
				</Stack>
			</CardContent>
		</Card>
	);
}

function GroupCards({
	tick,
	openIncidentGroups,
}: {
	tick: number;
	openIncidentGroups: Map<string, GroupIncidents>;
}) {
	const groups = useApi("statuses", "group_ids", {}, [tick]);

	if (groups.status === "loading" || groups.status === "idle") {
		return <LinearProgress />;
	}
	if (groups.status === "error") {
		return <Alert severity="error">{groups.error.message}</Alert>;
	}

	// One grid, alphabetical. Each card carries its own ranks in its dot strip,
	// so sectioning or ordering by rank sorts the page by something already
	// written on every card — and leaves an operator looking for one group
	// scanning for where its rank happens to start. A name is what they know it
	// by, so a name is what the page is in order of.
	// spec: CHK#presentation
	const ids = groups.data;

	if (ids.length === 0) {
		return (
			<Alert severity="info">
				No server groups configured. Create one via the Servers page.
			</Alert>
		);
	}

	return (
		<Box
			sx={{
				display: "grid",
				gridTemplateColumns: "repeat(auto-fill, minmax(16em, 1fr))",
				gap: 1.5,
				// One step down for the whole grid. Everything inside a card is
				// sized in em from here — type, padding, dots, enclosures — so
				// this is the single knob for how dense the page is, and 16em
				// columns shrink with it.
				fontSize: "0.9rem",
			}}
		>
			{ids.map((id) => (
				<GroupCardLoader
					key={id}
					groupId={id}
					tick={tick}
					incidents={openIncidentGroups.get(id) ?? null}
				/>
			))}
		</Box>
	);
}

function GroupCardLoader({
	groupId,
	tick,
	incidents,
}: {
	groupId: string;
	tick: number;
	incidents: GroupIncidents | null;
}) {
	const result = useApi(
		"statuses",
		"group_details",
		{ server_group_id: groupId },
		[groupId, tick],
	);

	// The border takes the loudest of the group's environments: a card has to
	// catch the eye across the grid before its rank rows can say which one.
	//
	// Held incidents tone the border down to warning so an operator can see
	// at a glance "yes there's a thing, but Slack hasn't been told yet — it
	// might still self-resolve". Lingering incidents tone down further to
	// info: everything has recovered and the incident is just waiting out
	// its linger window. Loud incidents stay full red.
	const worst = loudest(incidents);
	const borderColor = worst ? `${TONE[worst]}.main` : undefined;

	// Active operator presence anywhere in the group tints the card, so
	// "someone is already on it" reads at a glance — especially useful
	// next to an incident border. Hover steps the tint up a notch instead
	// of disappearing into the base shade.
	const operators =
		result.status === "ok" ? aggregateOperators(result.data.members) : [];
	const occupied = operators.length > 0;

	return (
		<MuiLink
			component={RouterLink}
			to={`/fleet/groups/${groupId}`}
			underline="none"
			color="inherit"
		>
			<Card
				variant="outlined"
				sx={{
					// The bands run edge to edge, so the card clips them to its
					// own rounded corners.
					overflow: "hidden",
					transition: "background-color 150ms",
					bgcolor: occupied ? "action.hover" : undefined,
					"&:hover": {
						bgcolor: occupied ? "action.selected" : "action.hover",
					},
					...(borderColor && {
						borderColor,
						borderWidth: 2,
					}),
				}}
			>
				{result.status === "loading" || result.status === "idle" ? (
					<CardContent sx={{ p: 1.5, "&:last-child": { pb: 1.5 } }}>
						<Typography variant="body2" color="text.secondary">
							Thinking…
						</Typography>
					</CardContent>
				) : result.status === "error" ? (
					<CardContent sx={{ p: 1.5, "&:last-child": { pb: 1.5 } }}>
						<Alert severity="error">{result.error.message}</Alert>
					</CardContent>
				) : (
					<GroupCard
						group={result.data}
						operators={operators}
						incidents={incidents}
					/>
				)}
			</Card>
		</MuiLink>
	);
}

/// A group card, in three bands: what it is, what is in it, and what is
/// happening to it.
///
/// The status band is omitted when there is neither an operator nor an
/// incident, so a quiet card is two bands and the eye is drawn to the ones
/// that have a third.
/// spec: CHK#presentation
function GroupCard({
	group,
	operators,
	incidents,
}: {
	group: ServerGroupCard;
	operators: AggregatedOperator[];
	incidents: GroupIncidents | null;
}) {
	// The headline version comes from whichever member speaks for the group, so
	// how to present it follows from the types its members actually have: a
	// group with no versioned member shows no version rather than an "unknown".
	// spec: APP#versions
	const tracking = useVersionTrackingAcross(
		useMemo(() => group.members.map((m) => m.type), [group.members]),
	);
	const hasStatusBand = operators.length > 0 || loudest(incidents) !== null;
	return (
		<Box>
			<Box
				sx={{
					display: "flex",
					alignItems: "baseline",
					justifyContent: "space-between",
					gap: 1,
					px: "0.625em",
					py: "0.5em",
					borderBottom: 1,
					borderColor: "divider",
				}}
			>
				<Typography
					component="h3"
					sx={{
						fontSize: "0.9375em",
						fontWeight: 400,
						overflow: "hidden",
						textOverflow: "ellipsis",
						whiteSpace: "nowrap",
						minWidth: 0,
					}}
				>
					{group.name}
				</Typography>
				<Box sx={{ flexShrink: 0 }}>
					<VersionIndicator
						version={group.version}
						tracking={tracking}
						distance={group.version_distance}
						addLink={false}
					/>
				</Box>
			</Box>

			<RankedDotStrip members={group.members} incidents={incidents} />

			{hasStatusBand && (
				<Box
					data-testid="status-band"
					sx={{
						display: "flex",
						alignItems: "stretch",
						borderTop: 1,
						borderColor: "divider",
						fontSize: "0.6875em",
					}}
				>
					{operators.length > 0 && (
						<OperatorSegment operators={operators} />
					)}
					{/* Always present once the band is, empty or not: the
					    incident segment is what fills the band to the card's
					    edge, and a segment that came and went would leave the
					    operator count floating on some cards and not others. */}
					<IncidentSegment incidents={incidents} />
				</Box>
			)}
		</Box>
	);
}

/// The incident segment: right-aligned in whatever the operator segment leaves
/// it, and filled with the incident's own colour rather than holding a chip.
///
/// The segment is the mark. A chip inside a band would be a second border
/// inside a border, at a size that reads as a control rather than a state, and
/// the band's whole job is to be legible in a card 16em wide.
///
/// What it says is not how loud the incident is: a held one is not yet in
/// Slack, and a recovering one has had its failures clear and is waiting out
/// the linger window.
///
/// Its colour is the loudest of the group's environments, since the rank rows
/// above it say which is in trouble. The tooltip names every target, and is
/// where a group's own incident is told apart from an environment's.
/// spec: CHK#presentation
function IncidentSegment({ incidents }: { incidents: GroupIncidents | null }) {
	const loudness = loudest(incidents);
	const segment = (
		<Box
			data-testid="incident-segment"
			data-loudness={loudness ?? "none"}
			sx={{
				flex: 1,
				display: "flex",
				alignItems: "center",
				justifyContent: "flex-end",
				px: 1,
				py: 0.5,
				bgcolor: loudness ? `${TONE[loudness]}.main` : "action.hover",
				color: loudness ? "common.white" : "text.secondary",
			}}
		>
			{loudness ? LABEL[loudness] : ""}
		</Box>
	);
	if (!loudness) return segment;
	return (
		<Tooltip
			title={
				<Box>
					{incidentTargets(incidents).map(([rank, state]) => (
						<Box key={rank ?? "_group"}>
							{rank ?? "the group itself"}: {EXPLANATION[state]}
						</Box>
					))}
				</Box>
			}
		>
			{segment}
		</Tooltip>
	);
}

const TONE: Record<IncidentLoudness, "error" | "warning" | "info"> = {
	loud: "error",
	held: "warning",
	lingering: "info",
};

// One word each. "incident (recovering)" does not fit a 16em card, and the
// state is the word that matters — that there is an incident is said by the
// segment being coloured at all.
const LABEL: Record<IncidentLoudness, string> = {
	loud: "incident",
	held: "held",
	lingering: "recovering",
};

const EXPLANATION: Record<IncidentLoudness, string> = {
	loud: "Open incident",
	held: "Open incident; Slack notice is still inside the per-group cooldown window",
	lingering:
		"Open incident whose failures have all recovered; it closes if they stay quiet through the linger window",
};

/// "People are in here": person icon and the distinct operator count, tooltip
/// naming who is on which box.
///
/// Its own width, with a rule between it and the incident segment beside it.
function OperatorSegment({
	operators,
}: {
	operators: AggregatedOperator[];
}) {
	return (
		<Tooltip
			title={
				<Box>
					{operators.map(({ op, machines }) => (
						<Box key={op.login}>
							{op.login} · {machines.join(", ")}
						</Box>
					))}
				</Box>
			}
		>
			<Box
				data-testid="operator-segment"
				sx={{
					display: "flex",
					alignItems: "center",
					gap: 0.375,
					px: 1,
					py: 0.5,
					borderRight: 1,
					borderColor: "divider",
					bgcolor: "action.hover",
					color: "text.secondary",
					flexShrink: 0,
				}}
			>
				<PersonIcon sx={{ fontSize: "1.1em" }} />
				{operators.length}
			</Box>
		</Tooltip>
	);
}

/// A group's members as rank rows of machine enclosures.
///
/// Rank is the outer break, highest first, since that is how an operator reads
/// a group. Within a rank every machine is a pill holding the dots for the
/// applications on it: a one-application box is one dot in a pill, so the
/// presence of an enclosure never means anything on its own — only its
/// contents do. Two dots in one pill is the case the machine grain exists for.
/// spec: FLT
// Every dot sits in an identical fixed-size cell, so wrapped rows align to the
// same column grid wherever the line breaks fall. Spacing comes from the
// container's `gap`, not per-dot margins (StatusDot's inline right-margin is
// neutralised).
/// Lighter than the card's own border, so a rank break reads as subordinate to
/// the card structure. Taken from the text colour rather than from black, so it
/// survives a dark ground.
const dividerLight = (theme: Theme) => alpha(theme.palette.text.primary, 0.08);

/// The rank spelled out behind its own row, faint enough to read only when
/// looked for. Dark mode needs more of it to reach the same faintness, since it
/// is lifting off a dark card rather than sinking into a light one.
const rankLabel = (theme: Theme) =>
	alpha(theme.palette.text.primary, theme.palette.mode === "dark" ? 0.20 : 0.11);

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

/// The machines of one group, each with the applications on it, bucketed by
/// the rank of the highest-ranked application it carries.
function machineRows(members: FacilityServerStatus[]) {
	const byMachine = new Map<string, FacilityServerStatus[]>();
	for (const m of members) {
		const on = byMachine.get(m.machine_id);
		if (on) on.push(m);
		else byMachine.set(m.machine_id, [m]);
	}
	const boxes = [...byMachine.values()].map((on) => {
		const sorted = [...on].sort(compareServersByRankThenType);
		return { applications: sorted, lead: sorted[0]! };
	});
	return groupServersByRank(
		boxes.map((b) => ({
			...b,
			rank: b.lead.rank,
			type: b.lead.type,
			name: b.lead.machine_name ?? b.lead.name,
		})),
	);
}

export function RankedDotStrip({
	members,
	incidents,
}: {
	members: FacilityServerStatus[];
	incidents?: GroupIncidents | null;
}) {
	const theme = useTheme();
	const rows = machineRows(members);
	return (
		<Stack data-testid="dot-strip" spacing={0.5} sx={{ minWidth: 0 }}>
			{rows.map(([rank, boxes], index) => {
				// Only an environment's row takes a mark: a group's own incident
				// can come from a check with no machine behind it at all.
				const incident = rank ? (incidents?.get(rank) ?? null) : null;
				const tone = incident ? theme.palette[TONE[incident]].main : null;
				return (
					<Box
						key={rank ?? "_unranked"}
						data-testid="rank-row"
						data-rank={rank ?? "unranked"}
						data-incident={incident ?? undefined}
						sx={{
							position: "relative",
							display: "flex",
							flexWrap: "wrap",
							alignItems: "center",
							gap: "0.4em",
							px: "0.625em",
							py: "0.4375em",
							...(tone ? { bgcolor: alpha(tone, 0.16) } : {}),
							// Lighter than the card's own borders, so the rank break
							// reads as subordinate to the card structure. `divider`
							// is the card's border, so it cannot also be the rule
							// inside it.
							...(index > 0
								? { borderTop: 1, borderColor: dividerLight }
								: {}),
							// The rank named behind its own row rather than taking
							// space of its own.
							"&::after": {
								content: "attr(data-rank)",
								position: "absolute",
								right: "0.5em",
								top: "50%",
								transform: "translateY(-50%)",
								fontSize: "0.9375em",
								fontWeight: 500,
								letterSpacing: "0.06em",
								textTransform: "uppercase",
								color: tone ? alpha(tone, 0.55) : rankLabel,
								pointerEvents: "none",
								zIndex: 0,
							},
						}}
					>
						{boxes.map((box) => (
							<MachineEnclosure
								key={box.lead.machine_id}
								up={box.lead.machine_up}
								health={box.lead.machine_health}
								name={box.lead.machine_name}
								maintained={box.lead.machine_maintained}
								settling={box.lead.machine_maintenance_settling}
								describes={box.applications.map(
									(m) =>
										`${m.name}${m.rank ? ` · ${m.rank}` : ""} · ${m.type}`,
								)}
							>
								{box.applications.map((m) => (
									<Box key={m.id} component="span" sx={dotCellSx}>
										<StatusDot
											up={m.up}
											health={m.health}
											monitored={m.is_monitored}
											size={DOT_SIZE}
										/>
									</Box>
								))}
							</MachineEnclosure>
						))}
					</Box>
				);
			})}
		</Stack>
	);
}
