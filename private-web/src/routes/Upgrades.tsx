import AddIcon from "@mui/icons-material/Add";
import BuildOutlinedIcon from "@mui/icons-material/BuildOutlined";
import CalendarMonthIcon from "@mui/icons-material/CalendarMonth";
import ChevronLeftIcon from "@mui/icons-material/ChevronLeft";
import ChevronRightIcon from "@mui/icons-material/ChevronRight";
import DeleteIcon from "@mui/icons-material/Delete";
import EditIcon from "@mui/icons-material/Edit";
import ExpandLessIcon from "@mui/icons-material/ExpandLess";
import ExpandMoreIcon from "@mui/icons-material/ExpandMore";
import {
	Alert,
	Autocomplete,
	Box,
	Button,
	ButtonGroup,
	Chip,
	Collapse,
	createFilterOptions,
	Dialog,
	DialogActions,
	DialogContent,
	DialogTitle,
	IconButton,
	LinearProgress,
	MenuItem,
	Paper,
	Stack,
	Table,
	TableBody,
	TableCell,
	TableContainer,
	TableHead,
	TableRow,
	TextField,
	ToggleButton,
	ToggleButtonGroup,
	Tooltip,
	Typography,
} from "@mui/material";
import { alpha, type Theme } from "@mui/material/styles";
import {
	type ReactElement,
	type ReactNode,
	useEffect,
	useMemo,
	useRef,
	useState,
} from "react";
import { Link as RouterLink } from "react-router-dom";
import { useApi, useApiAction } from "../api";
import DeclareMaintenanceDialog from "../components/DeclareMaintenanceDialog";
import ServerRankChip from "../components/ServerRankChip";
import TimeAgo from "../components/TimeAgo";
import { useIsAdmin } from "../hooks/useIsAdmin";
import { usePageTitle } from "../hooks/usePageTitle";
import type { ApiResponse, ServerRank } from "../types";

type PastPlan = ApiResponse<"upgrade_plans", "history">[number];
type PlannableVersion = ApiResponse<"upgrade_plans", "targets">[number];

/// Where every group is going. A group with no plan is listed too: one
/// several minors behind with nothing recorded is what this view exists to
/// surface.
// spec: UPG#the-dashboard
export default function Upgrades() {
	usePageTitle("Upgrades");
	const isAdmin = useIsAdmin() === true;
	const [tick, setTick] = useState(0);
	const fleet = useApi("upgrade_plans", "fleet", {}, [tick]);
	const past = useApi("upgrade_plans", "history", {}, [tick]);

	if (fleet.status === "loading" || fleet.status === "idle") {
		return <LinearProgress />;
	}
	if (fleet.status === "error") {
		return <Alert severity="error">{fleet.error.message}</Alert>;
	}

	const planned = fleet.data.filter((row) => row.plan);
	// The gap this list is for: a group's headline environment, behind the
	// newest version, with nothing recorded.
	const unplanned = fleet.data.filter(
		(row) => !row.plan && row.headline && (row.behind ?? 0) > 0,
	);

	return (
		<Stack spacing={2}>
			<Stack direction="row" spacing={2} sx={{ alignItems: "center" }}>
				<Typography variant="h4" component="h1" sx={{ flexGrow: 1 }}>
					Upgrades
				</Typography>
				{isAdmin && (
					<RecordPlan
						environments={fleet.data.map((row) => ({
							groupId: row.group_id,
							groupName: row.group_name,
							rank: row.rank,
						}))}
						onRecorded={() => setTick((t) => t + 1)}
					/>
				)}
			</Stack>

			<PlanCalendar
				fleet={fleet.data}
				past={past.status === "ok" ? past.data : []}
				isAdmin={isAdmin}
				onAmended={() => setTick((t) => t + 1)}
			/>

			<Paper variant="outlined" sx={{ p: 2 }} data-testid="planned-upgrades">
				<Typography variant="h6" component="h2" gutterBottom>
					Planned
				</Typography>
				{planned.length === 0 ? (
					<Typography variant="body2" color="text.secondary">
						No group has a recorded plan.
					</Typography>
				) : (
					<TableContainer>
						<Table size="small" sx={TIGHT_TABLE}>
						<TableHead>
							<TableRow>
								<TableCell>Environment</TableCell>
								<TableCell>Running</TableCell>
								<TableCell>Going to</TableCell>
								<TableCell>Data survives it</TableCell>
								<TableCell>Planned for</TableCell>
								<TableCell>Window</TableCell>
								<TableCell>Note</TableCell>
								{isAdmin && <TableCell />}
							</TableRow>
						</TableHead>
						<TableBody>
							{planned.map((row) => (
								<TableRow
									key={`${row.group_id}:${row.rank}`}
									data-testid="planned-upgrade-row"
								>
										<TableCell>
											<EnvironmentName
												groupId={row.group_id}
												groupName={row.group_name}
												rank={row.rank}
											/>
										</TableCell>
										<TableCell>{row.current_version ?? "unknown"}</TableCell>
										<TableCell>{row.target_version}</TableCell>
										<TableCell>
											<Stack
												direction="row"
												spacing={0.5}
												sx={{ alignItems: "center" }}
											>
												<VerdictChip
													verdict={row.verdict}
													testable={row.testable}
												/>
												<AttemptChip attempt={row.attempt} />
											</Stack>
										</TableCell>
										<TableCell>
											<PlannedFor
												date={row.plan?.planned_for ?? null}
												late={row.late}
											/>
										</TableCell>
										<TableCell>
											<PlannedTime
												time={row.plan?.planned_time ?? null}
												end={row.plan?.planned_end_time ?? null}
												zone={row.plan?.planned_zone ?? null}
											/>
										</TableCell>
										<TableCell>
											<PlanNote
												note={row.plan?.note ?? null}
												testId="planned-upgrade-note"
											/>
										</TableCell>
										{isAdmin && (
											<TableCell align="right">
												<EditPlan
													planId={row.plan?.id ?? ""}
													groupName={environmentName(row.group_name, row.rank)}
													targetVersion={row.target_version ?? ""}
													plannedFor={row.plan?.planned_for ?? null}
													plannedTime={row.plan?.planned_time ?? null}
													plannedEnd={row.plan?.planned_end_time ?? null}
													plannedZone={row.plan?.planned_zone ?? null}
													note={row.plan?.note ?? null}
													onAmended={() => setTick((t) => t + 1)}
												/>
												<WithdrawPlan
													planId={row.plan?.id ?? ""}
													groupName={environmentName(row.group_name, row.rank)}
													targetVersion={row.target_version ?? ""}
													onWithdrawn={() => setTick((t) => t + 1)}
												/>
												<DeclareFromPlan
													groupId={row.group_id}
													rank={row.rank}
													groupName={environmentName(row.group_name, row.rank)}
													plannedTime={row.plan?.planned_time ?? null}
													plannedEnd={row.plan?.planned_end_time ?? null}
													note={row.plan?.note ?? null}
													onDeclared={() => setTick((t) => t + 1)}
												/>
											</TableCell>
										)}
								</TableRow>
							))}
						</TableBody>
					</Table>
				</TableContainer>
				)}
			</Paper>

			<Disclosure
				title="No plan recorded"
				subject="groups with no plan"
				caption={
					unplanned.length === 1
						? "1 group is behind with no plan, so it gets no pre-upgrade testing"
						: `${unplanned.length} groups are behind with no plan, so they get no pre-upgrade testing`
				}
				testId="unplanned-upgrades"
			>
					{unplanned.length === 0 ? (
						<Typography variant="body2" color="text.secondary">
							Every group that is behind has a plan.
						</Typography>
					) : (
						<TableContainer>
							<Table size="small" sx={TIGHT_TABLE}>
							<TableHead>
								<TableRow>
									<TableCell>Group</TableCell>
									<TableCell>Running</TableCell>
									<TableCell>Behind by</TableCell>
								</TableRow>
							</TableHead>
							<TableBody>
								{unplanned.map((row) => (
									<TableRow
										key={`${row.group_id}:${row.rank}`}
										data-testid="unplanned-upgrade-row"
									>
										<TableCell>
											<EnvironmentName
												groupId={row.group_id}
												groupName={row.group_name}
												rank={row.rank}
											/>
										</TableCell>
										<TableCell>{row.current_version ?? "unknown"}</TableCell>
										<TableCell>{behindLabel(row.behind ?? 0)}</TableCell>
									</TableRow>
								))}
							</TableBody>
						</Table>
					</TableContainer>
					)}
			</Disclosure>

			<PastPlans plans={past.status === "ok" ? past.data : []} />
		</Stack>
	);
}

type FleetRow = ApiResponse<"upgrade_plans", "fleet">[number];

/// The fleet's distance is majors times a thousand plus minors; a major behind
/// is far enough that the minors no longer matter.
function behindLabel(behind: number): string {
	const majors = Math.floor(behind / 1000);
	const minors = behind % 1000;
	if (majors > 0) return `${majors} major${majors === 1 ? "" : "s"}`;
	return `${minors} minor${minors === 1 ? "" : "s"}`;
}

/// How an environment is named where it is read: the group, with the rank after
/// it unless it is the group's production.
function environmentName(group: string, rank: ServerRank): string {
	return rank === "production" ? group : `${group} ${rank}`;
}

function EnvironmentName({
	groupId,
	groupName,
	rank,
}: {
	groupId: string;
	groupName: string;
	rank: ServerRank;
}) {
	return (
		<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
			<RouterLink to={`/fleet/groups/${groupId}`}>{groupName}</RouterLink>
			{rank !== "production" && <ServerRankChip rank={rank} />}
		</Stack>
	);
}

type View = "month" | "week" | "day";

const VIEWS: View[] = ["month", "week", "day"];

/// Part of a window as one column draws it. A plan running past midnight is two
/// of these: the rest of its night, and the following morning.
type Segment = {
	entry: Entry;
	date: string;
	from: number;
	to: number;
	tail: boolean;
};

type Block = Segment & { lane: number; lanes: number };

/// How an entry reads at a glance: still ahead, already met, or past the day it
/// named.
type Tone = "open" | "late" | "done";

type Entry = {
	planId: string;
	date: string;
	groupId: string;
	group: string;
	version: string;
	time: string | null;
	end: string | null;
	zone: string | null;
	note: string | null;
	tone: Tone;
};

const WEEKDAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// Green is canopy's own, so a plan still ahead wears it. A met plan recedes to
/// grey: it is history, and the month is read for what has yet to happen.
function toneColour(theme: Theme, tone: Tone): string {
	switch (tone) {
		case "done":
			return theme.palette.text.secondary;
		case "late":
			return theme.palette.warning.main;
		default:
			return theme.palette.primary.main;
	}
}

/// Beyond this a day's entries would crowd the week out of shape, so the rest
/// sit behind a count that names them.
const ENTRIES_PER_DAY = 3;

/// Which month, week, or day a group moves in. The table below answers
/// what each plan says; this answers which week is busy, which two groups
/// land on the same night, and how long each one is expected to take.
// spec: UPG#the-dashboard
function PlanCalendar({
	fleet,
	past,
	isAdmin,
	onAmended,
}: {
	fleet: FleetRow[];
	past: PastPlan[];
	isAdmin: boolean;
	onAmended: () => void;
}) {
	const [view, setView] = useState<View>("month");
	const [cursor, setCursor] = useState(startOfToday);
	const [editing, setEditing] = useState<Entry | null>(null);
	const [copied, setCopied] = useState(false);
	const feed = useApi("commons", "calendar_url");
	const feedUrl = feed.status === "ok" ? feed.data : null;
	const today = localDate(new Date());

	const entries = useMemo(() => {
		const all: Entry[] = [];
		for (const row of fleet) {
			if (!row.plan?.planned_for) continue;
			all.push({
				planId: row.plan.id,
				date: row.plan.planned_for,
				groupId: row.group_id,
				group: environmentName(row.group_name, row.rank),
				version: row.target_version ?? "",
				time: row.plan.planned_time,
				end: row.plan.planned_end_time,
				zone: row.plan.planned_zone,
				note: row.plan.note,
				tone: row.late ? "late" : "open",
			});
		}
		// Met plans stay on the calendar: what landed is half of what a month
		// of upgrades looked like.
		for (const row of past) {
			if (row.outcome !== "met" || !row.plan.planned_for) continue;
			all.push({
				planId: row.plan.id,
				date: row.plan.planned_for,
				groupId: row.group_id,
				group: environmentName(row.group_name, row.plan.rank),
				version: row.target_version,
				time: row.plan.planned_time,
				end: row.plan.planned_end_time,
				zone: row.plan.planned_zone,
				note: row.plan.note,
				tone: "done",
			});
		}
		return all.sort(
			(a, b) =>
				(a.time ?? "").localeCompare(b.time ?? "") ||
				a.group.localeCompare(b.group),
		);
	}, [fleet, past]);

	const days = useMemo(() => visibleDays(cursor, view), [cursor, view]);
	const month = localDate(cursor).slice(0, 7);
	const shown = entries.filter((entry) =>
		view === "month" ? entry.date.startsWith(month) : days.includes(entry.date),
	).length;

	// A met plan is history and no longer amendable, so it keeps the link out
	// to the group instead.
	const editor = (entry: Entry) =>
		isAdmin && entry.tone !== "done" ? () => setEditing(entry) : null;

	return (
		<Paper variant="outlined" sx={{ p: 2 }} data-testid="upgrade-calendar">
			<Stack direction="row" spacing={1} sx={{ alignItems: "center", mb: 1.5 }}>
				<Box sx={{ flexGrow: 1, minWidth: 0 }}>
					<Typography variant="h6" component="h2" sx={{ lineHeight: 1.25 }}>
						{periodName(cursor, view)}{" "}
						<Box
							component="span"
							sx={{ color: "text.secondary", fontWeight: 400 }}
						>
							{periodYear(cursor, view)}
						</Box>
					</Typography>
					<Typography variant="caption" color="text.secondary">
						{countLabel(shown, view)}
					</Typography>
				</Box>
				{feedUrl && (
					<Tooltip title="Copy the feed URL to subscribe a calendar">
						<Button
							size="small"
							sx={{ flexShrink: 0 }}
							startIcon={<CalendarMonthIcon />}
							onClick={async () => {
								await navigator.clipboard.writeText(feedUrl);
								setCopied(true);
								setTimeout(() => setCopied(false), 2000);
							}}
						>
							{copied ? "Copied" : "Subscribe"}
						</Button>
					</Tooltip>
				)}
				<ToggleButtonGroup
					size="small"
					exclusive
					value={view}
					onChange={(_, next: View | null) => next && setView(next)}
					sx={{ flexShrink: 0 }}
				>
					{VIEWS.map((option) => (
						<ToggleButton key={option} value={option} sx={VIEW_BUTTON}>
							{option}
						</ToggleButton>
					))}
				</ToggleButtonGroup>
				<ButtonGroup size="small" variant="outlined" sx={{ flexShrink: 0 }}>
					<Button
						aria-label={`previous ${view}`}
						onClick={() => setCursor((at) => stepped(at, view, -1))}
					>
						<ChevronLeftIcon fontSize="small" />
					</Button>
					<Button onClick={() => setCursor(startOfToday())}>Today</Button>
					<Button
						aria-label={`next ${view}`}
						onClick={() => setCursor((at) => stepped(at, view, 1))}
					>
						<ChevronRightIcon fontSize="small" />
					</Button>
				</ButtonGroup>
			</Stack>

			{view === "month" ? (
				<MonthGrid
					days={days}
					month={month}
					today={today}
					entries={entries}
					editor={editor}
					onOpenDay={(date) => {
						setCursor(dateOf(date));
						setView("day");
					}}
				/>
			) : (
				<TimeGrid
					days={days}
					today={today}
					entries={entries}
					editor={editor}
				/>
			)}

			{editing && (
				<EditPlanDialog
					planId={editing.planId}
					groupName={editing.group}
					targetVersion={editing.version}
					plannedFor={editing.date}
					plannedTime={editing.time}
					plannedEnd={editing.end}
					plannedZone={editing.zone}
					note={editing.note}
					onClose={() => setEditing(null)}
					onAmended={() => {
						setEditing(null);
						onAmended();
					}}
				/>
			)}
		</Paper>
	);
}

/// The month as anyone reads a month: whole weeks, with the neighbouring
/// months' edges drawn faded rather than left blank, so a plan on the 1st or
/// the 31st is never off the edge of the view.
function MonthGrid({
	days,
	month,
	today,
	entries,
	editor,
	onOpenDay,
}: {
	days: string[];
	month: string;
	today: string;
	entries: Entry[];
	editor: (entry: Entry) => (() => void) | null;
	onOpenDay: (date: string) => void;
}) {
	return (
		<Box sx={CALENDAR_GRID}>
			{WEEKDAYS.map((day) => (
				<Typography key={day} variant="caption" sx={WEEKDAY_CELL}>
					{day}
				</Typography>
			))}
			{days.map((date) => {
				const inMonth = date.startsWith(month);
				const onDay = entries.filter((entry) => entry.date === date);
				const overflow = onDay.slice(ENTRIES_PER_DAY);
				return (
					<Box
						key={date}
						data-testid="calendar-day"
						data-date={date}
						sx={(theme) => ({
							...CALENDAR_CELL,
							// Opaque: the 1px gridlines are the container's own
							// background showing through the gaps, so a tinted cell
							// would let them flood it.
							bgcolor: inMonth
								? "background.paper"
								: theme.palette.mode === "dark"
									? "#0d0d0d"
									: theme.palette.grey[50],
						})}
					>
						<Box sx={{ display: "flex", justifyContent: "flex-end" }}>
							<Box
								component="button"
								type="button"
								onClick={() => onOpenDay(date)}
								sx={[
									{
										...DAY_NUMBER,
										...DAY_BUTTON,
										color: inMonth ? "text.secondary" : "text.disabled",
									},
									date === today && TODAY_NUMBER,
								]}
							>
								{Number(date.slice(8))}
							</Box>
						</Box>
						{onDay.slice(0, ENTRIES_PER_DAY).map((entry) => (
							<CalendarEntry
								key={entry.planId}
								entry={entry}
								onEdit={editor(entry)}
							/>
						))}
						{overflow.length > 0 && (
							<CalendarTooltip
								title={overflow
									.map((entry) => `${entry.group} ${entry.version}`)
									.join(", ")}
							>
								<Typography variant="caption" sx={OVERFLOW_COUNT}>
									+{overflow.length} more
								</Typography>
							</CalendarTooltip>
						)}
					</Box>
				);
			})}
		</Box>
	);
}

/// The hours of a day, or of a week. The length of a block is how long the
/// group expects to be down, and a window running past midnight is drawn
/// again on the following morning.
function TimeGrid({
	days,
	today,
	entries,
	editor,
}: {
	days: string[];
	today: string;
	entries: Entry[];
	editor: (entry: Entry) => (() => void) | null;
}) {
	const hours = useRef<HTMLDivElement>(null);
	const now = useMinute();
	const blocks = useMemo(() => laidOut(days, entries), [days, entries]);
	const resting = restingHour(blocks);

	// Upgrades run at night, which is off the bottom of the scroller until
	// something puts it in view.
	useEffect(() => {
		hours.current?.scrollTo({ top: GRID_SLACK + resting * HOUR_HEIGHT });
	}, [resting]);

	const columns = `${GUTTER}px repeat(${days.length}, minmax(0, 1fr))`;
	const marked = (date: string) =>
		days.length > 1 && date === today && TODAY_COLUMN;

	return (
		<Box sx={TIME_FRAME}>
			<Box
				sx={{
					display: "grid",
					gridTemplateColumns: columns,
					borderBottom: 1,
					borderColor: "divider",
				}}
			>
				<Box />
				{days.map((date) => (
					<Box
						key={date}
						sx={[COLUMN_HEAD, marked(date)]}
					>
						<Box component="span" sx={WEEKDAY_LABEL}>
							{weekdayOf(date)}
						</Box>
						<Box
							component="span"
							sx={[DAY_NUMBER, HEAD_NUMBER, date === today && TODAY_NUMBER]}
						>
							{Number(date.slice(8))}
						</Box>
					</Box>
				))}
			</Box>

			<Box
				ref={hours}
				sx={{
					maxHeight: VISIBLE_HOURS * HOUR_HEIGHT + 2 * GRID_SLACK,
					overflowY: "auto",
					py: `${GRID_SLACK}px`,
				}}
			>
				<Box
					sx={{
						display: "grid",
						gridTemplateColumns: columns,
						height: 24 * HOUR_HEIGHT,
					}}
				>
					<Box sx={{ position: "relative" }}>
						{[...HOURS, 24].map((hour) => (
							<Box key={hour} sx={{ ...HOUR_LABEL, top: hour * HOUR_HEIGHT }}>
								{clockTime(`${String(hour % 24).padStart(2, "0")}:00`)}
							</Box>
						))}
					</Box>
					{days.map((date) => (
						<Box
							key={date}
							data-testid="calendar-day"
							data-date={date}
							sx={[HOUR_COLUMN, marked(date)]}
						>
							{date === localDate(now) && (
								<Box
									sx={{
										...NOW_LINE,
										top: `${((now.getHours() * 60 + now.getMinutes()) / DAY_MINUTES) * 100}%`,
									}}
								/>
							)}
							{blocks
								.filter((block) => block.date === date)
								.map((block) => (
									<TimeBlock
										key={`${block.entry.planId}${block.tail ? "-tail" : ""}`}
										block={block}
										onEdit={editor(block.entry)}
									/>
								))}
						</Box>
					))}
				</Box>
			</Box>
		</Box>
	);
}

function CalendarEntry({
	entry,
	onEdit,
}: {
	entry: Entry;
	onEdit: (() => void) | null;
}) {
	return (
		<CalendarTooltip title={<EntryTooltip entry={entry} />}>
			<Box
				{...(onEdit
					? { component: "button" as const, type: "button", onClick: onEdit }
					: { component: RouterLink, to: `/fleet/groups/${entry.groupId}` })}
				data-testid="calendar-entry"
				sx={[
					(theme) => ({
						...CALENDAR_ENTRY,
						bgcolor: alpha(toneColour(theme, entry.tone), 0.14),
						"&:hover": { bgcolor: alpha(toneColour(theme, entry.tone), 0.26) },
					}),
					!!onEdit && ENTRY_BUTTON,
				]}
			>
				<Box sx={ENTRY_LABEL}>
					{entry.time && (
						<>
							<Box component="span" sx={{ color: "text.secondary" }}>
								{clockTime(entry.time)}
							</Box>{" "}
						</>
					)}
					<Box component="span" sx={{ fontWeight: 500 }}>
						{entry.group}
					</Box>{" "}
					<Box component="span" sx={{ color: "text.secondary" }}>
						{entry.version}
					</Box>
				</Box>
			</Box>
		</CalendarTooltip>
	);
}

/// One window on the hour grid, sized to how long it runs.
function TimeBlock({
	block,
	onEdit,
}: {
	block: Block;
	onEdit: (() => void) | null;
}) {
	const { entry } = block;

	return (
		<CalendarTooltip title={<EntryTooltip entry={entry} />}>
			<Box
				{...(onEdit
					? { component: "button" as const, type: "button", onClick: onEdit }
					: { component: RouterLink, to: `/fleet/groups/${entry.groupId}` })}
				data-testid="calendar-entry"
				{...(block.tail ? { "data-continues": "true" } : null)}
				sx={[
					(theme) => ({
						...TIME_BLOCK,
						top: `${(block.from / DAY_MINUTES) * 100}%`,
						height: `${((block.to - block.from) / DAY_MINUTES) * 100}%`,
						left: `${(block.lane / block.lanes) * 100}%`,
						width: `${100 / block.lanes}%`,
						bgcolor: alpha(toneColour(theme, entry.tone), 0.16),
						borderColor: alpha(toneColour(theme, entry.tone), 0.45),
						"&:hover": { bgcolor: alpha(toneColour(theme, entry.tone), 0.28) },
					}),
					!!onEdit && BLOCK_BUTTON,
				]}
			>
				<Box sx={ENTRY_LABEL}>
					<Box component="span" sx={{ fontWeight: 500 }}>
						{entry.group}
					</Box>{" "}
					<Box component="span" sx={{ color: "text.secondary" }}>
						{entry.version}
					</Box>
				</Box>
				<Box sx={BLOCK_HOURS}>
					{block.tail
						? `until ${clockTime(wallClock(block.to))}`
						: entry.time && clockRange(entry.time, entry.end)}
				</Box>
			</Box>
		</CalendarTooltip>
	);
}

/// Hover card for the calendar: prefers sitting above its entry (flipping when
/// there's no room), never captures the pointer, and hides once its entry
/// scrolls out of the grid's clip.
function CalendarTooltip({
	title,
	children,
}: {
	title: ReactNode;
	children: ReactElement;
}) {
	return (
		<Tooltip
			title={title}
			placement="top"
			disableInteractive
			slotProps={CALENDAR_TOOLTIP_SLOTS}
		>
			{children}
		</Tooltip>
	);
}

const CALENDAR_TOOLTIP_SLOTS = {
	popper: {
		sx: {
			'&[data-popper-reference-hidden]': {
				visibility: "hidden",
				pointerEvents: "none",
			},
		},
	},
} as const;

function EntryTooltip({ entry }: { entry: Entry }) {
	return (
		<>
			{entry.tone === "done"
				? `${entry.group} reached ${entry.version}`
				: `${entry.group} to ${entry.version}${entry.tone === "late" ? " (late)" : ""}`}
			{entry.time && entry.zone && (
				<Box sx={ENTRY_NOTE}>
					{clockRange(entry.time, entry.end)} {zoneLabel(entry.zone)} (
					{entry.zone})
				</Box>
			)}
			{entry.note && <Box sx={ENTRY_NOTE}>{entry.note}</Box>}
		</>
	);
}

/// A timed plan as the blocks an hour grid draws, laid out so that windows
/// overlapping in the same column share its width rather than covering each
/// other.
function laidOut(days: string[], entries: Entry[]): Block[] {
	const columns = new Map<string, Segment[]>();
	for (const entry of entries) {
		if (!entry.time) continue;
		const from = minutesInto(entry.time);
		const runs = windowMinutes(entry.time, entry.end);
		const spans: Segment[] = [
			{ entry, date: entry.date, from, to: Math.min(from + runs, DAY_MINUTES), tail: false },
		];
		if (from + runs > DAY_MINUTES) {
			spans.push({
				entry,
				date: nextDay(entry.date),
				from: 0,
				to: from + runs - DAY_MINUTES,
				tail: true,
			});
		}
		for (const span of spans.filter((span) => days.includes(span.date))) {
			const column = columns.get(span.date);
			if (column) column.push(span);
			else columns.set(span.date, [span]);
		}
	}

	const out: Block[] = [];
	for (const column of columns.values()) {
		const freeFrom: number[] = [];
		const placed = column
			.sort((a, b) => a.from - b.from || a.to - b.to)
			.map((span) => {
				const found = freeFrom.findIndex((at) => at <= span.from);
				const lane = found < 0 ? freeFrom.length : found;
				freeFrom[lane] = span.to;
				return { ...span, lane };
			});
		out.push(...placed.map((span) => ({ ...span, lanes: freeFrom.length })));
	}
	return out;
}

/// A clock that ticks once a minute, for the line marking now. Paused while the
/// tab is hidden, since nobody is reading it.
function useMinute(): Date {
	const [now, setNow] = useState(() => new Date());
	useEffect(() => {
		const id = window.setInterval(() => {
			if (!document.hidden) setNow(new Date());
		}, 60_000);
		return () => window.clearInterval(id);
	}, []);
	return now;
}

/// Where the grid rests: the hour that brings the most windows into view.
/// Upgrades cluster at night, so opening at the earliest of them would let one
/// morning entry pin the view twelve hours away from the rest.
function restingHour(blocks: Block[]): number {
	const latest = 24 - VISIBLE_HOURS;
	if (!blocks.length) return Math.min(MORNING / 60, latest);

	const candidates = blocks
		.map((block) =>
			Math.min(Math.max(0, Math.floor(block.from / 60) - 1), latest),
		)
		.sort((a, b) => a - b);
	let resting = candidates[0];
	let most = -1;
	for (const hour of candidates) {
		const shown = blocks.filter(
			(block) =>
				block.from >= hour * 60 && block.to <= (hour + VISIBLE_HOURS) * 60,
		).length;
		if (shown > most) {
			most = shown;
			resting = hour;
		}
	}
	return resting;
}

/// The days a view draws.
function visibleDays(cursor: Date, view: View): string[] {
	if (view === "day") return [localDate(cursor)];
	const at = weekStart(view === "week" ? cursor : monthStart(cursor));
	const wanted = view === "week" ? 7 : monthRows(cursor) * 7;
	const days: string[] = [];
	while (days.length < wanted) {
		days.push(localDate(at));
		at.setDate(at.getDate() + 1);
	}
	return days;
}

/// Six rows only for a month that starts late enough to need one.
function monthRows(cursor: Date): number {
	const first = monthStart(cursor);
	const length = new Date(
		cursor.getFullYear(),
		cursor.getMonth() + 1,
		0,
	).getDate();
	return Math.ceil((((first.getDay() + 6) % 7) + length) / 7);
}

function stepped(cursor: Date, view: View, delta: number): Date {
	const at = new Date(cursor);
	if (view === "month") {
		at.setDate(1);
		at.setMonth(at.getMonth() + delta);
	} else {
		at.setDate(at.getDate() + delta * (view === "week" ? 7 : 1));
	}
	return at;
}

function periodName(cursor: Date, view: View): string {
	if (view === "month") {
		return cursor.toLocaleDateString(undefined, { month: "long" });
	}
	if (view === "day") {
		return cursor.toLocaleDateString(undefined, {
			weekday: "long",
			day: "numeric",
			month: "long",
		});
	}
	const monday = weekStart(cursor);
	const sunday = new Date(monday);
	sunday.setDate(monday.getDate() + 6);
	const shortMonth = (at: Date) =>
		at.toLocaleDateString(undefined, { month: "short" });
	return monday.getMonth() === sunday.getMonth()
		? `${monday.getDate()}-${sunday.getDate()} ${shortMonth(sunday)}`
		: `${monday.getDate()} ${shortMonth(monday)}-${sunday.getDate()} ${shortMonth(sunday)}`;
}

function periodYear(cursor: Date, view: View): number {
	return (view === "week" ? weekStart(cursor) : cursor).getFullYear();
}

function countLabel(count: number, view: View): string {
	const span = view === "day" ? "on this day" : `this ${view}`;
	if (count === 0) return `nothing planned ${span}`;
	return `${count} upgrade${count === 1 ? "" : "s"} ${span}`;
}

function weekdayOf(date: string): string {
	return dateOf(date).toLocaleDateString(undefined, { weekday: "short" });
}

function monthStart(cursor: Date): Date {
	return new Date(cursor.getFullYear(), cursor.getMonth(), 1);
}

function weekStart(at: Date): Date {
	const monday = new Date(at);
	monday.setDate(at.getDate() - ((at.getDay() + 6) % 7));
	return monday;
}

function startOfToday(): Date {
	const now = new Date();
	return new Date(now.getFullYear(), now.getMonth(), now.getDate());
}

function localDate(at: Date): string {
	return [
		at.getFullYear(),
		String(at.getMonth() + 1).padStart(2, "0"),
		String(at.getDate()).padStart(2, "0"),
	].join("-");
}

function dateOf(date: string): Date {
	const [year, month, day] = date.split("-").map(Number);
	return new Date(year, month - 1, day);
}

function nextDay(date: string): string {
	const at = dateOf(date);
	at.setDate(at.getDate() + 1);
	return localDate(at);
}

const CALENDAR_GRID = {
	display: "grid",
	gridTemplateColumns: "repeat(7, minmax(0, 1fr))",
	gap: "1px",
	bgcolor: "divider",
	border: 1,
	borderColor: "divider",
	borderRadius: 1,
	overflow: "hidden",
};

const WEEKDAY_LABEL = {
	color: "text.secondary",
	fontSize: "0.85rem",
};

const WEEKDAY_CELL = {
	...WEEKDAY_LABEL,
	bgcolor: "background.paper",
	textAlign: "center",
	py: 0.75,
};

const CALENDAR_CELL = {
	minHeight: 80,
	p: 0.5,
	display: "flex",
	flexDirection: "column",
	gap: 0.25,
	minWidth: 0,
};

const DAY_NUMBER = {
	width: 26,
	height: 26,
	color: "text.secondary",
	display: "grid",
	placeItems: "center",
	fontSize: "0.85rem",
	lineHeight: 1,
	fontVariantNumeric: "proportional-nums",
	pt: "2px",
};

const HEAD_NUMBER = {
	width: "auto",
	minWidth: 0,
};

const TODAY_NUMBER = {
	width: 28,
	height: 28,
	bgcolor: "primary.dark",
	color: "common.white",
	borderRadius: "50%",
};

const CALENDAR_ENTRY = {
	display: "flex",
	alignItems: "center",
	px: 0.75,
	py: "2px",
	borderRadius: 0.75,
	minWidth: 0,
	textDecoration: "none",
	color: "text.primary",
	fontSize: "0.68rem",
	lineHeight: 1.6,
};

/// A button element brings its own font and chrome, which the pill has to undo
/// to sit level with the ones that are still links.
const ENTRY_BUTTON = {
	border: 0,
	width: "100%",
	font: "inherit",
	fontSize: "0.68rem",
	textAlign: "left",
	cursor: "pointer",
};

const ENTRY_NOTE = {
	mt: 0.5,
	opacity: 0.8,
};

const ENTRY_LABEL = {
	minWidth: 0,
	overflow: "hidden",
	textOverflow: "ellipsis",
	whiteSpace: "nowrap",
};

const HOUR_HEIGHT = 40;

const VISIBLE_HOURS = 11;

const GRID_SLACK = HOUR_HEIGHT / 2;

const GUTTER = 58;

const HOURS = [...Array(24).keys()];

const MORNING = 8 * 60;

const VIEW_BUTTON = {
	px: 1.25,
	py: 0.25,
	textTransform: "capitalize",
	fontSize: "0.75rem",
};

const DAY_BUTTON = {
	border: 0,
	bgcolor: "transparent",
	fontFamily: "inherit",
	cursor: "pointer",
	px: 0,
	pb: 0,
};

const TIME_FRAME = {
	border: 1,
	borderColor: "divider",
	borderRadius: 1,
	overflow: "hidden",
};

const COLUMN_HEAD = {
	display: "flex",
	alignItems: "baseline",
	justifyContent: "center",
	gap: 0.5,
	py: 0.5,
	borderLeft: 1,
	borderColor: "divider",
};

const HOUR_LABEL = {
	position: "absolute",
	right: 0,
	transform: "translateY(-50%)",
	pr: 0.75,
	color: "text.secondary",
	fontSize: "0.7rem",
	lineHeight: 1.2,
};

const HOUR_COLUMN = (theme: Theme) => ({
	position: "relative",
	borderLeft: 1,
	borderBottom: 1,
	borderColor: "divider",
	minWidth: 0,
	backgroundImage: `repeating-linear-gradient(to bottom, ${theme.palette.divider} 0 1px, transparent 1px ${HOUR_HEIGHT}px)`,
});

const TIME_BLOCK = {
	position: "absolute",
	minHeight: 20,
	overflow: "hidden",
	px: 0.75,
	py: "1px",
	border: 1,
	borderRadius: 0.75,
	textDecoration: "none",
	color: "text.primary",
	fontSize: "0.75rem",
	lineHeight: 1.45,
};

const BLOCK_BUTTON = {
	display: "block",
	fontFamily: "inherit",
	textAlign: "left",
	cursor: "pointer",
};

const TODAY_COLUMN = { bgcolor: "action.hover" };

const NOW_LINE = {
	position: "absolute",
	left: 0,
	right: 0,
	borderTop: 2,
	borderColor: "text.primary",
	pointerEvents: "none",
	zIndex: 1,
	"&::before": {
		content: '""',
		position: "absolute",
		left: 0,
		top: -4,
		width: 6,
		height: 6,
		borderRadius: "50%",
		bgcolor: "text.primary",
	},
};

const BLOCK_HOURS = {
	...ENTRY_LABEL,
	color: "text.secondary",
	fontSize: "0.68rem",
};

const OVERFLOW_COUNT = {
	color: "text.secondary",
	fontSize: "0.65rem",
	pl: 0.5,
	cursor: "default",
};

/// What each group planned before, and how it ended. A withdrawn plan is
/// readable here or nowhere: a group that stopped going somewhere leaves
/// no other mark on the fleet.
// spec: UPG#the-dashboard
function PastPlans({ plans }: { plans: PastPlan[] }) {
	if (plans.length === 0) return null;

	return (
		<Disclosure
			title="Past plans"
			subject="past plans"
			caption="where each group was going before, and how it ended"
			testId="past-plans"
		>
			<TableContainer>
				<Table size="small" sx={TIGHT_TABLE}>
				<TableHead>
					<TableRow>
						<TableCell>Environment</TableCell>
						<TableCell>Was going to</TableCell>
						<TableCell>Planned for</TableCell>
						<TableCell>Window</TableCell>
						<TableCell>Ended</TableCell>
						<TableCell>Note</TableCell>
					</TableRow>
				</TableHead>
				<TableBody>
					{plans.map((row) => (
						<TableRow key={row.plan.id} data-testid="past-plan-row">
								<TableCell>
									<EnvironmentName
										groupId={row.group_id}
										groupName={row.group_name}
										rank={row.plan.rank}
									/>
								</TableCell>
								<TableCell>{row.target_version}</TableCell>
								<TableCell>{row.plan.planned_for ?? ""}</TableCell>
								<TableCell>
									<PlannedTime
										time={row.plan.planned_time ?? null}
										end={row.plan.planned_end_time ?? null}
										zone={row.plan.planned_zone ?? null}
									/>
								</TableCell>
								<TableCell>
									<Stack
										direction="row"
										spacing={0.5}
										sx={{ alignItems: "center" }}
									>
										<OutcomeChip
											outcome={row.outcome}
											withdrawnBy={row.plan.withdrawn_by}
										/>
										<Typography variant="body2" color="text.secondary">
											<TimeAgo timestamp={row.ended_at} />
										</Typography>
									</Stack>
								</TableCell>
								<TableCell>
									<PlanNote
										note={row.plan.note}
										testId="past-plan-note"
									/>
								</TableCell>
							</TableRow>
					))}
				</TableBody>
			</Table>
		</TableContainer>
		</Disclosure>
	);
}

/// How a plan ended. Withdrawn reads differently from met: the group
/// stopped going there rather than arriving.
function OutcomeChip({
	outcome,
	withdrawnBy,
}: {
	outcome: PastPlan["outcome"];
	withdrawnBy: string | null;
}) {
	if (outcome === "met") {
		return <Chip size="small" color="success" label="met" />;
	}
	if (outcome === "withdrawn") {
		return (
			<Tooltip
				title={
					withdrawnBy
						? `withdrawn by ${withdrawnBy}; the upgrade did not happen`
						: "the group stopped going there; the upgrade did not happen"
				}
			>
				<Chip size="small" color="warning" variant="outlined" label="withdrawn" />
			</Tooltip>
		);
	}
	return (
		<Tooltip title="a later plan took its place">
			<Chip size="small" variant="outlined" label="replaced" />
		</Tooltip>
	);
}

/// Whether the environment's own data survives the planned version, rolled up
/// from its applications. Pairing it with the plan is the point of this view.
function VerdictChip({
	verdict,
	testable,
}: {
	verdict: string | null | undefined;
	testable: boolean | null | undefined;
}) {
	if (verdict === "passed") {
		return <Chip size="small" color="success" label="passed" />;
	}
	if (verdict === "failed") {
		return (
			<Tooltip title="an application's data broke the migrations; the version is held back">
				<Chip size="small" color="warning" label="failed" />
			</Tooltip>
		);
	}
	if (testable === false) {
		return (
			<Tooltip title="nothing is declared to migrate this group's data, so no test will run: declare a restore replica for it on the group's page">
				<Chip
					size="small"
					color="warning"
					variant="outlined"
					label="not set up"
				/>
			</Tooltip>
		);
	}
	return <Chip size="small" variant="outlined" label="not yet tested" />;
}

/// An attempt under way, beside the verdict rather than replacing it: a row can
/// read as failed with a fresh attempt already running.
function AttemptChip({
	attempt,
}: {
	attempt: "in_flight" | "ended_without_report" | null | undefined;
}) {
	if (attempt === "in_flight") {
		return (
			<Tooltip title="a restore is under way; a verdict lands when it reports">
				<Chip size="small" color="info" variant="outlined" label="testing" />
			</Tooltip>
		);
	}
	if (attempt === "ended_without_report") {
		return (
			<Tooltip title="a restore ran and never reported how it went, so the pipeline may be stuck">
				<Chip
					size="small"
					color="warning"
					variant="outlined"
					label="no report"
				/>
			</Tooltip>
		);
	}
	return null;
}

function PlannedFor({ date, late }: { date: string | null; late: boolean }) {
	if (!date) {
		return (
			<Typography variant="body2" color="text.secondary">
				no date
			</Typography>
		);
	}
	if (!late) {
		return <>{date}</>;
	}
	return (
		<Tooltip title="the planned day has passed and the group has not moved">
			<Chip size="small" color="warning" variant="outlined" label={`${date} (late)`} />
		</Tooltip>
	);
}

/// A section the operator opens when they want it. Both of these answer a
/// question that is worth having on the page and not worth reading every time.
function Disclosure({
	title,
	subject,
	caption,
	testId,
	children,
}: {
	title: string;
	subject: string;
	caption: string;
	testId: string;
	children: React.ReactNode;
}) {
	const [open, setOpen] = useState(false);
	return (
		<Paper variant="outlined" sx={{ p: 2 }} data-testid={testId}>
			<Stack
				direction="row"
				spacing={1}
				sx={{ alignItems: "baseline", cursor: "pointer" }}
				onClick={() => setOpen((shown) => !shown)}
			>
				<IconButton
					size="small"
					aria-label={`${open ? "Hide" : "Show"} ${subject}`}
					aria-expanded={open}
				>
					{open ? (
						<ExpandLessIcon fontSize="small" />
					) : (
						<ExpandMoreIcon fontSize="small" />
					)}
				</IconButton>
				<Typography variant="h6" component="h2">
					{title}
				</Typography>
				<Typography variant="body2" color="text.secondary">
					{caption}
				</Typography>
			</Stack>
			<Collapse in={open}>{children}</Collapse>
		</Paper>
	);
}

/// What an operator needed the next reader to know, under the row it belongs
/// to. Held to one line so a long note cannot set the row height for every
/// other group; the whole of it is on hover.
function PlanNote({ note, testId }: { note: string | null; testId: string }) {
	if (!note) return null;
	return (
		<Tooltip title={note}>
			<Typography variant="body2" noWrap sx={NOTE_TEXT} data-testid={testId}>
				{note}
			</Typography>
		</Tooltip>
	);
}

/// The window a group moves in, as the wall clocks it was recorded as.
/// Canopy holds no timezone for a group, so the zone travels with the time or
/// the reader cannot tell whose midnight it is.
function PlannedTime({
	time,
	end,
	zone,
}: {
	time: string | null;
	end: string | null;
	zone: string | null;
}) {
	if (!time || !zone) return null;
	const offset = zoneOffset(zone);
	return (
		<Tooltip title={offset ? `${zone} (${offset})` : zone}>
			<span>
				{clockRange(time, end)} {zoneLabel(zone)}
			</span>
		</Tooltip>
	);
}

/// The window as a reader says it, or the hour alone where the plan names no
/// close.
function clockRange(time: string, end: string | null): string {
	return end ? `${clockTime(time)}-${clockTime(end)}` : clockTime(time);
}

const DAY_MINUTES = 24 * 60;

/// What the calendar feed allows a plan that names no close.
const ASSUMED_MINUTES = 60;

function minutesInto(time: string): number {
	const [hours, minutes] = time.split(":").map(Number);
	return hours * 60 + minutes;
}

function wallClock(minutes: number): string {
	const of = ((minutes % DAY_MINUTES) + DAY_MINUTES) % DAY_MINUTES;
	const hours = String(Math.floor(of / 60)).padStart(2, "0");
	return `${hours}:${String(of % 60).padStart(2, "0")}`;
}

/// How long a window runs. A close earlier in the day than the open is the next
/// morning.
/// The close a form offers as soon as an hour is typed, so the common case is
/// one field rather than two.
function anHourLater(time: string): string {
	return time ? wallClock(minutesInto(time) + ASSUMED_MINUTES) : "";
}

function windowMinutes(time: string, end: string | null): number {
	if (!end) return ASSUMED_MINUTES;
	return (
		(minutesInto(end) - minutesInto(time) + DAY_MINUTES) % DAY_MINUTES ||
		ASSUMED_MINUTES
	);
}

function clockTime(time: string): string {
	const [hours, minutes] = time.split(":").map(Number);
	const suffix = hours < 12 ? "am" : "pm";
	const hour = hours % 12 === 0 ? 12 : hours % 12;
	if (minutes === 0) return `${hour}${suffix}`;
	return `${hour}:${String(minutes).padStart(2, "0")}${suffix}`;
}

/// tzdata dropped invented abbreviations in 2017, so these zones report
/// themselves as `+12` and CLDR only offers `GMT+12`. Australia and New Zealand
/// are absent on purpose: CLDR has real DST-aware ones for those.
const ZONE_ABBREVIATIONS: Record<string, string> = {
	"Pacific/Fiji": "FJT",
	"Pacific/Nauru": "NRT",
	"Pacific/Tarawa": "GILT",
	"Pacific/Guadalcanal": "SBT",
	"Pacific/Port_Moresby": "PGT",
	"Pacific/Efate": "VUT",
	"Pacific/Apia": "WST",
	"Pacific/Tongatapu": "TOT",
	"Pacific/Palau": "PWT",
	"Asia/Karachi": "PKT",
	"Asia/Dili": "TLT",
	"Indian/Maldives": "MVT",
};

/// What to call the zone in a table cell. CLDR first, since it is DST-aware
/// where it has an answer, then the region's own abbreviation, then the place.
function zoneLabel(zone: string): string {
	const short = zoneShortName(zone);
	if (short && !short.startsWith("GMT") && !short.startsWith("UTC")) {
		return short;
	}
	return (
		ZONE_ABBREVIATIONS[zone] ?? (zone.split("/").pop() ?? zone).replace(/_/g, " ")
	);
}

const zoneShortName = (zone: string) => zonePart(zone, "short");

const zoneOffset = (zone: string) => zonePart(zone, "shortOffset");

function zonePart(
	zone: string,
	timeZoneName: "short" | "shortOffset",
): string | null {
	try {
		return (
			new Intl.DateTimeFormat("en", { timeZone: zone, timeZoneName })
				.formatToParts(new Date())
				.find((part) => part.type === "timeZoneName")?.value ?? null
		);
	} catch {
		return null;
	}
}

const GOING_FIELDS = {
	display: "grid",
	gridTemplateColumns: "repeat(3, minmax(0, 1fr))",
	columnGap: 1.5,
	alignItems: "start",
};

const WHEN_FIELDS = {
	display: "grid",
	gridTemplateColumns: "minmax(0, 1fr) 136px 136px",
	columnGap: 1.5,
};

const NOTE_TEXT = { maxWidth: 240 };

const TIGHT_TABLE = {
	"& td, & th": { width: "1%", whiteSpace: "nowrap" },
	"& td:last-child, & th:last-child": { width: "auto" },
};

const ZONES = Intl.supportedValuesOf("timeZone");

/// The zone most plans are in, so leaving it alone records what was meant.
const DEFAULT_ZONE = "Pacific/Fiji";

/// The zone a planned time is a wall clock in. Paired with the time field: a
/// plan with no hour records no zone.
function ZoneField({
	value,
	onChange,
	disabled,
}: {
	value: string;
	onChange: (zone: string) => void;
	disabled: boolean;
}) {
	return (
		<Autocomplete<string, false, true, false>
			size="small"
			disabled={disabled}
			disableClearable
			options={ZONES}
			value={value}
			onChange={(_, zone) => onChange(zone)}
			renderInput={(params) => <TextField {...params} label="Timezone" />}
		/>
	);
}

const SHORTLIST_MINORS = 10;

const matchVersion = createFilterOptions<PlannableVersion>({
	stringify: (option) => option.version,
});

/// The newest ready patch of each of the last ten minors. Every published
/// version ahead is plannable, which runs to hundreds, so the list an operator
/// scrolls is the recent releases and typing reaches the rest.
///
/// A minor with nothing clear of known issues still gets a row, flagged, rather
/// than dropping out of the list unexplained.
function recentMinors(options: PlannableVersion[]): PlannableVersion[] {
	const picked = new Map<string, PlannableVersion>();
	for (const option of options) {
		const minor = option.version.split(".").slice(0, 2).join(".");
		const held = picked.get(minor);
		if (held ? held.ready || !option.ready : picked.size === SHORTLIST_MINORS) {
			continue;
		}
		picked.set(minor, option);
	}
	return [...picked.values()];
}

function helperText(
	chosen: boolean,
	options: PlannableVersion[],
	shortlist: PlannableVersion[],
): string | undefined {
	if (!chosen) return undefined;
	if (options.length === 0) return "already on the newest";
	if (options.length > shortlist.length) return "type for older";
	return undefined;
}

type EnvironmentOption = {
	groupId: string;
	groupName: string;
	rank: ServerRank;
};

/// Record where an environment is going. The version picker offers only valid
/// targets, so the operator cannot pick one the API would refuse.
// spec: UPG#a-plan
function RecordPlan({
	environments,
	onRecorded,
}: {
	environments: EnvironmentOption[];
	onRecorded: () => void;
}) {
	const [open, setOpen] = useState(false);

	return (
		<>
			<Button
				variant="contained"
				startIcon={<AddIcon />}
				onClick={() => setOpen(true)}
			>
				Record a plan
			</Button>
			{open && (
				<RecordPlanDialog
					environments={environments}
					onClose={() => setOpen(false)}
					onRecorded={() => {
						setOpen(false);
						onRecorded();
					}}
				/>
			)}
		</>
	);
}

/// Mounted only while it is open, so it opens empty rather than holding what
/// was typed the last time.
function RecordPlanDialog({
	environments,
	onClose,
	onRecorded,
}: {
	environments: EnvironmentOption[];
	onClose: () => void;
	onRecorded: () => void;
}) {
	const [groupId, setGroupId] = useState("");
	const [rank, setRank] = useState<ServerRank | "">("");
	const [versionId, setVersionId] = useState("");
	const [plannedFor, setPlannedFor] = useState("");
	const [plannedTime, setPlannedTime] = useState("");
	const [endTime, setEndTime] = useState("");
	const [endTyped, setEndTyped] = useState(false);
	const [zone, setZone] = useState(DEFAULT_ZONE);
	const [note, setNote] = useState("");
	const record = useApiAction("upgrade_plans", "record");
	const groups = useMemo(() => {
		const seen = new Map<string, string>();
		for (const env of environments) {
			if (!seen.has(env.groupId)) seen.set(env.groupId, env.groupName);
		}
		return [...seen].map(([id, name]) => ({ id, name }));
	}, [environments]);
	const ranks = environments
		.filter((env) => env.groupId === groupId)
		.map((env) => env.rank);
	const chosen = groupId !== "" && rank !== "";
	const targets = useApi(
		"upgrade_plans",
		"targets",
		chosen ? { group_id: groupId, rank } : undefined,
		[groupId, rank],
	);

	const options = targets.status === "ok" ? targets.data : [];
	const shortlist = useMemo(() => recentMinors(options), [options]);

	const submit = async () => {
		if (!groupId || rank === "" || !versionId) return;
		await record.call({
			group_id: groupId,
			rank,
			target_version_id: versionId,
			planned_for: plannedFor || null,
			planned_time: plannedTime || null,
			planned_end_time: plannedTime ? endTime || null : null,
			planned_zone: plannedTime ? zone : null,
			note: note || null,
		});
		onRecorded();
	};

	return (
		<Dialog
			open
			onClose={onClose}
			fullWidth
			maxWidth="sm"
			data-testid="record-plan"
		>
			<DialogTitle>Record a plan</DialogTitle>
			<DialogContent>
				<Stack spacing={2} sx={{ mt: 1 }}>
					<Box sx={GOING_FIELDS}>
						<TextField
							select
							size="small"
							label="Group"
							value={groupId}
							onChange={(e) => {
								const id = e.target.value;
								const offered = environments.filter(
									(env) => env.groupId === id,
								);
								setGroupId(id);
								// One environment needs no second choice.
								setRank(offered.length === 1 ? offered[0].rank : "");
								setVersionId("");
							}}
						>
							{groups.map((group) => (
								<MenuItem key={group.id} value={group.id}>
									{group.name}
								</MenuItem>
							))}
						</TextField>
						<TextField
							select
							size="small"
							label="Environment"
							value={rank}
							disabled={!groupId}
							onChange={(e) => {
								setRank(e.target.value as ServerRank);
								setVersionId("");
							}}
						>
							{ranks.map((option) => (
								<MenuItem
									key={option}
									value={option}
									sx={{ textTransform: "capitalize" }}
								>
									{option}
								</MenuItem>
							))}
						</TextField>
						<Autocomplete<PlannableVersion, false, false, false>
							size="small"
							disabled={!chosen || options.length === 0}
							options={options}
							value={options.find((option) => option.id === versionId) ?? null}
							onChange={(_, option) => setVersionId(option?.id ?? "")}
							getOptionLabel={(option) => option.version}
							isOptionEqualToValue={(a, b) => a.id === b.id}
							filterOptions={(all, state) =>
								state.inputValue === "" ? shortlist : matchVersion(all, state)
							}
							renderOption={(props, option) => (
								<li {...props} key={option.id}>
									<Stack
										direction="row"
										spacing={1}
										sx={{ alignItems: "center" }}
									>
										<span>{option.version}</span>
										{!option.ready && (
											<Chip
												size="small"
												color="warning"
												variant="outlined"
												label="known issue"
											/>
										)}
									</Stack>
								</li>
							)}
							renderInput={(params) => (
								<TextField
									{...params}
									label="Going to"
									helperText={helperText(chosen, options, shortlist)}
								/>
							)}
						/>
					</Box>
					<Box sx={WHEN_FIELDS}>
						<TextField
							size="small"
							type="date"
							label="Planned for"
							disabled={!chosen}
							value={plannedFor}
							onChange={(e) => {
								setPlannedFor(e.target.value);
								if (!e.target.value) {
									setPlannedTime("");
									setEndTime("");
									setEndTyped(false);
								}
							}}
							slotProps={{ inputLabel: { shrink: true } }}
						/>
						<TextField
							size="small"
							type="time"
							label="Starts"
							value={plannedTime}
							disabled={!plannedFor}
							onChange={(e) => {
								setPlannedTime(e.target.value);
								if (!e.target.value) {
									setEndTime("");
									setEndTyped(false);
								} else if (!endTyped) {
									setEndTime(anHourLater(e.target.value));
								}
							}}
							slotProps={{ inputLabel: { shrink: true } }}
						/>
						<TextField
							size="small"
							type="time"
							label="Ends"
							value={endTime}
							disabled={!plannedTime}
							onChange={(e) => {
								setEndTime(e.target.value);
								setEndTyped(true);
							}}
							slotProps={{ inputLabel: { shrink: true } }}
						/>
					</Box>
					<ZoneField value={zone} onChange={setZone} disabled={!plannedTime} />
					<TextField
						size="small"
						label="Note"
						disabled={!chosen}
						value={note}
						onChange={(e) => setNote(e.target.value)}
						multiline
						minRows={2}
					/>
					{record.error && (
						<Alert severity="error">{record.error.message}</Alert>
					)}
				</Stack>
			</DialogContent>
			<DialogActions>
				<Button onClick={onClose}>Cancel</Button>
				<Button
					variant="contained"
					disabled={!chosen || !versionId || record.pending}
					onClick={submit}
				>
					Record
				</Button>
			</DialogActions>
		</Dialog>
	);
}

/// Amend an open plan's date and note. The target is deliberately absent:
/// moving a group somewhere else is a new plan, not a correction to this
/// one, so it goes through the record form.
// spec: UPG#a-plan
function EditPlan({
	planId,
	groupName,
	targetVersion,
	plannedFor,
	plannedTime,
	plannedEnd,
	plannedZone,
	note,
	onAmended,
}: PlanFields & { onAmended: () => void }) {
	const [open, setOpen] = useState(false);

	return (
		<>
			<IconButton
				size="small"
				aria-label={`Edit ${groupName}'s plan`}
				onClick={() => setOpen(true)}
				disabled={!planId}
			>
				<EditIcon fontSize="small" />
			</IconButton>
			{open && (
				<EditPlanDialog
					planId={planId}
					groupName={groupName}
					targetVersion={targetVersion}
					plannedFor={plannedFor}
					plannedTime={plannedTime}
					plannedEnd={plannedEnd}
					plannedZone={plannedZone}
					note={note}
					onClose={() => setOpen(false)}
					onAmended={() => {
						setOpen(false);
						onAmended();
					}}
				/>
			)}
		</>
	);
}

type PlanFields = {
	planId: string;
	groupName: string;
	targetVersion: string;
	plannedFor: string | null;
	plannedTime: string | null;
	plannedEnd: string | null;
	plannedZone: string | null;
	note: string | null;
};

/// The amendment form. Mounted only while it is open, so it always starts from
/// the plan as it now stands rather than what it held the last time.
// spec: UPG#a-plan
function EditPlanDialog({
	planId,
	groupName,
	targetVersion,
	plannedFor,
	plannedTime,
	plannedEnd,
	plannedZone,
	note,
	onClose,
	onAmended,
}: PlanFields & { onClose: () => void; onAmended: () => void }) {
	const [date, setDate] = useState(plannedFor ?? "");
	const [time, setTime] = useState(plannedTime?.slice(0, 5) ?? "");
	const [end, setEnd] = useState(plannedEnd?.slice(0, 5) ?? "");
	// A plan that already names an hour stands as recorded: filling an hour in
	// for it would assert a window nobody typed.
	const [endTyped, setEndTyped] = useState(plannedTime !== null);
	const [zone, setZone] = useState(plannedZone ?? DEFAULT_ZONE);
	const [text, setText] = useState(note ?? "");
	const amend = useApiAction("upgrade_plans", "amend");

	const save = async () => {
		await amend.call({
			id: planId,
			planned_for: date || null,
			planned_time: time || null,
			planned_end_time: time ? end || null : null,
			planned_zone: time ? zone : null,
			note: text || null,
		});
		onAmended();
	};

	return (
		<Dialog open onClose={onClose} fullWidth maxWidth="sm" data-testid="edit-plan">
			<DialogTitle>
				{groupName} &rarr; {targetVersion}
			</DialogTitle>
			<DialogContent>
				<Stack spacing={2} sx={{ mt: 1 }}>
					<Box sx={WHEN_FIELDS}>
						<TextField
							size="small"
							type="date"
							label="Planned for"
							value={date}
							onChange={(e) => {
								setDate(e.target.value);
								if (!e.target.value) {
									setTime("");
									setEnd("");
								}
							}}
							slotProps={{ inputLabel: { shrink: true } }}
						/>
						<TextField
							size="small"
							type="time"
							label="Starts"
							value={time}
							disabled={!date}
							onChange={(e) => {
								setTime(e.target.value);
								if (!e.target.value) {
									setEnd("");
									setEndTyped(false);
								} else if (!endTyped) {
									setEnd(anHourLater(e.target.value));
								}
							}}
							slotProps={{ inputLabel: { shrink: true } }}
						/>
						<TextField
							size="small"
							type="time"
							label="Ends"
							value={end}
							disabled={!time}
							onChange={(e) => {
								setEnd(e.target.value);
								setEndTyped(true);
							}}
							slotProps={{ inputLabel: { shrink: true } }}
						/>
					</Box>
					<ZoneField value={zone} onChange={setZone} disabled={!time} />
					<TextField
						size="small"
						label="Note"
						value={text}
						onChange={(e) => setText(e.target.value)}
						multiline
						minRows={2}
					/>
					<Typography variant="body2" color="text.secondary">
						To move {groupName} to a different version, record a new plan
						instead. This one is kept as history.
					</Typography>
					{amend.error && <Alert severity="error">{amend.error.message}</Alert>}
				</Stack>
			</DialogContent>
			<DialogActions>
				<Button onClick={onClose}>Cancel</Button>
				<Button variant="contained" onClick={save} disabled={amend.pending}>
					Save
				</Button>
			</DialogActions>
		</Dialog>
	);
}

/// Withdraw a plan: the group is no longer going there. This does not say
/// the upgrade happened; Canopy closes a met plan on its own.
// spec: UPG#a-plan
function WithdrawPlan({
	planId,
	groupName,
	targetVersion,
	onWithdrawn,
}: {
	planId: string;
	groupName: string;
	targetVersion: string;
	onWithdrawn: () => void;
}) {
	const withdraw = useApiAction("upgrade_plans", "withdraw");
	const onClick = async () => {
		if (
			!window.confirm(
				`Withdraw ${groupName}'s plan to move to ${targetVersion}? Pre-upgrade testing stops for this group until a new plan is recorded.`,
			)
		)
			return;
		try {
			await withdraw.call({ id: planId });
			onWithdrawn();
		} catch {
			/* surfaced by the reload showing the plan still there */
		}
	};

	return (
		<IconButton
			size="small"
			aria-label={`Withdraw ${groupName}'s plan`}
			onClick={onClick}
			disabled={withdraw.pending || !planId}
		>
			<DeleteIcon fontSize="small" />
		</IconButton>
	);
}

/** How long the plan says the group is down, from its window's two
 * wall clocks. A close earlier in the day than the open is the following
 * morning, as the plan reads it. Two hours where the plan names no window,
 * which is what a declaration otherwise starts from. */
function plannedHours(time: string | null, end: string | null): number {
	if (!time || !end) return 2;
	const minutes = (clock: string) => {
		const [h, m] = clock.split(":");
		return Number(h) * 60 + Number(m ?? 0);
	};
	const span = minutes(end) - minutes(time);
	return (span > 0 ? span : span + 24 * 60) / 60;
}

/** Declare maintenance over an environment from its open plan, carrying the
 * plan's window length and note. The plan supplies the prefill and triggers
 * nothing: this is an operator saying the work is starting now. */
// spec: MNT#declaring
function DeclareFromPlan({
	groupId,
	rank,
	groupName,
	plannedTime,
	plannedEnd,
	note,
	onDeclared,
}: {
	groupId: string;
	rank: ServerRank;
	groupName: string;
	plannedTime: string | null;
	plannedEnd: string | null;
	note: string | null;
	onDeclared: () => void;
}) {
	const [open, setOpen] = useState(false);
	const hours = plannedHours(plannedTime, plannedEnd);
	return (
		<>
			<Tooltip title="Declare maintenance: suspend this environment's alerting while the upgrade runs">
				<IconButton
					size="small"
					aria-label={`Declare maintenance for ${groupName}`}
					onClick={() => setOpen(true)}
				>
					<BuildOutlinedIcon fontSize="small" />
				</IconButton>
			</Tooltip>
			<DeclareMaintenanceDialog
				open={open}
				onClose={() => setOpen(false)}
				scope="group"
				id={groupId}
				rank={rank}
				targetLabel={groupName}
				prefill={{
					expectedEnd: new Date(
						Date.now() + hours * 3600_000,
					).toISOString(),
					note: note ?? undefined,
				}}
				onDone={onDeclared}
			/>
		</>
	);
}
