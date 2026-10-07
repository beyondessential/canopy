//! The pieces every backup schedule editor shares: the kind selector, the live
//! firing preview, the read-only summary, the layer chip, the next-backup
//! text, and the history beside an editor.
// spec: BKO#editing-schedules

import {
	Autocomplete,
	Box,
	Button,
	Chip,
	FormControl,
	FormControlLabel,
	Radio,
	RadioGroup,
	Stack,
	TextField,
	Tooltip,
	Typography,
} from "@mui/material";
import { useEffect, useState } from "react";
import { useApi } from "../api";
import { humanSeconds } from "../lib/humanDuration";
import type {
	EffectiveSchedule,
	NextBackup,
	Schedule,
	ScheduleLayer,
	SchedulePreview,
	ZoneUsed,
} from "../types";
import TimeAgo from "./TimeAgo";

const MIN_INTERVAL_HOURS = 1;

// Chrome leaves UTC out of the supported list, and it is the zone an operator
// most often means on purpose.
const ZONES = [
	"UTC",
	...Intl.supportedValuesOf("timeZone").filter((zone) => zone !== "UTC"),
];

// ── Draft ──────────────────────────────────────────────────────────────────

/** A schedule as it is being edited: every kind's fields at once, so switching
 * kind and back doesn't lose what was typed. */
export interface ScheduleDraft {
	kind: Schedule["kind"];
	hours: string;
	expression: string;
	zone: string;
}

export function draftFromSchedule(
	schedule: Schedule | null | undefined,
): ScheduleDraft {
	const draft: ScheduleDraft = {
		kind: "manual",
		hours: "6",
		expression: "",
		zone: "",
	};
	if (!schedule) return draft;
	draft.kind = schedule.kind;
	if (schedule.kind === "interval") draft.hours = String(schedule.seconds / 3600);
	if (schedule.kind === "cron") {
		draft.expression = schedule.expression;
		draft.zone = schedule.zone ?? "";
	}
	return draft;
}

/** The schedule a draft stands for, or null while it isn't one yet. */
export function scheduleFromDraft(draft: ScheduleDraft): Schedule | null {
	switch (draft.kind) {
		case "manual":
			return { kind: "manual" };
		case "interval": {
			const hours = Number(draft.hours);
			if (!Number.isFinite(hours) || hours < MIN_INTERVAL_HOURS) return null;
			return { kind: "interval", seconds: Math.round(hours * 3600) };
		}
		case "cron": {
			const expression = draft.expression.trim();
			if (expression === "") return null;
			return { kind: "cron", expression, zone: draft.zone || null };
		}
	}
}

function intervalProblem(draft: ScheduleDraft): string | null {
	if (draft.kind !== "interval") return null;
	const hours = Number(draft.hours);
	return Number.isFinite(hours) && hours >= MIN_INTERVAL_HOURS
		? null
		: `At least ${MIN_INTERVAL_HOURS} hour`;
}

// ── Summary ────────────────────────────────────────────────────────────────

function intervalText(seconds: number): string {
	const hours = seconds / 3600;
	if (hours === 1) return "Every hour";
	if (Number.isInteger(hours)) return `Every ${hours} hours`;
	return `Every ${humanSeconds(seconds)}`;
}

/** Where a cron expression is read, in words. */
function zonePhrase(schedule: Schedule, used?: ZoneUsed | null): string {
	if (schedule.kind !== "cron") return "";
	if (schedule.zone) return `in ${schedule.zone}`;
	if (!used) return "in each machine's own timezone";
	if (used.source === "machine") return `in ${used.name}, the machine's timezone`;
	return "in UTC";
}

/** A schedule as one line of plain text. */
export function scheduleSummary(
	schedule: Schedule,
	used?: ZoneUsed | null,
): string {
	switch (schedule.kind) {
		case "manual":
			return "Manual only";
		case "interval":
			return intervalText(schedule.seconds);
		case "cron":
			return `Cron \`${schedule.expression}\` ${zonePhrase(schedule, used)}`;
	}
}

/** A schedule as one line, with the cron expression in monospace. */
export function ScheduleSummary({
	schedule,
	zone,
}: {
	schedule: Schedule;
	/** The zone a machine reads the schedule in, when it is one machine's. */
	zone?: ZoneUsed | null;
}) {
	if (schedule.kind !== "cron") return <>{scheduleSummary(schedule)}</>;
	return (
		<>
			Cron{" "}
			<Box component="code" sx={{ fontFamily: "monospace" }}>
				{schedule.expression}
			</Box>{" "}
			{zonePhrase(schedule, zone)}
		</>
	);
}

const LAYER_LABEL: Record<ScheduleLayer, string> = {
	machine: "machine override",
	group: "group override",
	fleet: "fleet default",
};

/** Which layer a schedule comes from. An override is filled, the default is not. */
export function ScheduleLayerChip({
	layer,
}: {
	layer: ScheduleLayer | null | undefined;
}) {
	if (!layer) return null;
	const override = layer !== "fleet";
	return (
		<Chip
			size="small"
			label={LAYER_LABEL[layer]}
			color={override ? "secondary" : "default"}
			variant={override ? "filled" : "outlined"}
		/>
	);
}

const ZONE_FLAG: Partial<Record<ZoneUsed["source"], [string, string]>> = {
	unreported_utc: [
		"No timezone reported",
		"This machine reports no timezone, so a schedule read in the machine's own timezone reads in UTC.",
	],
	unrecognised_utc: [
		"Unrecognised timezone",
		"Canopy doesn't recognise the timezone this machine reports, so a schedule read in the machine's own timezone reads in UTC.",
	],
};

/** Says so when UTC is a fallback rather than a choice. */
export function ZoneFlag({ zone }: { zone: ZoneUsed | null | undefined }) {
	const flag = zone ? ZONE_FLAG[zone.source] : undefined;
	if (!flag) return null;
	return (
		<Tooltip title={flag[1]}>
			<Chip
				size="small"
				color="warning"
				variant="outlined"
				label={`${flag[0]}, reads in UTC`}
			/>
		</Tooltip>
	);
}

/** What a machine follows for a type: the schedule, where it comes from, and
 * the zone it is read in. */
export function EffectiveScheduleLine({
	effective,
}: {
	effective: EffectiveSchedule;
}) {
	return (
		<Stack
			direction="row"
			spacing={1}
			sx={{ alignItems: "center", flexWrap: "wrap", rowGap: 0.5 }}
		>
			<Typography variant="body2">
				<ScheduleSummary schedule={effective.schedule} zone={effective.zone} />
			</Typography>
			<ScheduleLayerChip layer={effective.layer} />
			<ZoneFlag zone={effective.zone} />
		</Stack>
	);
}

// ── Times ──────────────────────────────────────────────────────────────────

/** An instant on the wall clock of the zone it is read in. */
export function formatInZone(iso: string, zone: string | null | undefined) {
	try {
		return new Intl.DateTimeFormat(undefined, {
			timeZone: zone ?? undefined,
			weekday: "short",
			day: "numeric",
			month: "short",
			hour: "2-digit",
			minute: "2-digit",
			timeZoneName: "short",
		}).format(new Date(iso));
	} catch {
		return iso;
	}
}

function When({ at, zone }: { at: string; zone?: string | null }) {
	if (!zone) return <TimeAgo timestamp={at} />;
	return (
		<>
			{formatInZone(at, zone)} (<TimeAgo timestamp={at} />)
		</>
	);
}

/** When a machine's backup of a type is next expected. */
export function NextBackupText({
	next,
	zone,
}: {
	next: NextBackup | null | undefined;
	/** The zone a cron schedule is read in, so a firing reads on its own clock. */
	zone?: ZoneUsed | null;
}) {
	if (!next) {
		return (
			<Typography variant="body2" color="text.secondary" component="span">
				—
			</Typography>
		);
	}
	switch (next.kind) {
		case "manual":
			return <span>manual</span>;
		case "due_now":
			return <span>due now</span>;
		case "due_until":
			return (
				<span>
					due until <When at={next.at} zone={zone?.name} />
				</span>
			);
		case "at":
			return (
				<span>
					<When at={next.at} zone={zone?.name} />
				</span>
			);
		case "unreadable":
			return (
				<Typography variant="body2" color="error" component="span">
					unreadable schedule
				</Typography>
			);
	}
}

// ── Preview ────────────────────────────────────────────────────────────────

/** What a schedule being edited applies to, for the preview: one machine's
 * override, a group's, or (neither) the fleet default. */
export interface ScheduleScope {
	type: string;
	machineId?: string;
	groupId?: string;
}

const PREVIEW_DEBOUNCE_MS = 400;

function useDebounced<T>(value: T, ms: number): T {
	const [held, setHeld] = useState(value);
	useEffect(() => {
		const id = window.setTimeout(() => setHeld(value), ms);
		return () => window.clearTimeout(id);
	}, [value, ms]);
	return held;
}

/** Everything an editor needs from the schedule being edited. */
export interface ScheduleState {
	draft: ScheduleDraft;
	setDraft: (draft: ScheduleDraft) => void;
	scope: ScheduleScope;
	/** The schedule to save, when the draft is one and nothing refuses it. */
	schedule: Schedule | null;
	/** Why the server would refuse the expression, once it has said. */
	refusal: string | null;
	/** The preview call itself failed. */
	previewError: string | null;
	previewMachines: PreviewRows | null;
	/** A cron expression is typed and the preview hasn't caught up with it. */
	checking: boolean;
}

type PreviewRows = SchedulePreview["machines"];

/** The draft, and the debounced preview of it while it is a cron expression. */
export function useScheduleState(
	initial: Schedule | null | undefined,
	scope: ScheduleScope,
): ScheduleState {
	const [draft, setDraft] = useState(() => draftFromSchedule(initial));
	const complete = scheduleFromDraft(draft);
	const cron = complete?.kind === "cron" ? complete : null;
	const typeKnown = scope.type.trim() !== "";

	const key = cron && typeKnown ? JSON.stringify([cron, scope]) : "";
	const settledKey = useDebounced(key, PREVIEW_DEBOUNCE_MS);
	const preview = useApi(
		"backups",
		"schedule_preview",
		{
			type: scope.type,
			schedule: complete,
			machine_id: scope.machineId ?? null,
			group_id: scope.groupId ?? null,
		},
		[settledKey],
		{ skip: settledKey === "" },
	);

	const checking = key !== "" && (key !== settledKey || preview.status === "loading");
	const answered = key !== "" && key === settledKey && preview.status === "ok";
	const refusal = answered && preview.status === "ok" ? preview.data.refusal : null;

	return {
		draft,
		setDraft,
		scope,
		schedule:
			complete && !intervalProblem(draft) && !refusal && !checking
				? complete
				: null,
		refusal,
		previewError:
			key !== "" && key === settledKey && preview.status === "error"
				? preview.error.message
				: null,
		previewMachines:
			answered && preview.status === "ok" ? preview.data.machines : null,
		checking,
	};
}

function Preview({ machines }: { machines: PreviewRows }) {
	if (machines.length === 0) return null;
	return (
		<Stack spacing={1} data-testid="schedule-preview">
			<Typography variant="caption" color="text.secondary">
				Next firings
			</Typography>
			{machines.map((m) => (
				<Box key={`${m.machine_id ?? "none"}:${m.zone.name}`}>
					<Stack
						direction="row"
						spacing={1}
						sx={{ alignItems: "center", flexWrap: "wrap", rowGap: 0.5 }}
					>
						<Typography variant="body2">
							<strong>{m.machine_name ?? "Example machine"}</strong> ·{" "}
							{m.zone.name}
						</Typography>
						<ZoneFlag zone={m.zone} />
					</Stack>
					<Box
						component="ul"
						sx={{ m: 0, pl: 3, typography: "body2", color: "text.secondary" }}
					>
						{m.firings.map((at) => (
							<li key={at}>{formatInZone(at, m.zone.name)}</li>
						))}
					</Box>
				</Box>
			))}
		</Stack>
	);
}

// ── Editor ─────────────────────────────────────────────────────────────────

const KIND_LABEL: Record<Schedule["kind"], string> = {
	manual: "Manual only",
	interval: "Interval",
	cron: "Cron",
};

/** The three-way kind selector with the fields of the chosen kind, and, for a
 * cron expression, the reason it would be refused and its next firings. */
export function ScheduleEditor({
	state,
	disabled = false,
}: {
	state: ScheduleState;
	disabled?: boolean;
}) {
	const { draft, setDraft, scope } = state;
	const patch = (change: Partial<ScheduleDraft>) =>
		setDraft({ ...draft, ...change });
	const hoursProblem = intervalProblem(draft);

	return (
		<Stack spacing={1.5}>
			<FormControl disabled={disabled}>
				<RadioGroup
					row
					aria-label="Schedule kind"
					value={draft.kind}
					onChange={(e) => patch({ kind: e.target.value as Schedule["kind"] })}
				>
					{(Object.keys(KIND_LABEL) as Schedule["kind"][]).map((kind) => (
						<FormControlLabel
							key={kind}
							value={kind}
							control={<Radio size="small" />}
							label={KIND_LABEL[kind]}
						/>
					))}
				</RadioGroup>
			</FormControl>
			{draft.kind === "interval" && (
				<TextField
					label="Back up every (hours)"
					type="number"
					size="small"
					value={draft.hours}
					onChange={(e) => patch({ hours: e.target.value })}
					disabled={disabled}
					error={hoursProblem != null}
					helperText={hoursProblem ?? undefined}
					slotProps={{ htmlInput: { min: MIN_INTERVAL_HOURS, step: 1 } }}
					sx={{ width: 200 }}
				/>
			)}
			{draft.kind === "cron" && (
				<>
					<TextField
						label="Cron expression"
						size="small"
						value={draft.expression}
						onChange={(e) => patch({ expression: e.target.value })}
						disabled={disabled}
						error={state.refusal != null}
						helperText={
							state.refusal ??
							"Minute, hour, day of month, month, day of week. H picks a stable slot per machine."
						}
						slotProps={{
							htmlInput: { spellCheck: false, autoCapitalize: "off" },
						}}
						sx={{ maxWidth: 420, "& input": { fontFamily: "monospace" } }}
					/>
					<Autocomplete<string, false, false, false>
						size="small"
						disabled={disabled}
						options={ZONES}
						value={draft.zone === "" ? null : draft.zone}
						onChange={(_, zone) => patch({ zone: zone ?? "" })}
						renderInput={(params) => (
							<TextField
								{...params}
								label="Timezone"
								helperText={
									scope.machineId
										? "Empty reads it in the machine's own timezone"
										: "Empty reads it in each machine's own timezone"
								}
							/>
						)}
						sx={{ maxWidth: 420 }}
					/>
					{state.previewError && (
						<Typography variant="caption" color="error">
							{state.previewError}
						</Typography>
					)}
					{scope.type.trim() === "" && (
						<Typography variant="caption" color="text.secondary">
							Enter the backup type to preview its firings.
						</Typography>
					)}
					{state.previewMachines && !state.refusal && (
						<Preview machines={state.previewMachines} />
					)}
				</>
			)}
		</Stack>
	);
}

// ── History ────────────────────────────────────────────────────────────────

const HISTORY_SHOWN = 5;

/** Every set and clear of one layer, newest first: what it became, who did it,
 * and when. Shown beside the editor for that layer. */
export function ScheduleHistory({
	layer,
	type,
	groupId,
	machineId,
	reloadKey = 0,
}: {
	layer: ScheduleLayer;
	type: string;
	groupId?: string;
	machineId?: string;
	/** Change to fetch the history again, after a save. */
	reloadKey?: number;
}) {
	const history = useApi(
		"backups",
		"schedule_history",
		{
			layer,
			type,
			group_id: groupId ?? null,
			machine_id: machineId ?? null,
		},
		[layer, type, groupId, machineId, reloadKey],
		{ skip: type.trim() === "" },
	);
	const [all, setAll] = useState(false);

	if (history.status !== "ok" || history.data.length === 0) return null;
	const shown = all ? history.data : history.data.slice(0, HISTORY_SHOWN);

	return (
		<Stack spacing={0.5} data-testid="schedule-history">
			<Typography variant="caption" color="text.secondary">
				History
			</Typography>
			<Box component="ul" sx={{ m: 0, pl: 3, typography: "body2" }}>
				{shown.map((change) => (
					<li key={`${change.changed_at}:${change.changed_by ?? ""}`}>
						{change.schedule ? (
							<ScheduleSummary schedule={change.schedule} />
						) : (
							"Cleared"
						)}
						<Typography
							component="span"
							variant="body2"
							color="text.secondary"
						>
							{" "}
							·{" "}
							{Date.parse(change.changed_at) <= 0 ? (
								"before changes were recorded"
							) : (
								<>
									{change.changed_by ? `${change.changed_by}, ` : ""}
									<TimeAgo timestamp={change.changed_at} />
								</>
							)}
						</Typography>
					</li>
				))}
			</Box>
			{history.data.length > HISTORY_SHOWN && (
				<Box>
					<Button size="small" onClick={() => setAll((a) => !a)}>
						{all ? "Show fewer" : `Show all ${history.data.length}`}
					</Button>
				</Box>
			)}
		</Stack>
	);
}
