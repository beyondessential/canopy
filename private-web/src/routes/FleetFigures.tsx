import {
	Alert,
	Autocomplete,
	Box,
	Chip,
	IconButton,
	LinearProgress,
	MenuItem,
	Paper,
	Select,
	Stack,
	Table,
	TableBody,
	TableCell,
	TableContainer,
	TableHead,
	TableRow,
	TextField,
	Tooltip,
	Typography,
	createFilterOptions,
} from "@mui/material";
import BarChartIcon from "@mui/icons-material/BarChart";
import ExpandLessIcon from "@mui/icons-material/ExpandLess";
import ExpandMoreIcon from "@mui/icons-material/ExpandMore";
import SortByAlphaIcon from "@mui/icons-material/SortByAlpha";
import { useMemo, useState } from "react";
import { Link as RouterLink } from "react-router-dom";
import { useApi } from "../api";
import { usePageTitle } from "../hooks/usePageTitle";
import { useReloadInterval } from "../hooks/useReloadInterval";
import { useIsVersionTracked } from "../hooks/useApplicationTypes";
import { valueComparator } from "../lib/valueOrder";
import type {
	ApplicationType,
	FleetMachineDetailData,
	FleetServerDetailData,
} from "../types";

/// How many distinct values a distribution card shows before collapsing the
/// rest. A field like `uptimeSecs` is near-unique across the fleet, and a
/// line per target tells an operator nothing about spread.
const TOP_VALUES = 8;

/// The release branch a Tamanu version belongs to: `2.54.3` → `2.54`. The
/// fleet moves a release at a time, so the branch is the grouping an
/// operator acts on; the exact version splits it finer than that.
// spec: FIG#fleet-spread
function tamanuRelease(version: string | null): string | null {
	const parts = /^(\d+)\.(\d+)/.exec(version ?? "");
	return parts && `${parts[1]}.${parts[2]}`;
}

/// PostgreSQL's own major version, which the project renumbered at 10: from
/// there it's the leading component (`16.3` → `16`), and before it the first
/// two (`9.6.24` → `9.6`).
// spec: FIG#fleet-spread
function postgresMajor(version: string | null): string | null {
	const parts = /^(\d+)(?:\.(\d+))?/.exec(version ?? "");
	if (!parts) return null;
	if (Number(parts[1]) >= 10 || parts[2] === undefined) return parts[1]!;
	return `${parts[1]}.${parts[2]}`;
}

/// Which population a figure or field is spread over. A fact about the box
/// belongs to the box however many workloads it carries.
// spec: FIG#fleet-spread
type Grain = "machine" | "application";

/// What one of a population is called, so a count on screen names the unit it
/// is counting rather than leaving the reader to assume.
// spec: FIG#crossings
const UNIT: Record<Grain, { one: string; many: string }> = {
	machine: { one: "machine", many: "machines" },
	application: { one: "application", many: "applications" },
};

/// One member of either population, flattened so the spread, the lookup and
/// the crossing work the same way on both.
interface Row {
	grain: Grain;
	id: string;
	name: string;
	/// The box this row counts against in a crossing: a machine's own id, an
	/// application's the box it runs on.
	machineId: string;
	href: string;
	/// What the application is, for the figures whose spread covers only the
	/// types canopy tracks a release train for. Absent on a machine.
	type: ApplicationType | null;
	detail: Record<string, unknown>;
	checks: Record<string, Record<string, unknown> | undefined>;
	/// The derived figures, by figure key.
	figures: Record<string, unknown>;
}

interface Figure {
	key: string;
	label: string;
	/// Whether the figure leads the page as a card of its own. The rest are
	/// reachable through the field lookup and the crossing.
	card?: boolean;
	/// The population it spreads over.
	grain: Grain;
}

/// The figures canopy derives. Each version figure carries a coarser
/// grouping alongside the exact value it reports, and the coarse one is what
/// the page leads with.
// spec: FIG#fleet-spread
const FIGURES: Figure[] = [
	{ key: "release", label: "Tamanu release", card: true, grain: "application" },
	{ key: "version", label: "Tamanu version", grain: "application" },
	{
		key: "postgresMajor",
		label: "PostgreSQL major",
		card: true,
		grain: "application",
	},
	{ key: "postgres", label: "PostgreSQL version", grain: "application" },
	{ key: "bestool", label: "bestool", card: true, grain: "machine" },
	{ key: "platform", label: "Platform", card: true, grain: "machine" },
	{ key: "nodejs", label: "Node.js", card: true, grain: "application" },
	{ key: "timezone", label: "Timezone", card: true, grain: "application" },
];

function machineRow(machine: FleetMachineDetailData): Row {
	return {
		grain: "machine",
		id: machine.machine_id,
		name: machine.machine_name,
		machineId: machine.machine_id,
		href: `/fleet/machines/${machine.machine_id}`,
		type: null,
		detail: (machine.detail ?? {}) as Record<string, unknown>,
		checks: (machine.checks ?? {}) as Row["checks"],
		figures: { bestool: machine.bestool, platform: machine.platform },
	};
}

function applicationRow(application: FleetServerDetailData): Row {
	return {
		grain: "application",
		id: application.server_id,
		name: application.server_name,
		machineId: application.machine_id,
		href: `/fleet/applications/${application.server_id}`,
		type: application.type,
		detail: (application.detail ?? {}) as Record<string, unknown>,
		checks: (application.checks ?? {}) as Row["checks"],
		figures: {
			release: tamanuRelease(application.version),
			version: application.version,
			postgresMajor: postgresMajor(application.postgres),
			postgres: application.postgres,
			nodejs: application.nodejs,
			timezone: application.timezone,
		},
	};
}

/// The fields whose spread covers only the applications whose type canopy
/// holds a release train for. An application that has no version to report is
/// absent from these spreads rather than counted among those reporting nothing
/// — it isn't one that failed to report, it's one with nothing to report.
/// Every other field, including the database-engine and bestool versions,
/// still covers the whole fleet.
// spec: APP#versions
const VERSION_TRACKED_FIELDS = new Set(["release", "version"]);

/// Narrow a population to the rows a field's spread covers.
function coveredBy(
	field: string,
	rows: Row[],
	isVersionTracked: (type: ApplicationType) => boolean,
): Row[] {
	if (!VERSION_TRACKED_FIELDS.has(field)) return rows;
	return rows.filter((r) => r.type !== null && isVersionTracked(r.type));
}

/// The name to present a field under: a figure's own label, or the field
/// name as an operator typed it.
function fieldLabel(field: string): string {
	return FIGURES.find((f) => f.key === field)?.label ?? field;
}

/// Which population a field is spread over. A derived figure declares its
/// grain; an arbitrary field takes it from the data, a key a machine reports
/// being the box's and everything else the workload's.
// spec: FIG#fleet-spread
function grainOf(field: string, machineKeys: Set<string>): Grain {
	const figure = FIGURES.find((f) => f.key === field);
	if (figure) return figure.grain;
	return machineKeys.has(field) ? "machine" : "application";
}

/// Read one healthcheck field off a row. Check names can contain dots
/// themselves, so every split point is tried, longest check name first: for
/// `pg.replication.lag`, a check literally named `pg.replication` wins over
/// one named `pg`.
function pickCheckField(row: Row, field: string): unknown {
	for (let dot = field.lastIndexOf("."); dot > 0; dot = field.lastIndexOf(".", dot - 1)) {
		const check = row.checks[field.slice(0, dot)];
		if (check) return check[field.slice(dot + 1)];
	}
	return undefined;
}

/// Resolve a field name to the value it takes on a row: one of the derived
/// figures, a healthcheck's field as `check.field`, or a field the sources
/// report against the target itself.
function picker(field: string): (row: Row) => unknown {
	if (FIGURES.some((f) => f.key === field)) return (row) => row.figures[field];
	const own = (row: Row) => row.detail[field];
	if (!field.includes(".")) return own;
	// A target-wide field can carry a dot in its name too, so fall back to
	// one where no check answers to the part before the dot.
	return (row) => {
		const onCheck = pickCheckField(row, field);
		return onCheck === undefined ? own(row) : onCheck;
	};
}

/// The label a value groups under. Everything is compared as its rendered
/// text, so `16.3` and `"16.3"` land in the same bucket; `null` means the
/// target reports nothing for this field.
function bucket(value: unknown): string | null {
	if (value == null) return null;
	if (typeof value === "string") return value === "" ? null : value;
	if (typeof value === "object") return JSON.stringify(value);
	return String(value);
}

interface Group {
	value: string | null;
	rows: Row[];
}

/// Put a value-to-rows map in presentation order: largest group first, with
/// the unreported group always last so it doesn't crowd out real values.
function groupsFrom(byValue: Map<string | null, Row[]>): Group[] {
	return [...byValue.entries()]
		.map(([value, rows]) => ({ value, rows }))
		.sort((a, b) => {
			if ((a.value === null) !== (b.value === null)) return a.value === null ? 1 : -1;
			return b.rows.length - a.rows.length || String(a.value).localeCompare(String(b.value));
		});
}

/// Group a population by its value for one field.
function distribution(rows: Row[], pick: (row: Row) => unknown): Group[] {
	const byValue = new Map<string | null, Row[]>();
	for (const row of rows) {
		const key = bucket(pick(row));
		const existing = byValue.get(key);
		if (existing) existing.push(row);
		else byValue.set(key, [row]);
	}
	return groupsFrom(byValue);
}

/// Whether a spread reads largest group first, or in the order of the values
/// themselves.
type SortMode = "popularity" | "value";

/// Reorder a spread that [`groupsFrom`] has already put in popularity
/// order. Sorting by value compares the values as whatever they look like —
/// numbers, versions, or text. The unreported group stays last either way:
/// it's a population rather than a value, and sorts nowhere meaningful among
/// them.
// spec: FIG#fleet-spread
function orderGroups(groups: Group[], mode: SortMode): Group[] {
	if (mode === "popularity") return groups;
	const values = groups
		.map((g) => g.value)
		.filter((v): v is string => v !== null);
	const compare = valueComparator(values);
	return [...groups].sort((a, b) => {
		if (a.value === null || b.value === null) {
			return a.value === b.value ? 0 : a.value === null ? 1 : -1;
		}
		return compare(a.value, b.value);
	});
}

/// Flips a spread between its two orders. Shows what a click does rather
/// than where things stand, since the order itself is already on screen.
function SortToggle({
	mode,
	onChange,
}: {
	mode: SortMode;
	onChange: (mode: SortMode) => void;
}) {
	const next: SortMode = mode === "popularity" ? "value" : "popularity";
	const label = next === "value" ? "Sort by value" : "Sort by popularity";
	return (
		<Tooltip title={label}>
			<IconButton size="small" aria-label={label} onClick={() => onChange(next)}>
				{next === "value" ? (
					<SortByAlphaIcon fontSize="inherit" />
				) : (
					<BarChartIcon fontSize="inherit" />
				)}
			</IconButton>
		</Tooltip>
	);
}

export default function FleetFigures() {
	usePageTitle("Fleet figures");
	const tick = useReloadInterval(60_000, "canopy-data-changed");
	const result = useApi("statuses", "fleet_detail", {}, [tick]);
	const isVersionTracked = useIsVersionTracked();

	const data = result.status === "ok" ? result.data : null;
	const machines = useMemo(
		() => (data?.machines ?? []).map(machineRow),
		[data],
	);
	const applications = useMemo(
		() => (data?.applications ?? []).map(applicationRow),
		[data],
	);

	// Every field any source currently reports, so an operator can find one
	// without knowing its name. Healthcheck fields come through as
	// `check.field` — the same lookup, addressed through the check that
	// reports them. The machine's keys are kept apart, because a key a box
	// reports is what makes a field machine-grained.
	// spec: FIG#fleet-spread
	const { reportedKeys, machineKeys } = useMemo(() => {
		const keysOf = (rows: Row[]) => {
			const own = new Set<string>();
			const onChecks = new Set<string>();
			for (const row of rows) {
				for (const key of Object.keys(row.detail)) own.add(key);
				for (const [check, fields] of Object.entries(row.checks)) {
					for (const key of Object.keys(fields ?? {})) onChecks.add(`${check}.${key}`);
				}
			}
			return { own, onChecks };
		};
		const machine = keysOf(machines);
		const application = keysOf(applications);
		const own = [...new Set([...machine.own, ...application.own])].sort();
		const onChecks = [
			...new Set([...machine.onChecks, ...application.onChecks]),
		].sort();
		return {
			reportedKeys: [...own, ...onChecks],
			machineKeys: new Set([...machine.own, ...machine.onChecks]),
		};
	}, [machines, applications]);

	const population = (grain: Grain) =>
		grain === "machine" ? machines : applications;

	if (result.status === "loading" || result.status === "idle") {
		return <LinearProgress />;
	}
	if (result.status === "error") {
		return <Alert severity="error">{result.error.message}</Alert>;
	}
	if (machines.length === 0 && applications.length === 0) {
		return <Alert severity="info">No servers yet.</Alert>;
	}

	return (
		<Stack spacing={3}>
			<Typography variant="body2" color="text.secondary">
				What the fleet's {machines.length}{" "}
				{machines.length === 1 ? "machine" : "machines"} and{" "}
				{applications.length}{" "}
				{applications.length === 1 ? "application" : "applications"} currently
				report about themselves. Each value is the most recent one any source
				reported, so a target that has gone quiet still counts.
			</Typography>

			<Box
				sx={{
					display: "grid",
					gap: 2,
					gridTemplateColumns: {
						xs: "1fr",
						sm: "repeat(2, 1fr)",
						md: "repeat(3, 1fr)",
					},
				}}
			>
				{FIGURES.filter((figure) => figure.card).map((figure) => {
					// Each card's total is the population it actually covers: the
					// grain the figure belongs to, and within it the targets that
					// have such a figure to report at all.
					// spec: FIG#fleet-spread
					const covered = coveredBy(
						figure.key,
						population(figure.grain),
						isVersionTracked,
					);
					return (
						<DistributionCard
							key={figure.key}
							label={figure.label}
							grain={figure.grain}
							groups={distribution(covered, picker(figure.key))}
							total={covered.length}
						/>
					);
				})}
			</Box>

			<LookupCard
				machines={machines}
				applications={applications}
				keys={reportedKeys}
				machineKeys={machineKeys}
			/>
			<CrossTab
				machines={machines}
				applications={applications}
				keys={reportedKeys}
				machineKeys={machineKeys}
			/>
		</Stack>
	);
}

function DistributionCard({
	label,
	grain,
	groups,
	total,
}: {
	label: string;
	grain: Grain;
	groups: Group[];
	total: number;
}) {
	const [expanded, setExpanded] = useState<string | null>(null);
	const [showAll, setShowAll] = useState(false);
	const [sort, setSort] = useState<SortMode>("popularity");

	// Which values are worth a line is a question of size, so the collapse
	// takes the largest groups whichever order they end up presented in.
	const top = showAll ? groups : groups.slice(0, TOP_VALUES);
	const shown = orderGroups(top, sort);
	const hidden = groups.length - top.length;
	const hiddenRows = groups
		.slice(top.length)
		.reduce((sum, g) => sum + g.rows.length, 0);

	return (
		<Paper variant="outlined" sx={{ p: 2 }} role="group" aria-label={label}>
			<Stack
				direction="row"
				sx={{ alignItems: "center", justifyContent: "space-between", mb: 1 }}
			>
				<Stack direction="row" spacing={1} sx={{ alignItems: "baseline" }}>
					<Typography variant="subtitle2">{label}</Typography>
					<Typography variant="caption" color="text.secondary">
						{total} {total === 1 ? UNIT[grain].one : UNIT[grain].many}
					</Typography>
				</Stack>
				<SortToggle mode={sort} onChange={setSort} />
			</Stack>
			<Stack spacing={0.5}>
				{shown.map((group) => (
					<ValueRow
						key={group.value ?? " unreported"}
						group={group}
						total={total}
						expanded={expanded === (group.value ?? " unreported")}
						onToggle={() =>
							setExpanded((v) =>
								v === (group.value ?? " unreported")
									? null
									: (group.value ?? " unreported"),
							)
						}
					/>
				))}
				{hidden > 0 && (
					<Typography
						variant="caption"
						color="text.secondary"
						sx={{ cursor: "pointer" }}
						onClick={() => setShowAll(true)}
					>
						{hidden} other {hidden === 1 ? "value" : "values"} ({hiddenRows}{" "}
						{hiddenRows === 1 ? UNIT[grain].one : UNIT[grain].many}) — show all
					</Typography>
				)}
			</Stack>
		</Paper>
	);
}

function ValueRow({
	group,
	total,
	expanded,
	onToggle,
}: {
	group: Group;
	total: number;
	expanded: boolean;
	onToggle: () => void;
}) {
	const share = total === 0 ? 0 : (group.rows.length / total) * 100;
	return (
		<Box>
			<Stack
				direction="row"
				spacing={1}
				sx={{ alignItems: "center", cursor: "pointer" }}
				onClick={onToggle}
				role="button"
				aria-label={`${group.value ?? "not reported"}: ${group.rows.length}`}
			>
				<Typography
					variant="body2"
					sx={{
						fontFamily: group.value === null ? undefined : "monospace",
						color: group.value === null ? "text.secondary" : undefined,
						minWidth: 0,
						flex: "0 1 auto",
						overflow: "hidden",
						textOverflow: "ellipsis",
						whiteSpace: "nowrap",
					}}
				>
					{group.value ?? "not reported"}
				</Typography>
				<Box
					sx={{
						flex: "1 1 auto",
						height: 6,
						borderRadius: 1,
						bgcolor: "action.hover",
						overflow: "hidden",
						minWidth: 24,
					}}
				>
					<Box
						sx={{
							width: `${share}%`,
							height: "100%",
							bgcolor: group.value === null ? "action.disabled" : "primary.main",
						}}
					/>
				</Box>
				<Typography variant="body2" sx={{ flexShrink: 0 }}>
					{group.rows.length}
				</Typography>
				{expanded ? (
					<ExpandLessIcon fontSize="small" />
				) : (
					<ExpandMoreIcon fontSize="small" />
				)}
			</Stack>
			{expanded && <TargetChips rows={group.rows} />}
		</Box>
	);
}

function TargetChips({ rows }: { rows: Row[] }) {
	return (
		<Stack direction="row" spacing={0.5} sx={{ flexWrap: "wrap", py: 0.5 }} useFlexGap>
			{rows.map((row) => (
				<Chip
					key={`${row.grain}:${row.id}`}
					size="small"
					variant="outlined"
					label={row.name}
					component={RouterLink}
					to={row.href}
					clickable
				/>
			))}
		</Stack>
	);
}

function LookupCard({
	machines,
	applications,
	keys,
	machineKeys,
}: {
	machines: Row[];
	applications: Row[];
	keys: string[];
	machineKeys: Set<string>;
}) {
	const [field, setField] = useState<string | null>(null);
	const isVersionTracked = useIsVersionTracked();
	const grain = field === null ? "application" : grainOf(field, machineKeys);
	const covered = useMemo(
		() =>
			field === null
				? []
				: coveredBy(
						field,
						grain === "machine" ? machines : applications,
						isVersionTracked,
					),
		[machines, applications, field, grain, isVersionTracked],
	);
	const groups = useMemo(
		() => (field === null ? [] : distribution(covered, picker(field))),
		[covered, field],
	);
	// The figures the page doesn't lead with — the exact versions behind the
	// coarse groupings, mostly — are only reachable here and in the crossing.
	const options = useMemo(() => [...FIGURES.map((f) => f.key), ...keys], [keys]);

	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<Typography variant="subtitle2" sx={{ mb: 1 }}>
				Look up a field
			</Typography>
			<Autocomplete
				freeSolo
				options={options}
				filterOptions={filterFields}
				groupBy={(option) =>
					FIGURES.some((f) => f.key === option)
						? "Figures"
						: option.includes(".")
							? "Healthcheck fields"
							: "Reported fields"
				}
				renderOption={({ key, ...props }, option) => (
					<li key={key} {...props}>
						{fieldLabel(option)}
					</li>
				)}
				inputValue={field ?? ""}
				onInputChange={(_, v) => setField(v || null)}
				renderInput={(params) => (
					<TextField
						{...params}
						size="small"
						label="Field"
						placeholder="e.g. uptimeSecs or diskspace.percent"
					/>
				)}
				sx={{ maxWidth: 360, mb: field ? 2 : 0 }}
			/>
			{field && groups.length > 0 && (
				<DistributionCard
					label={fieldLabel(field)}
					grain={grain}
					groups={groups}
					total={covered.length}
				/>
			)}
		</Paper>
	);
}

/// Match a figure on the name it presents under as well as on the field name
/// it resolves to, so "Tamanu release" finds `release`.
const filterFields = createFilterOptions<string>({
	stringify: (option) => `${option} ${fieldLabel(option)}`,
});

/// The values one machine takes on a crossing's axis.
///
/// A machine figure gives the box's own single value. An application figure
/// gives the set its applications give, so a box whose applications disagree
/// lands in each matching cell. `null` in place of a set means the machine is
/// dropped from the crossing: it has nothing on that axis to report, rather
/// than reporting nothing.
// spec: FIG#crossings
function axisValues(
	machine: Row,
	byMachine: Map<string, Row[]>,
	field: string,
	grain: Grain,
	pick: (row: Row) => unknown,
	isVersionTracked: (type: ApplicationType) => boolean,
): (string | null)[] | null {
	if (grain === "machine") return [bucket(pick(machine))];
	const applications = coveredBy(
		field,
		byMachine.get(machine.id) ?? [],
		isVersionTracked,
	);
	if (applications.length === 0) {
		return VERSION_TRACKED_FIELDS.has(field) ? null : [null];
	}
	return [...new Set(applications.map((a) => bucket(pick(a))))];
}

function CrossTab({
	machines,
	applications,
	keys,
	machineKeys,
}: {
	machines: Row[];
	applications: Row[];
	keys: string[];
	machineKeys: Set<string>;
}) {
	const fields = useMemo(
		() => [...new Set([...FIGURES.map((f) => f.key), ...keys])],
		[keys],
	);
	const [rowField, setRowField] = useState("postgresMajor");
	const [colField, setColField] = useState("release");
	const [cell, setCell] = useState<string | null>(null);
	const [sort, setSort] = useState<SortMode>("popularity");

	const isVersionTracked = useIsVersionTracked();
	const { rows, cols, counts, total } = useMemo(() => {
		const pickRow = picker(rowField);
		const pickCol = picker(colField);
		const rowGrain = grainOf(rowField, machineKeys);
		const colGrain = grainOf(colField, machineKeys);
		const byMachine = new Map<string, Row[]>();
		for (const application of applications) {
			const list = byMachine.get(application.machineId);
			if (list) list.push(application);
			else byMachine.set(application.machineId, [application]);
		}

		// A crossing counts machines whatever is on its axes, so an
		// application figure is read as the set of values the box's workloads
		// give it and the box lands in each of them.
		// spec: FIG#crossings
		const counts = new Map<string, Row[]>();
		const byRow = new Map<string | null, Row[]>();
		const byCol = new Map<string | null, Row[]>();
		let total = 0;
		for (const machine of machines) {
			const rowValues = axisValues(
				machine,
				byMachine,
				rowField,
				rowGrain,
				pickRow,
				isVersionTracked,
			);
			const colValues = axisValues(
				machine,
				byMachine,
				colField,
				colGrain,
				pickCol,
				isVersionTracked,
			);
			// A machine absent from either axis is dropped from the table
			// rather than placed in an unreported row, so a crossing never
			// implies it failed to report a figure it doesn't have.
			// spec: APP#versions
			if (rowValues === null || colValues === null) continue;
			total += 1;
			for (const value of rowValues) {
				const list = byRow.get(value);
				if (list) list.push(machine);
				else byRow.set(value, [machine]);
			}
			for (const value of colValues) {
				const list = byCol.get(value);
				if (list) list.push(machine);
				else byCol.set(value, [machine]);
			}
			for (const row of rowValues) {
				for (const col of colValues) {
					const key = `${row} ${col}`;
					const list = counts.get(key);
					if (list) list.push(machine);
					else counts.set(key, [machine]);
				}
			}
		}
		return {
			rows: orderGroups(groupsFrom(byRow), sort).map((g) => g.value),
			cols: orderGroups(groupsFrom(byCol), sort).map((g) => g.value),
			counts,
			total,
		};
	}, [machines, applications, rowField, colField, sort, machineKeys, isVersionTracked]);

	const label = (value: string | null) => value ?? "not reported";

	return (
		<Paper variant="outlined" sx={{ p: 2 }} role="group" aria-label="Cross two fields">
			<Stack
				direction="row"
				sx={{ alignItems: "center", justifyContent: "space-between", mb: 1 }}
			>
				<Stack direction="row" spacing={1} sx={{ alignItems: "baseline" }}>
					<Typography variant="subtitle2">Cross two fields</Typography>
					<Typography variant="caption" color="text.secondary">
						counting {total} {total === 1 ? UNIT.machine.one : UNIT.machine.many}
					</Typography>
				</Stack>
				<SortToggle mode={sort} onChange={setSort} />
			</Stack>
			<Stack direction="row" spacing={2} sx={{ mb: 2, flexWrap: "wrap" }} useFlexGap>
				<FieldPicker label="Rows" value={rowField} options={fields} onChange={setRowField} />
				<FieldPicker
					label="Columns"
					value={colField}
					options={fields}
					onChange={setColField}
				/>
			</Stack>
			<TableContainer sx={{ overflowX: "auto" }}>
				<Table size="small">
					<TableHead>
						<TableRow>
							<TableCell />
							{cols.map((col) => (
								<TableCell key={label(col)} align="right">
									{label(col)}
								</TableCell>
							))}
						</TableRow>
					</TableHead>
					<TableBody>
						{rows.map((row) => (
							<TableRow key={label(row)}>
								<TableCell component="th" scope="row">
									{label(row)}
								</TableCell>
								{cols.map((col) => {
									const key = `${row} ${col}`;
									const list = counts.get(key) ?? [];
									return (
										<TableCell
											key={label(col)}
											align="right"
											onClick={() =>
												list.length > 0 && setCell((c) => (c === key ? null : key))
											}
											sx={{
												cursor: list.length > 0 ? "pointer" : undefined,
												color: list.length === 0 ? "text.disabled" : undefined,
												bgcolor: cell === key ? "action.selected" : undefined,
											}}
										>
											{list.length || "·"}
										</TableCell>
									);
								})}
							</TableRow>
						))}
					</TableBody>
				</Table>
			</TableContainer>
			{cell && <TargetChips rows={counts.get(cell) ?? []} />}
		</Paper>
	);
}

function FieldPicker({
	label,
	value,
	options,
	onChange,
}: {
	label: string;
	value: string;
	options: string[];
	onChange: (v: string) => void;
}) {
	return (
		<Stack spacing={0.5}>
			<Typography variant="caption" color="text.secondary">
				{label}
			</Typography>
			<Select
				size="small"
				value={value}
				onChange={(e) => onChange(e.target.value)}
				inputProps={{ "aria-label": label }}
			>
				{options.map((option) => (
					<MenuItem key={option} value={option}>
						{fieldLabel(option)}
					</MenuItem>
				))}
			</Select>
		</Stack>
	);
}
