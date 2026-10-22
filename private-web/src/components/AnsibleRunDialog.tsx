import CheckCircleOutlineIcon from "@mui/icons-material/CheckCircleOutlined";
import ContentCopyIcon from "@mui/icons-material/ContentCopy";
import ExpandLessIcon from "@mui/icons-material/ExpandLess";
import ExpandMoreIcon from "@mui/icons-material/ExpandMore";
import PlayArrowIcon from "@mui/icons-material/PlayArrow";
import {
	Alert,
	Autocomplete,
	Box,
	Button,
	ButtonBase,
	Chip,
	CircularProgress,
	Collapse,
	Dialog,
	DialogActions,
	DialogContent,
	DialogTitle,
	IconButton,
	LinearProgress,
	MenuItem,
	Stack,
	TextField,
	ToggleButton,
	ToggleButtonGroup,
	Tooltip,
	Typography,
} from "@mui/material";
import { useEffect, useMemo, useState } from "react";
import { ApiError, callApi, useApi, useApiAction } from "../api";
import type {
	AnsibleLease,
	CatalogueView,
	LimitPreview,
	RunMember,
	ServerRank,
} from "../types";
import { GradedAction } from "./GradedAction";

/** What Canopy says a lease waits for its run to start, until it has said. */
const PENDING_FALLBACK_SECS = 300;
const NOTHING = "";

type Playbook = CatalogueView["playbooks"][number];

type Stage = "configure" | "waiting" | "started" | "timeout" | "gone";

/// How an operator starts a run: the playbook and what it acts on, then the
/// lease the run takes over, handed to it in the command shown.
// spec: ANSRUN#presentation
export default function AnsibleRunDialog({
	open,
	onClose,
	groupId,
	rank,
	catalogue,
	preset,
	playbooks,
	takeOver,
	holder,
	onChanged,
}: {
	open: boolean;
	onClose: () => void;
	groupId: string;
	rank: ServerRank;
	catalogue: CatalogueView;
	/** The playbook run button this dialog was opened from, or null for Run…. */
	preset: "install" | "upgrade" | null;
	/** The playbooks Run… offers. */
	playbooks: ReadonlyArray<Playbook>;
	/** Whether to start by taking over what another operator holds. */
	takeOver: boolean;
	/** Whoever holds the environment, where someone does. */
	holder: string | null;
	onChanged: () => void;
}) {
	const [stage, setStage] = useState<Stage>("configure");
	const [playbookName, setPlaybookName] = useState<string>("");
	const [tokens, setTokens] = useState<string[]>([]);
	const [tags, setTags] = useState<string[]>([]);
	const [skipTags, setSkipTags] = useState<string[]>([]);
	const [extra, setExtra] = useState("");
	const [assignments, setAssignments] = useState<Record<string, string>>({});
	const [advanced, setAdvanced] = useState(false);
	const [vcs, setVcs] = useState<"git" | "jj">(
		() => (localStorage.getItem("run-vcs") === "jj" ? "jj" : "git"),
	);
	const [lease, setLease] = useState<AnsibleLease | null>(null);
	const [now, setNow] = useState(() => Date.now());
	const [copied, setCopied] = useState(false);
	const [preview, setPreview] = useState<LimitPreview | null>(null);
	const take = useApiAction("ansible", "take_lease");
	const release = useApiAction("ansible", "release_lease");
	const hold = useApiAction("maintenance", "hold");
	const [windowId, setWindowId] = useState<string | null>(null);
	const [holding, setHolding] = useState(false);
	const [holdEnd, setHoldEnd] = useState("");
	const [held, setHeld] = useState<string | null>(null);

	const options = useApi(
		"ansible",
		"run_options",
		{ server_group_id: groupId, rank },
		[groupId, rank, open],
		{ skip: !open },
	);
	const members: RunMember[] =
		options.status === "ok" ? options.data.members : [];
	const hasCentral =
		options.status === "ok" && options.data.has_central;
	const pendingSecs =
		options.status === "ok"
			? options.data.pending_for_secs
			: PENDING_FALLBACK_SECS;

	const offered = useMemo(
		() =>
			preset
				? playbooks.filter((playbook) => playbook.preset === preset)
				: playbooks,
		[playbooks, preset],
	);
	const playbook =
		offered.find((candidate) => candidate.name === playbookName) ?? offered[0];
	// The catalogue says what a playbook is run as, so the run takes that and
	// not what the operator labels it: an upgrade is not started as a configure.
	const effectiveIntent = playbook?.intent ?? "configure";

	// Reset what a previous opening left behind.
	// biome-ignore lint/correctness/useExhaustiveDependencies: opening is the trigger.
	useEffect(() => {
		if (!open) return;
		setStage("configure");
		setPlaybookName("");
		setTokens([]);
		setTags([]);
		setSkipTags([]);
		setExtra("");
		setAssignments({});
		setLease(null);
		setPreview(null);
		setWindowId(null);
		setHolding(false);
		setHeld(null);
		take.reset();
		release.reset();
		hold.reset();
	}, [open, preset]);

	const configures = (type: string) =>
		!playbook?.applications || playbook.applications.includes(type);
	const empties = members.filter((member) => member.application_types.length === 0);
	const assignable = (playbook?.applications ?? []).length > 0 ? empties : [];

	// The default for a machine nothing is on: a facility, where the environment
	// already holds a central. Otherwise the operator chooses.
	// spec: ANSRUN#machines-with-nothing-on-them
	useEffect(() => {
		if (!playbook) return;
		setAssignments((current) => {
			const next = { ...current };
			for (const member of assignable) {
				if (next[member.id] === undefined && hasCentral && configures("tamanu-facility")) {
					next[member.id] = "tamanu-facility";
				}
			}
			return next;
		});
		// biome-ignore lint/correctness/useExhaustiveDependencies: derived from the playbook and members.
	}, [playbook?.name, members.length, hasCentral]);

	const limit = tokens.join(":");
	useEffect(() => {
		if (!open || stage !== "configure" || options.status !== "ok") return;
		let current = true;
		const handle = window.setTimeout(() => {
			callApi("ansible", "limit_preview", {
				server_group_id: groupId,
				rank,
				limit: limit === "" ? "all" : limit,
				assignments: assignmentList(assignments),
			})
				.then((result) => {
					if (current) setPreview(result);
				})
				.catch(() => {
					if (current) setPreview(null);
				});
		}, 250);
		return () => {
			current = false;
			window.clearTimeout(handle);
		};
	}, [open, stage, options.status, groupId, rank, limit, assignments]);

	const chosen = assignable.every((member) => assignments[member.id] !== undefined);
	const centrals = Object.values(assignments).filter((v) => v === "tamanu-central").length;
	const canStart =
		playbook !== undefined &&
		chosen &&
		preview !== null &&
		preview.error === null &&
		preview.matches.length > 0 &&
		!take.pending;

	const start = (takeOver: boolean) => {
		if (!playbook) return;
		take
			.call({
				server_group_id: groupId,
				rank,
				intent: effectiveIntent,
				playbook: playbook.name,
				limit: limit === "" ? null : limit,
				tags,
				skip_tags: skipTags,
				assignments: assignmentList(assignments),
				take_over: takeOver,
			})
			.then((taken) => {
				setLease(taken as AnsibleLease);
				setNow(Date.now());
				setStage("waiting");
				onChanged();
			})
			.catch(() => {
				// Shown as the refusal, and the run state is read again so a lease
				// taken meanwhile is offered for taking over.
				onChanged();
			});
	};

	// While the lease waits, ask whether the run has read it.
	useEffect(() => {
		if (stage !== "waiting" || lease === null) return;
		const tick = window.setInterval(() => setNow(Date.now()), 1000);
		const poll = window.setInterval(() => {
			callApi("ansible", "run_state", { server_group_id: groupId, rank })
				.then((state) => {
					const held = state.lease;
					if (held && held.id === lease.id) {
						if (held.started_at) {
							setLease(held);
							setWindowId(
								state.window?.id ?? null,
							);
							setStage("started");
							onChanged();
						}
					} else {
						setStage(
							Date.now() >= Date.parse(lease.taken_at) + pendingSecs * 1000
								? "timeout"
								: "gone",
						);
						onChanged();
					}
				})
				.catch(() => {
					// A poll that fails is tried again.
				});
		}, 2500);
		return () => {
			window.clearInterval(tick);
			window.clearInterval(poll);
		};
	}, [stage, lease, groupId, rank, onChanged]);

	const command = useMemo(
		() =>
			lease && playbook
				? buildCommand({
						vcs,
						origin: window.location.origin,
						leaseId: lease.id,
						playbook: playbook.name,
						limit,
						tags,
						skipTags,
						extra,
					})
				: null,
		[lease, playbook, vcs, limit, tags, skipTags, extra],
	);

	const copy = async () => {
		if (!command) return;
		try {
			await navigator.clipboard.writeText(command.copied);
			setCopied(true);
			window.setTimeout(() => setCopied(false), 2000);
		} catch {
			/* the command is on the page */
		}
	};

	const giveUp = () => {
		if (!lease) return;
		// Released only while the run has not read the lease, since a run that
		// has is under way and releasing would end the maintenance it declared.
		release
			.call({ lease_id: lease.id, unless_started: true })
			.then(
				() => true,
				(error) => {
					if (error instanceof ApiError && error.status === 409) return false;
					// Already gone, which is what was asked for.
					return error instanceof ApiError && error.status === 404;
				},
			)
			.then(async (done) => {
				if (done) {
					onChanged();
					onClose();
					return;
				}
				const state = await callApi("ansible", "run_state", { server_group_id: groupId, rank });
				const held = state.lease;
				if (held && held.id === lease.id && held.started_at) {
					setLease(held);
					setWindowId(state.window?.id ?? null);
					setStage("started");
				}
				onChanged();
			})
			.catch(() => {
				// The run state is read again on the next poll.
			});
	};

	// A refusal because the run has started, or because the lease is already
	// gone, is the answer to giving up rather than a failure of it.
	const giveUpFailed =
		release.error &&
		!(release.error instanceof ApiError && [404, 409].includes(release.error.status))
			? release.error
			: null;

	const remaining = lease
		? Math.max(0, Date.parse(lease.taken_at) + pendingSecs * 1000 - now)
		: 0;
	const title = preset
		? `${preset === "install" ? "Install" : "Upgrade"} ${rank}`
		: `Run on ${rank}`;
	const refusal = take.error?.message ?? null;
	// Whether another operator holds the environment is what the run state says,
	// refreshed when a take is refused, never what the refusal happens to say.
	const heldByOther = holder !== null;

	return (
		<Dialog open={open} onClose={stage === "waiting" ? undefined : onClose} fullWidth maxWidth="md">
			<DialogTitle>
				{title}{" "}
				{playbook && preset && (
					<Typography component="span" sx={{ fontFamily: "monospace" }} color="text.secondary">
						{playbook.name}
					</Typography>
				)}
			</DialogTitle>
			<DialogContent
				sx={{ display: "flex", flexDirection: "column", gap: 2 }}
				data-testid="run-dialog"
			>
				{stage === "configure" && (
					<>
						{options.status === "loading" && <LinearProgress />}
						{refusal && (
							<Alert
								severity="error"
								data-testid="run-refusal"
								action={
									heldByOther ? (
										<GradedAction calls="ansible/take_lease" action="Take over the lease">
											<Button
												color="inherit"
												size="small"
												onClick={() => start(true)}
												data-testid="run-take-over"
											>
												Take over
											</Button>
										</GradedAction>
									) : undefined
								}
							>
								{refusal}
							</Alert>
						)}
						{!preset && (
							<Stack direction="row" spacing={2} sx={{ alignItems: "center" }}>
								<TextField
									select
									size="small"
									label="Playbook"
									value={playbook?.name ?? ""}
									onChange={(event) => {
										setPlaybookName(event.target.value);
									}}
									sx={{ minWidth: 320 }}
									slotProps={{ htmlInput: { style: { fontFamily: "monospace" } } }}
									data-testid="run-playbook"
								>
									{offered.map((candidate) => (
										<MenuItem key={candidate.name} value={candidate.name}>
											<Box>
												<Typography sx={{ fontFamily: "monospace" }} variant="body2">
													{candidate.name}
												</Typography>
												{candidate.description && (
													<Typography variant="caption" color="text.secondary">
														{candidate.description}
													</Typography>
												)}
											</Box>
										</MenuItem>
									))}
								</TextField>
								<ToggleButtonGroup
									size="small"
									exclusive
									disabled
									value={effectiveIntent}
								>
									<ToggleButton value="configure">Configure</ToggleButton>
									<ToggleButton value="upgrade">Upgrade</ToggleButton>
								</ToggleButtonGroup>
							</Stack>
						)}

						{assignable.length > 0 && (
							<Box data-testid="run-assignments">
								<Typography variant="subtitle2" gutterBottom>
									Machines with nothing on them
								</Typography>
								<Stack spacing={1}>
									{assignable.map((member) => (
										<Stack
											key={member.id}
											direction="row"
											spacing={2}
											sx={{ alignItems: "center" }}
											data-testid={`run-assign-${member.name}`}
										>
											<Typography sx={{ minWidth: 220 }}>{member.name}</Typography>
											<TextField
												select
												size="small"
												label="Install as"
												value={assignments[member.id] ?? "unset"}
												error={assignments[member.id] === undefined}
												onChange={(event) =>
													setAssignments((current) => ({
														...current,
														[member.id]:
															event.target.value === "unset" ? NOTHING : event.target.value,
													}))
												}
												sx={{ minWidth: 220 }}
											>
												<MenuItem value="unset" disabled>
													Choose
												</MenuItem>
												<MenuItem value={NOTHING}>Nothing</MenuItem>
												{(playbook?.applications ?? [])
													.filter(
														(type) =>
															type !== "tamanu-central" ||
															(!hasCentral &&
																(centrals === 0 ||
																	assignments[member.id] === "tamanu-central")),
													)
													.map((type) => (
														<MenuItem key={type} value={type}>
															{typeLabel(type)}
														</MenuItem>
													))}
											</TextField>
										</Stack>
									))}
								</Stack>
							</Box>
						)}

						<Box>
							<Autocomplete
								multiple
								freeSolo
								size="small"
								options={[...(preview?.groups ?? []), ...(preview?.hosts ?? [])]}
								value={tokens}
								onChange={(_, next) => setTokens(next.map(String))}
								renderInput={(params) => (
									<TextField
										{...params}
										label="Limit"
										placeholder={tokens.length === 0 ? "all" : undefined}
										slotProps={{
											...params.slotProps,
											htmlInput: {
												...params.slotProps.htmlInput,
												style: { fontFamily: "monospace" },
												"data-testid": "run-limit",
											},
										}}
									/>
								)}
							/>
							<Typography
								variant="caption"
								color={preview?.error ? "error" : "text.secondary"}
								sx={{ display: "block", mt: 0.5 }}
								data-testid="run-limit-matches"
							>
								{preview === null
									? " "
									: preview.error
										? preview.error
										: preview.matches.length === 0
											? "Matches no machine"
											: `Matches ${preview.matches.map((m) => m.host_name).join(", ")}`}
							</Typography>
						</Box>

						<Stack direction="row" sx={{ alignItems: "center", justifyContent: "space-between" }}>
							<ButtonBase
								onClick={() => setAdvanced((a) => !a)}
								sx={{ gap: 0.5, borderRadius: 1 }}
								aria-expanded={advanced}
							>
								{advanced ? <ExpandLessIcon fontSize="small" /> : <ExpandMoreIcon fontSize="small" />}
								<Typography variant="body2">Advanced</Typography>
							</ButtonBase>
							<ToggleButtonGroup
								size="small"
								exclusive
								value={vcs}
								onChange={(_, next) => {
									if (!next) return;
									setVcs(next);
									localStorage.setItem("run-vcs", next);
								}}
							>
								<ToggleButton value="git">git</ToggleButton>
								<ToggleButton value="jj">jj</ToggleButton>
							</ToggleButtonGroup>
						</Stack>
						<Collapse in={advanced} unmountOnExit>
							<Stack spacing={2}>
								<TagField
									label="Tags"
									options={playbook?.tags ?? []}
									notes={catalogue.tags}
									value={tags}
									onChange={setTags}
								/>
								<TagField
									label="Skip tags"
									options={playbook?.tags ?? []}
									notes={catalogue.tags}
									value={skipTags}
									onChange={setSkipTags}
								/>
								<TextField
									size="small"
									label="Extra arguments"
									placeholder="-vv, --check…"
									value={extra}
									onChange={(event) => setExtra(event.target.value)}
									slotProps={{ htmlInput: { style: { fontFamily: "monospace" } } }}
								/>
							</Stack>
						</Collapse>
					</>
				)}

				{stage === "waiting" && command && (
					<>
						<Box
							component="pre"
							data-testid="run-command"
							sx={{
								m: 0,
								p: 1.5,
								borderRadius: 1,
								bgcolor: "action.hover",
								overflow: "auto",
								fontSize: "0.85em",
								fontFamily: "monospace",
								whiteSpace: "pre-wrap",
								wordBreak: "break-all",
								position: "relative",
							}}
						>
							<Box component="span" sx={{ color: "text.secondary" }} data-testid="run-command-checkout">
								{command.checkout}
							</Box>
							{"\n"}
							<span data-testid="run-command-copied">{command.copied}</span>
							<Tooltip title={copied ? "Copied" : "Copy the command"}>
								<IconButton
									size="small"
									onClick={copy}
									aria-label="Copy the command"
									sx={{ position: "absolute", top: 4, right: 4 }}
								>
									<ContentCopyIcon fontSize="small" />
								</IconButton>
							</Tooltip>
						</Box>
						<Stack direction="row" spacing={1} sx={{ alignItems: "center" }} data-testid="run-waiting">
							<CircularProgress size={18} />
							<Typography>Waiting for the run to start</Typography>
							<Typography
								variant="body2"
								color="text.secondary"
								sx={{ ml: "auto", fontVariantNumeric: "tabular-nums" }}
								data-testid="run-clock"
							>
								{formatClock(remaining)}
							</Typography>
						</Stack>
					</>
				)}

				{stage === "started" && (
					<Stack spacing={2} data-testid="run-started">
						<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
							<CheckCircleOutlineIcon color="success" />
							<Typography>
								Running since{" "}
								{lease?.started_at
									? new Date(lease.started_at).toLocaleTimeString([], {
											hour: "2-digit",
											minute: "2-digit",
										})
									: ""}
							</Typography>
							<Chip size="small" label="Maintenance" color="warning" variant="outlined" />
						</Stack>
						{held && (
							<Typography variant="body2" color="text.secondary" data-testid="run-held">
								Maintenance is held until {new Date(held).toLocaleString()}, whatever
								becomes of the lease.
							</Typography>
						)}
						{holding && (
							<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
								<TextField
									size="small"
									type="datetime-local"
									label="Work is expected to finish"
									value={holdEnd}
									onChange={(event) => setHoldEnd(event.target.value)}
									slotProps={{ inputLabel: { shrink: true } }}
								/>
								<GradedAction calls="maintenance/hold" action="Hold maintenance">
									<Button
										variant="outlined"
										size="small"
										disabled={holdEnd === "" || hold.pending || windowId === null}
										data-testid="run-hold-confirm"
										onClick={() =>
											windowId &&
											hold
												.call({ id: windowId, expected_end: new Date(holdEnd).toISOString() })
												.then(() => {
													setHeld(new Date(holdEnd).toISOString());
													setHolding(false);
													onChanged();
												})
												.catch(() => {
													// Surfaced below.
												})
										}
									>
										Hold
									</Button>
								</GradedAction>
							</Stack>
						)}
						{hold.error && <Alert severity="warning">{hold.error.message}</Alert>}
					</Stack>
				)}

				{stage === "waiting" && giveUpFailed && (
					<Alert severity="warning" data-testid="run-give-up-failed">
						{giveUpFailed.message}
					</Alert>
				)}

				{stage === "gone" && (
					<Alert severity="warning" data-testid="run-gone">
						The lease is no longer held: it was released, or another operator took it
						over.
					</Alert>
				)}

				{stage === "timeout" && (
					<Alert severity="warning" data-testid="run-timeout">
						The run didn't start within {formatPending(pendingSecs)}, so the lease was released.
					</Alert>
				)}
			</DialogContent>
			<DialogActions>
				{stage === "configure" && (
					<>
						<Button onClick={onClose}>Cancel</Button>
						<GradedAction calls="ansible/take_lease" action="Take the lease and start work">
							<Button
								variant="contained"
								startIcon={<PlayArrowIcon />}
								disabled={!canStart}
								onClick={() => start(takeOver)}
								data-testid="run-start"
							>
								Start work
							</Button>
						</GradedAction>
					</>
				)}
				{stage === "waiting" && (
					<Button onClick={giveUp} disabled={release.pending} data-testid="run-give-up">
						Give up
					</Button>
				)}
				{stage === "started" && (
					<>
						{held === null && windowId !== null && (
							<Button
								onClick={() => {
									setHoldEnd(inFourHours());
									setHolding((h) => !h);
								}}
								data-testid="run-hold"
							>
								Hold maintenance
							</Button>
						)}
						<Button onClick={onClose}>Close</Button>
					</>
				)}
				{(stage === "timeout" || stage === "gone") && (
					<>
						<Button onClick={onClose}>Close</Button>
						<Button
							variant="contained"
							startIcon={<PlayArrowIcon />}
							onClick={() => {
								setStage("configure");
								setLease(null);
								take.reset();
							}}
						>
							Start again
						</Button>
					</>
				)}
			</DialogActions>
		</Dialog>
	);
}

function TagField({
	label,
	options,
	notes,
	value,
	onChange,
}: {
	label: string;
	options: ReadonlyArray<string>;
	notes: Record<string, string>;
	value: string[];
	onChange: (value: string[]) => void;
}) {
	return (
		<Autocomplete
			multiple
			size="small"
			// The catalogue lists every tag a playbook takes.
			disabled={options.length === 0}
			options={[...options]}
			value={value}
			onChange={(_, next) => onChange(next.map(String))}
			renderOption={(props, option) => (
				<Box component="li" {...props} key={option} sx={{ display: "block !important" }}>
					<Typography sx={{ fontFamily: "monospace" }} variant="body2">
						{option}
					</Typography>
					{notes[option] && (
						<Typography variant="caption" color="text.secondary">
							{notes[option]}
						</Typography>
					)}
				</Box>
			)}
			renderInput={(params) => (
				<TextField
					{...params}
					label={label}
					helperText={options.length === 0 ? "This playbook takes no tags" : undefined}
					slotProps={{
						...params.slotProps,
						htmlInput: {
							...params.slotProps.htmlInput,
							style: { fontFamily: "monospace" },
						},
					}}
				/>
			)}
		/>
	);
}

function assignmentList(assignments: Record<string, string>) {
	return Object.entries(assignments).map(([machine_id, type]) => ({
		machine_id,
		application_type: type === NOTHING ? null : type,
	}));
}

export function typeLabel(type: string): string {
	const text = type.replace(/-/g, " ");
	return text.charAt(0).toUpperCase() + text.slice(1);
}

/** `datetime-local` wants a local wall clock with no zone. */
function inFourHours(): string {
	const at = new Date(Date.now() + 4 * 3600_000);
	const pad = (n: number) => String(n).padStart(2, "0");
	return `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())}T${pad(at.getHours())}:${pad(at.getMinutes())}`;
}

function formatPending(secs: number): string {
	return secs % 60 === 0
		? `${secs / 60} minute${secs === 60 ? "" : "s"}`
		: `${secs} seconds`;
}

function formatClock(ms: number): string {
	const total = Math.ceil(ms / 1000);
	return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
}

/// A value for a shell, quoted where it would be split.
export function shellArg(value: string): string {
	if (/^[\w.:/@=,+-]+$/.test(value)) return value;
	return `'${value.replace(/'/g, "'\\''")}'`;
}

/// The command a run is started with. The first line changes into the
/// operator's own checkout, which only they know, so it is shown and left out of
/// what is copied. The lease names the environment and the intent, so the run
/// is handed nothing else of them.
// spec: ANSRUN#presentation
export function buildCommand(args: {
	vcs: "git" | "jj";
	origin: string;
	leaseId: string;
	playbook: string;
	limit: string;
	tags: ReadonlyArray<string>;
	skipTags: ReadonlyArray<string>;
	extra: string;
}): { checkout: string; copied: string } {
	const update =
		args.vcs === "git"
			? "git switch main && git pull --ff-only"
			: "jj git fetch && jj new main@origin";
	const environment = [`CANOPY_URL=${args.origin}`, `CANOPY_LEASE=${args.leaseId}`];
	const run = [
		"uv run ansible-playbook -i inventory/canopy.yml",
		shellArg(args.playbook),
		...(args.limit === "" ? [] : ["--limit", shellArg(args.limit)]),
		...(args.tags.length === 0 ? [] : ["--tags", shellArg(args.tags.join(","))]),
		...(args.skipTags.length === 0
			? []
			: ["--skip-tags", shellArg(args.skipTags.join(","))]),
		...(args.extra.trim() === "" ? [] : [args.extra.trim()]),
	];
	return {
		checkout: "cd <infra>",
		copied: [update, "cd ansible", `${environment.join(" ")} ${run.join(" ")}`].join("\n"),
	};
}
