import {
	Alert,
	Box,
	Button,
	Dialog,
	DialogActions,
	DialogContent,
	DialogContentText,
	DialogTitle,
	LinearProgress,
	MenuItem,
	Stack,
	TextField,
	ToggleButton,
	ToggleButtonGroup,
	Typography,
} from "@mui/material";
import BuildOutlinedIcon from "@mui/icons-material/BuildOutlined";
import { useEffect, useState } from "react";
import { useApi, useApiAction } from "../api";
import { GradedAction } from "./GradedAction";
import KindChip from "./KindChip";
import ServerRankChip from "./ServerRankChip";
import TimeAgo from "./TimeAgo";
import type {
	MaintenanceGrain,
	MaintenanceScope,
	MaintenanceTargetChoice,
	MaintenanceWindow,
	ServerRank,
} from "../types";

const PRESETS = [1, 2, 4, 8];

/** `datetime-local` wants a local wall clock with no zone, which is what
 * the operator is thinking in when they say "back by six". */
function toLocalInput(at: Date): string {
	const pad = (n: number) => String(n).padStart(2, "0");
	return `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())}T${pad(at.getHours())}:${pad(at.getMinutes())}`;
}

function hoursFromNow(hours: number): string {
	return toLocalInput(new Date(Date.now() + hours * 3600_000));
}

function grainOf(scope: MaintenanceScope, id: string, rank?: ServerRank | null): MaintenanceGrain {
	if (scope === "application") return { kind: "application", application_id: id };
	if (scope === "machine") return { kind: "machine", machine_id: id };
	return rank
		? { kind: "environment", group_id: id, rank }
		: { kind: "group", group_id: id };
}

function grainOfWindow(window: MaintenanceWindow): MaintenanceGrain | null {
	if (window.application_id) return grainOf("application", window.application_id);
	if (window.machine_id) return grainOf("machine", window.machine_id);
	if (window.server_group_id) return grainOf("group", window.server_group_id, window.rank);
	return null;
}

function grainKey(grain: MaintenanceGrain): string {
	switch (grain.kind) {
		case "application":
			return `application:${grain.application_id}`;
		case "machine":
			return `machine:${grain.machine_id}`;
		case "environment":
			return `environment:${grain.group_id}:${grain.rank}`;
		case "group":
			return `group:${grain.group_id}`;
	}
}

/** The request body naming a grain as a window's target. */
function targetColumns(grain: MaintenanceGrain) {
	switch (grain.kind) {
		case "application":
			return { application_id: grain.application_id };
		case "machine":
			return { machine_id: grain.machine_id };
		case "environment":
			return { server_group_id: grain.group_id, rank: grain.rank };
		case "group":
			return { server_group_id: grain.group_id, rank: null };
	}
}

function explanation(grain: MaintenanceGrain): string {
	switch (grain.kind) {
		case "environment":
			return `Every check on the group's ${grain.rank} machines is suspended: nothing opens or joins an incident, and nothing notifies. The rest of the group stays watched.`;
		case "group":
			return "Every check on this group and its machines is suspended: nothing opens or joins an incident, and nothing notifies.";
		case "machine":
			return "Every check on this machine and the applications on it is suspended: nothing opens or joins an incident, and nothing notifies.";
		case "application":
			return "Every check on this application is suspended: nothing opens or joins an incident, and nothing notifies. The machine it runs on, and anything else on it, stays watched.";
	}
}

/** A grain as the picker shows it: what kind it is, and which. */
function GrainLabel({ choice }: { choice: MaintenanceTargetChoice }) {
	return (
		<Stack direction="row" spacing={1} sx={{ alignItems: "center", minWidth: 0 }}>
			<KindChip kind={choice.grain.kind} />
			{choice.grain.kind === "environment" ? (
				<ServerRankChip rank={choice.grain.rank} />
			) : (
				<Typography noWrap>{choice.label}</Typography>
			)}
		</Stack>
	);
}

/** Declare a maintenance window, or amend one. The declaration starts at the
 * grain it was offered over and can be retargeted to anything on that grain's
 * line of descent; amending a window can move it along its own. Offered over a
 * target that already has a window, it amends that window from the start. */
// spec: MNT#choosing-what-to-cover
export default function DeclareMaintenanceDialog({
	open,
	onClose,
	scope,
	id,
	rank,
	existing,
	prefill,
	offerLift,
	incidentId,
	fixed,
	upgradePlanId,
	onDone,
}: {
	open: boolean;
	onClose: () => void;
	scope: MaintenanceScope;
	id: string;
	/** With a group, narrows the window to the machines serving the group's
	 * applications at this rank. */
	rank?: ServerRank | null;
	/** The window being amended. */
	existing?: MaintenanceWindow | null;
	/** Starting values where something else knows them, such as an upgrade
	 * plan's window and note. */
	prefill?: { expectedEnd?: string; note?: string };
	/** Offer to end the work from in here, for a surface with no room to carry
	 * a lift of its own. Where the caller already shows one, this stays off so
	 * the same action is not in two places. */
	offerLift?: boolean;
	/** The incident this is offered from, so each choice says whether it
	 * covers every failing check in it. */
	incidentId?: string;
	/** Declares the work on one target, which the caller depends on. */
	fixed?: boolean;
	/** The upgrade plan this declares from. */
	upgradePlanId?: string;
	onDone: () => void;
}) {
	const declare = useApiAction("maintenance", "declare");
	const amend = useApiAction("maintenance", "amend");
	const lift = useApiAction("maintenance", "lift");
	const start: MaintenanceGrain =
		(existing && grainOfWindow(existing)) ?? grainOf(scope, id, rank);
	const [chosen, setChosen] = useState<MaintenanceGrain>(start);
	const [defaultEnd, setDefaultEnd] = useState(() => hoursFromNow(2));
	const [endsAt, setEndsAt] = useState<string | null>(null);
	const [note, setNote] = useState<string | null>(null);

	const targets = useApi(
		"maintenance",
		"targets",
		{
			start,
			incident_id: incidentId ?? null,
			window_id: existing?.id ?? null,
		},
		[open, grainKey(start), incidentId, existing?.id],
		{ skip: !open },
	);

	useEffect(() => {
		if (!open) return;
		setChosen(start);
		setDefaultEnd(prefill?.expectedEnd ? toLocalInput(new Date(prefill.expectedEnd)) : hoursFromNow(2));
		setEndsAt(null);
		setNote(null);
		declare.reset();
		amend.reset();
		lift.reset();
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [open, existing?.id]);

	const choices: MaintenanceTargetChoice[] =
		targets.status === "ok" ? targets.data.choices : [];
	const choiceOf = (grain: MaintenanceGrain) =>
		choices.find((choice) => grainKey(choice.grain) === grainKey(grain));
	const startChoice = choiceOf(start);
	const chosenChoice = choiceOf(chosen);

	// Offered over a target with a window of its own, the declaration amends
	// that window from the start, so choosing another grain moves it.
	// spec: MNT#moving-a-window
	const own = existing ?? startChoice?.window ?? null;
	const moving = own !== null && grainKey(chosen) !== grainKey(start);
	// Declaring onto someone else's window amends it, changing only what the
	// operator has explicitly changed.
	const joined = own === null ? (chosenChoice?.window ?? null) : null;
	const amending = own ?? joined;
	const fixedBecause =
		targets.status === "ok" && own !== null ? targets.data.fixed_because : null;
	const pickable = !fixed && !fixedBecause && choices.length > 1;

	const baseEnd = amending ? toLocalInput(new Date(amending.expected_end)) : defaultEnd;
	const baseNote = amending ? (amending.note ?? "") : (prefill?.note ?? "");
	const shownEnd = endsAt ?? baseEnd;
	const shownNote = note ?? baseNote;

	const submit = async () => {
		const at = new Date(shownEnd);
		if (Number.isNaN(at.getTime())) return;
		const trimmed = shownNote.trim() === "" ? null : shownNote.trim();
		try {
			if (amending) {
				await amend.call({
					id: amending.id,
					...(moving ? { target: chosen } : {}),
					...(endsAt !== null ? { expected_end: at.toISOString() } : {}),
					...(note !== null ? { note: trimmed } : {}),
				});
			} else {
				await declare.call({
					...targetColumns(chosen),
					expected_end: at.toISOString(),
					note: trimmed,
					upgrade_plan_id: upgradePlanId ?? null,
				});
			}
			onDone();
			onClose();
		} catch {
			/* surfaced below */
		}
	};

	const pending = declare.pending || amend.pending || lift.pending;
	const error = declare.error ?? amend.error ?? lift.error;
	return (
		<Dialog open={open} onClose={onClose} fullWidth maxWidth="sm">
			<DialogTitle>{amending ? "Amend maintenance" : "Declare maintenance"}</DialogTitle>
			<DialogContent>
				<Stack spacing={2} sx={{ pt: 1 }}>
					{targets.status === "loading" || targets.status === "idle" ? (
						<LinearProgress />
					) : targets.status === "error" ? (
						<Alert severity="error">{targets.error.message}</Alert>
					) : (
						<TextField
							select
							size="small"
							label="Covers"
							value={grainKey(chosen)}
							disabled={!pickable}
							helperText={fixedBecause ?? undefined}
							onChange={(event) => {
								const picked = choices.find(
									(choice) => grainKey(choice.grain) === event.target.value,
								);
								if (picked) setChosen(picked.grain);
							}}
							slotProps={{
								select: {
									renderValue: () =>
										chosenChoice ? <GrainLabel choice={chosenChoice} /> : null,
								},
							}}
							data-testid="maintenance-covers"
						>
							{choices.map((choice) => {
								// A target holds at most one window, so a window being moved
								// cannot land on a grain that has its own.
								const occupied =
									own !== null &&
									choice.window !== null &&
									choice.window.id !== own.id;
								return (
									<MenuItem
										key={grainKey(choice.grain)}
										value={grainKey(choice.grain)}
										disabled={occupied}
										sx={{ pl: 2 + choice.depth * 2, gap: 1 }}
										data-testid={`covers-${grainKey(choice.grain)}`}
									>
										<GrainLabel choice={choice} />
										<Box sx={{ flex: 1 }} />
										{occupied ? (
											<Typography variant="caption" color="text.secondary">
												has its own window
											</Typography>
										) : choice.covers_failures === false ? (
											<Typography variant="caption" color="warning.dark">
												not all failing checks
											</Typography>
										) : null}
									</MenuItem>
								);
							})}
						</TextField>
					)}
					{chosenChoice?.covers_failures === false && (
						<Alert severity="warning" data-testid="not-all-failing-checks">
							Doesn't cover all failing checks
						</Alert>
					)}
					{joined && (
						<Alert
							severity="info"
							icon={<BuildOutlinedIcon fontSize="inherit" />}
							data-testid="joins-window"
						>
							Already under maintenance, ending <TimeAgo timestamp={joined.expected_end} />
						</Alert>
					)}
					<DialogContentText>
						{explanation(chosen)} The window ends itself at the time below, and
						watching resumes a few minutes later once the reporters have been heard
						from.
					</DialogContentText>
					<ToggleButtonGroup
						size="small"
						exclusive
						value={null}
						onChange={(_, hours: number | null) => {
							if (hours) setEndsAt(hoursFromNow(hours));
						}}
					>
						{PRESETS.map((hours) => (
							<ToggleButton key={hours} value={hours}>
								{hours}h
							</ToggleButton>
						))}
					</ToggleButtonGroup>
					<TextField
						size="small"
						type="datetime-local"
						label="Expected to end"
						value={shownEnd}
						onChange={(e) => setEndsAt(e.target.value)}
						helperText={
							joined && endsAt !== null && endsAt !== baseEnd
								? `Currently ${new Date(joined.expected_end).toLocaleString()}`
								: undefined
						}
						slotProps={{ inputLabel: { shrink: true } }}
					/>
					<TextField
						size="small"
						label="What's being done"
						placeholder="Upgrading to 2.62"
						multiline
						minRows={2}
						value={shownNote}
						onChange={(e) => setNote(e.target.value)}
						helperText={
							joined && note !== null && note.trim() !== baseNote
								? baseNote
									? `Currently: ${baseNote}`
									: "Currently no note"
								: undefined
						}
					/>
					{error && <Alert severity="error">{error.message}</Alert>}
				</Stack>
			</DialogContent>
			<DialogActions>
				{offerLift && own && !moving && (
					<GradedAction calls="maintenance/lift">
						<Button
							disabled={pending}
							onClick={async () => {
								try {
									await lift.call({ id: own.id });
									onDone();
									onClose();
								} catch {
									/* surfaced above */
								}
							}}
							sx={{ mr: "auto" }}
						>
							Lift
						</Button>
					</GradedAction>
				)}
				<Button onClick={onClose}>Cancel</Button>
				<GradedAction calls={amending ? "maintenance/amend" : "maintenance/declare"}>
					<Button
						variant="contained"
						onClick={submit}
						disabled={pending || shownEnd === "" || targets.status !== "ok"}
					>
						{moving ? "Move" : amending ? "Amend" : "Declare"}
					</Button>
				</GradedAction>
			</DialogActions>
		</Dialog>
	);
}
