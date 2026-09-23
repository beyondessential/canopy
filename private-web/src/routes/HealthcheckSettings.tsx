import {
	Alert,
	Autocomplete,
	Box,
	Button,
	Chip,
	Dialog,
	DialogActions,
	DialogContent,
	DialogTitle,
	FormControlLabel,
	IconButton,
	InputAdornment,
	LinearProgress,
	MenuItem,
	Paper,
	Select,
	Stack,
	Switch,
	Table,
	TableBody,
	TableCell,
	TableContainer,
	TableHead,
	TableRow,
	TextField,
	Typography,
} from "@mui/material";
import ArrowDownwardIcon from "@mui/icons-material/ArrowDownward";
import ArrowUpwardIcon from "@mui/icons-material/ArrowUpward";
import CheckCircleIcon from "@mui/icons-material/CheckCircle";
import DeleteIcon from "@mui/icons-material/Delete";
import EditIcon from "@mui/icons-material/Edit";
import WarningAmberIcon from "@mui/icons-material/WarningAmber";
import { useMemo, useState } from "react";
import Markdown from "../components/Markdown";
import { Link as RouterLink, useParams } from "react-router-dom";
import { ApiError, useApi, useApiAction } from "../api";
import CheckResultChip from "../components/CheckResultChip";
import { GradedAction } from "../components/GradedAction";
import TimeAgo from "../components/TimeAgo";
import { useIsAdmin } from "../hooks/useIsAdmin";
import { usePageTitle } from "../hooks/usePageTitle";
import { evaluate as evalCondition } from "../lib/healthcheck-rule-eval";
import {
	CEILINGS,
	CEILING_INTENT,
	CHECK_RESULT_INTENT,
	CHECK_RESULT_ORDER,
	healthcheckPath,
	namespaceFromSegment,
	qualifiedCheckName,
	sameNamespace,
	type Ceiling,
	type CheckPolicyData,
	type CheckResult,
	type HealthcheckSample,
} from "../types";

// ── Constrained JsonLogic shape ───────────────────────────────────────────
//
// The Rust deserialiser only accepts {if: [c1, r1, c2, r2, …]} ladders
// where each condition is {<op>: [{var: "<dotted>"}, <value>]}. The
// helpers below parse the raw JSON returned by the API into the typed
// shape this page edits, and serialize it back for /update_rules.

const RULE_OPS = ["==", "!=", "<", "<=", ">", ">=", "in_range"] as const;
type RuleOp = (typeof RULE_OPS)[number];
const OP_LABEL: Record<RuleOp, string> = {
	"==": "equal",
	"!=": "not equal",
	"<": "less than",
	"<=": "less than or equal",
	">": "greater than",
	">=": "greater than or equal",
	in_range: "version in range",
};
const VAR_PATTERN = /^(check|status|tag)\.[A-Za-z0-9_]+$/;

interface Branch {
	varPath: string;
	op: RuleOp;
	value: unknown;
	result: CheckResult;
}

function parseRules(raw: unknown): { branches: Branch[]; error: string | null } {
	if (raw == null) return { branches: [], error: null };
	if (typeof raw !== "object" || Array.isArray(raw)) {
		return { branches: [], error: "rules must be a JSON object" };
	}
	const obj = raw as Record<string, unknown>;
	const keys = Object.keys(obj);
	if (keys.length !== 1 || keys[0] !== "if") {
		return { branches: [], error: "rules must be {if: [...]}" };
	}
	const args = obj.if;
	if (!Array.isArray(args)) return { branches: [], error: "'if' value must be an array" };
	if (args.length % 2 !== 0) return { branches: [], error: "'if' args must be even-length" };
	const branches: Branch[] = [];
	for (let i = 0; i < args.length; i += 2) {
		const condRaw = args[i];
		const resultRaw = args[i + 1];
		const cond = parseCondition(condRaw);
		if (cond.error) return { branches: [], error: `branch ${i / 2}: ${cond.error}` };
		if (
			typeof resultRaw !== "string" ||
			!(CHECK_RESULT_ORDER as readonly string[]).includes(resultRaw)
		) {
			return {
				branches: [],
				error: `branch ${i / 2}: invalid result '${String(resultRaw)}'`,
			};
		}
		branches.push({
			varPath: cond.varPath,
			op: cond.op,
			value: cond.value,
			result: resultRaw as CheckResult,
		});
	}
	return { branches, error: null };
}

function parseCondition(raw: unknown): {
	varPath: string;
	op: RuleOp;
	value: unknown;
	error?: string;
} {
	const empty = { varPath: "", op: "==" as RuleOp, value: null };
	if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
		return { ...empty, error: "condition must be a JSON object" };
	}
	const obj = raw as Record<string, unknown>;
	const keys = Object.keys(obj);
	if (keys.length !== 1) {
		return { ...empty, error: "condition object must have exactly one key" };
	}
	const op = keys[0] as RuleOp;
	if (!(RULE_OPS as readonly string[]).includes(op)) {
		return { ...empty, error: `unknown op '${op}'` };
	}
	const args = obj[op];
	if (!Array.isArray(args) || args.length !== 2) {
		return { ...empty, error: `'${op}' must take exactly two args` };
	}
	const varObj = args[0];
	if (typeof varObj !== "object" || varObj === null || Array.isArray(varObj)) {
		return { ...empty, error: "first arg must be {var: '<dotted>'}" };
	}
	const varEntries = Object.entries(varObj as Record<string, unknown>);
	if (varEntries.length !== 1 || varEntries[0][0] !== "var" || typeof varEntries[0][1] !== "string") {
		return { ...empty, error: "first arg must be {var: '<dotted>'}" };
	}
	const varPath = varEntries[0][1] as string;
	return { varPath, op, value: args[1] };
}

function serializeRules(branches: Branch[]): unknown | null {
	if (branches.length === 0) return null;
	const args: unknown[] = [];
	for (const b of branches) {
		args.push({ [b.op]: [{ var: b.varPath }, b.value] });
		args.push(b.result);
	}
	return { if: args };
}

// Best-effort parse of a free-form value input: try as number, else
// JSON, else fall back to the raw string. Lets operators write
// `95` and `"prod"` without thinking about quoting.
function parseValueInput(raw: string): unknown {
	const trimmed = raw.trim();
	if (trimmed === "") return "";
	if (trimmed === "true") return true;
	if (trimmed === "false") return false;
	const n = Number(trimmed);
	if (!Number.isNaN(n) && /^-?\d+(\.\d+)?$/.test(trimmed)) return n;
	try {
		return JSON.parse(trimmed);
	} catch {
		return trimmed;
	}
}

function valueToInputText(value: unknown): string {
	if (typeof value === "string") return value;
	if (value === null) return "null";
	return JSON.stringify(value);
}

// ── Page ──────────────────────────────────────────────────────────────────

export default function HealthcheckSettings() {
	const { source, namespace: segment, checkName } = useParams<{
		source: string;
		namespace: string;
		checkName: string;
	}>();
	const namespace = namespaceFromSegment(segment ?? "-");
	const title = qualifiedCheckName(namespace ?? undefined, checkName ?? "");
	usePageTitle(title || "Healthcheck");
	const isAdmin = useIsAdmin() === true;
	const list = useApi("healthchecks", "list");

	// A check's identity is the (source, namespace, check) triple, so the
	// editor targets exactly one catalog entry — never an aggregate across
	// same-named checks from different sources or different application
	// types, which are unrelated checks.
	const row: CheckPolicyData | null =
		list.status === "ok" && namespace
			? (list.data.find(
					(r) =>
						r.source === source &&
						r.check_name === checkName &&
						sameNamespace(r.namespace, namespace),
				) ?? null)
			: null;

	if (!namespace) {
		return (
			<Alert severity="error">
				<code>{segment}</code> is not a check namespace.
			</Alert>
		);
	}

	return (
		<Stack spacing={2}>
			<Box>
				<Typography variant="body2" color="text.secondary">
					<RouterLink to="/settings/healthchecks">← All healthchecks</RouterLink>
				</Typography>
				<Typography variant="h6" component="h2" sx={{ fontFamily: "monospace" }}>
					{title}
				</Typography>
				<Typography
					variant="body2"
					color="text.secondary"
					sx={{ fontFamily: "monospace" }}
				>
					source: {source}
				</Typography>
			</Box>

			{list.status === "loading" || list.status === "idle" ? (
				<LinearProgress />
			) : list.status === "error" ? (
				<Alert severity="error">{list.error.message}</Alert>
			) : row === null ? (
				<Alert severity="warning">
					No healthcheck <code>{title}</code> reported by{" "}
					<code>{source}</code> in the catalog yet — it'll appear here once that
					source reports it.
				</Alert>
			) : (
				<Stack spacing={2}>
					<Typography variant="body2">
						<RouterLink to={healthcheckPath(row.source, row.namespace, row.check_name)}>
							See servers currently flagging this check
						</RouterLink>
					</Typography>
					<RowMetadata row={row} />
					<CeilingCard row={row} canEdit={isAdmin} onChanged={list.reload} />
					<DocumentationCard row={row} canEdit={isAdmin} onChanged={list.reload} />
					<NotesCard row={row} canEdit={isAdmin} onChanged={list.reload} />
					<RulesCard row={row} canEdit={isAdmin} onChanged={list.reload} />
				</Stack>
			)}
		</Stack>
	);
}

function RowMetadata({ row }: { row: CheckPolicyData }) {
	return (
		<Stack
			direction="row"
			spacing={2}
			sx={{ alignItems: "center", color: "text.secondary" }}
		>
			<Typography variant="caption">
				First seen <TimeAgo timestamp={row.first_seen} />
			</Typography>
			{row.pending_review ? (
				<Chip label="pending review" color="warning" size="small" />
			) : (
				<Typography variant="caption">
					Last reviewed{" "}
					{row.reviewed_at && <TimeAgo timestamp={row.reviewed_at} />}
					{row.reviewed_by && ` by ${row.reviewed_by}`}
				</Typography>
			)}
		</Stack>
	);
}

function CeilingCard({
	row,
	canEdit,
	onChanged,
}: {
	row: CheckPolicyData;
	canEdit: boolean;
	onChanged: () => void;
}) {
	const update = useApiAction("healthchecks", "update");
	const [ceiling, setCeiling] = useState<Ceiling>(row.ceiling as Ceiling);
	const [escalates, setEscalates] = useState(row.escalates);
	// Escalation only means anything at a failed ceiling: it bypasses
	// incident grace on an effective failure, and only a failed ceiling
	// admits a failed effective result. Below that it is dead config, so
	// the toggle is disabled and never sent as set.
	const canEscalate = ceiling === "failed";
	const effectiveEscalates = canEscalate && escalates;
	const save = async () => {
		try {
			await update.call({
				source: row.source,
				namespace: row.namespace,
				check_name: row.check_name,
				ceiling,
				escalates: effectiveEscalates,
				notes: row.notes,
			});
			onChanged();
		} catch {
			setCeiling(row.ceiling as Ceiling);
			setEscalates(row.escalates);
		}
	};
	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<Typography variant="subtitle1" gutterBottom>
				Ceiling
			</Typography>
			<Typography variant="body2" color="text.secondary" sx={{ mb: 1 }}>
				The maximum effective result for this check when no rule below
				matches: anything more urgent grades down to it.
			</Typography>
			<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
				{canEdit ? (
					<Select
						size="small"
						value={ceiling}
						onChange={(e) => setCeiling(e.target.value as Ceiling)}
						disabled={update.pending}
						sx={{ minWidth: 320 }}
					>
						{CEILINGS.map((c) => (
							<MenuItem key={c} value={c}>
								<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
									<CheckResultChip result={c} />
									<Typography variant="caption" color="text.secondary">
										{CEILING_INTENT[c]}
									</Typography>
								</Stack>
							</MenuItem>
						))}
					</Select>
				) : (
					<CheckResultChip result={row.ceiling as Ceiling} />
				)}
				{canEdit ? (
					<FormControlLabel
						control={
							<Switch
								size="small"
								checked={effectiveEscalates}
								onChange={(e) => setEscalates(e.target.checked)}
								disabled={update.pending || !canEscalate}
							/>
						}
						label={
							canEscalate
								? "Escalates: an effective failure notifies immediately, bypassing the incident grace period"
								: "Escalates: only available at a failed ceiling — a lower ceiling never produces a failure to escalate"
						}
						slotProps={{ typography: { variant: "caption" } }}
					/>
				) : row.escalates ? (
					<Chip
						label="escalates"
						color="error"
						size="small"
						variant="outlined"
						title="An effective failure notifies immediately, bypassing the incident grace period"
					/>
				) : (
					<Typography variant="caption" color="text.secondary">
						Does not escalate
					</Typography>
				)}
				{canEdit && (
					<GradedAction calls="healthchecks/update">
						<Button size="small" variant="outlined" onClick={save} disabled={update.pending}>
							Save
						</Button>
					</GradedAction>
				)}
			</Stack>
			{update.error && (
				<Alert severity="error" sx={{ mt: 1 }}>
					{formatError(update.error)}
				</Alert>
			)}
		</Paper>
	);
}

function NotesCard({
	row,
	canEdit,
	onChanged,
}: {
	row: CheckPolicyData;
	canEdit: boolean;
	onChanged: () => void;
}) {
	const update = useApiAction("healthchecks", "update");
	const [notes, setNotes] = useState<string>(row.notes ?? "");
	const save = async () => {
		try {
			await update.call({
				source: row.source,
				namespace: row.namespace,
				check_name: row.check_name,
				ceiling: row.ceiling,
				escalates: row.escalates,
				notes: notes || null,
			});
			onChanged();
		} catch {
			// Leave the textarea where it is so the user can retry.
		}
	};
	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<Typography variant="subtitle1" gutterBottom>
				Notes
			</Typography>
			<TextField
				multiline
				minRows={2}
				fullWidth
				size="small"
				placeholder="Operator commentary on this check (visible only on this page)"
				value={notes}
				onChange={(e) => setNotes(e.target.value)}
				disabled={!canEdit || update.pending}
			/>
			{canEdit && (
				<Box sx={{ mt: 1 }}>
					<GradedAction calls="healthchecks/update">
						<Button size="small" variant="outlined" onClick={save} disabled={update.pending}>
							Save notes
						</Button>
					</GradedAction>
				</Box>
			)}
			{update.error && (
				<Alert severity="error" sx={{ mt: 1 }}>
					{formatError(update.error)}
				</Alert>
			)}
		</Paper>
	);
}

/** Template seeding a fresh document. Convention only — the sections
 * guide authors; nothing enforces the structure. */
const DOC_TEMPLATE = `## Description

What this check observes, and on which part of the system.

## Results

- **fail** — 
- **warn** — 
- **skip** — 

## Solve

Steps to investigate and fix a failure.
`;

function DocumentationCard({
	row,
	canEdit,
	onChanged,
}: {
	row: CheckPolicyData;
	canEdit: boolean;
	onChanged: () => void;
}) {
	const update = useApiAction("healthchecks", "update_documentation");
	const [editing, setEditing] = useState(false);
	const [draft, setDraft] = useState<string>("");
	const startEditing = () => {
		setDraft(row.documentation ?? DOC_TEMPLATE);
		setEditing(true);
	};
	const save = async () => {
		try {
			await update.call({
				source: row.source,
				namespace: row.namespace,
				check_name: row.check_name,
				documentation: draft.trim() === "" ? null : draft,
			});
			setEditing(false);
			onChanged();
		} catch {
			// Leave the editor open so the user can retry.
		}
	};
	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<Stack
				direction="row"
				sx={{ alignItems: "center", justifyContent: "space-between" }}
			>
				<Typography variant="subtitle1" gutterBottom>
					Documentation
				</Typography>
				{canEdit && !editing && (
					<GradedAction calls="healthchecks/update_documentation">
						<Button size="small" variant="outlined" onClick={startEditing}>
							{row.documentation ? "Edit" : "Write documentation"}
						</Button>
					</GradedAction>
				)}
			</Stack>
			{editing ? (
				<>
					<TextField
						multiline
						minRows={10}
						fullWidth
						size="small"
						value={draft}
						onChange={(e) => setDraft(e.target.value)}
						disabled={update.pending}
						slotProps={{
							htmlInput: {
								style: { fontFamily: "monospace" },
								"aria-label": "Documentation markdown",
							},
						}}
					/>
					<Stack direction="row" spacing={1} sx={{ mt: 1 }}>
						<GradedAction calls="healthchecks/update_documentation">
							<Button
								size="small"
								variant="contained"
								onClick={save}
								disabled={update.pending}
							>
								Save documentation
							</Button>
						</GradedAction>
						<Button
							size="small"
							onClick={() => setEditing(false)}
							disabled={update.pending}
						>
							Cancel
						</Button>
					</Stack>
				</>
			) : row.documentation ? (
				<Markdown>{row.documentation}</Markdown>
			) : (
				<Typography variant="body2" color="text.secondary">
					Nobody has documented this check yet. What does it observe, what
					does each result mean, and how does one fix a failure? Shown
					wherever the check appears, and to agents over MCP.
				</Typography>
			)}
			{update.error && (
				<Alert severity="error" sx={{ mt: 1 }}>
					{formatError(update.error)}
				</Alert>
			)}
		</Paper>
	);
}

// ── Rules editor ──────────────────────────────────────────────────────────

function RulesCard({
	row,
	canEdit,
	onChanged,
}: {
	row: CheckPolicyData;
	canEdit: boolean;
	onChanged: () => void;
}) {
	const parsed = useMemo(() => parseRules(row.rules), [row.rules]);
	const [branches, setBranches] = useState<Branch[]>(parsed.branches);
	const [dialog, setDialog] = useState<{ index: number | null } | null>(null);
	const update = useApiAction("healthchecks", "update_rules");

	// Pull a representative sample of variables operators can reach for —
	// the most recent status push (across all servers) that reported
	// this check. Powers the Add/Edit dialog's autocomplete + validation.
	// Fetched lazily; the dialog handles a null sample gracefully.
	const sampleResp = useApi("healthchecks", "sample", {
		source: row.source,
		namespace: row.namespace,
		check_name: row.check_name,
	});
	const sample: HealthcheckSample | null =
		sampleResp.status === "ok" ? (sampleResp.data.sample ?? null) : null;

	// Widen the autocomplete with every `tag.*` key known to canopy. The
	// sample-resolved tag map only covers one server; a rule may want to
	// predicate on a tag that exists on a different server or group.
	const tagKeysResp = useApi("healthchecks", "tag_keys");
	const knownTagKeys = tagKeysResp.status === "ok" ? tagKeysResp.data : [];

	const dirty = useMemo(
		() => JSON.stringify(branches) !== JSON.stringify(parsed.branches),
		[branches, parsed.branches],
	);

	const save = async () => {
		try {
			await update.call({
				source: row.source,
				namespace: row.namespace,
				check_name: row.check_name,
				rules: serializeRules(branches),
			});
			onChanged();
		} catch {
			// keep local state for retry
		}
	};
	const deleteAll = async () => {
		setBranches([]);
		try {
			await update.call({
				source: row.source,
				namespace: row.namespace,
				check_name: row.check_name,
				rules: null,
			});
			onChanged();
		} catch {
			setBranches(parsed.branches);
		}
	};

	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<Stack direction="row" sx={{ alignItems: "center", mb: 1 }}>
				<Typography variant="subtitle1" sx={{ flex: 1 }}>
					Conditional rules
				</Typography>
				{canEdit && (
					<GradedAction opens="healthchecks/update_rules">
						<Button
							size="small"
							variant="contained"
							onClick={() => setDialog({ index: null })}
							disabled={update.pending}
						>
							Add rule
						</Button>
					</GradedAction>
				)}
			</Stack>
			<Typography variant="body2" color="text.secondary" sx={{ mb: 1 }}>
				Evaluated top-to-bottom on every push for this check. The first matching
				branch's result wins — rules can grade in either direction; if none
				match, the observed result is capped at the ceiling above.
			</Typography>
			{sample ? (
				<Typography variant="caption" color="text.secondary" sx={{ mb: 1, display: "block" }}>
					Editor uses the most recent push from{" "}
					<code>{sample.server_name ?? sample.server_host}</code> (
					<TimeAgo timestamp={sample.seen_at} />) to suggest fields and validate inputs.
				</Typography>
			) : sampleResp.status === "ok" ? (
				<Typography variant="caption" color="text.secondary" sx={{ mb: 1, display: "block" }}>
					No server has reported this check yet — the editor can't suggest fields
					until one does.
				</Typography>
			) : null}
			{parsed.error && (
				<Alert severity="warning" sx={{ mb: 1 }}>
					Stored rules are malformed ({parsed.error}) — ingestion falls back to the
					ceiling until you save valid rules from this page.
				</Alert>
			)}
			{branches.length === 0 ? (
				<Alert severity="info">No rules. The ceiling applies to every push.</Alert>
			) : (
				<TableContainer>
					<Table size="small">
						<TableHead>
							<TableRow>
								<TableCell width={40}>#</TableCell>
								<TableCell>Condition</TableCell>
								<TableCell>Result</TableCell>
								{canEdit && <TableCell width={150} />}
							</TableRow>
						</TableHead>
						<TableBody>
							{branches.map((b, idx) => (
								<TableRow key={idx} hover>
									<TableCell>{idx}</TableCell>
									<TableCell sx={{ fontFamily: "monospace", fontSize: "0.85em" }}>
										{b.varPath} {OP_LABEL[b.op]} {valueToInputText(b.value)}
									</TableCell>
									<TableCell>
										<CheckResultChip result={b.result} />
									</TableCell>
									{canEdit && (
										<TableCell>
											<Stack direction="row" spacing={0.5}>
												<GradedAction opens="healthchecks/update_rules">
													<IconButton
														size="small"
														disabled={idx === 0}
														onClick={() =>
															setBranches((bs) => swap(bs, idx, idx - 1))
														}
													>
														<ArrowUpwardIcon fontSize="small" />
													</IconButton>
												</GradedAction>
												<GradedAction opens="healthchecks/update_rules">
													<IconButton
														size="small"
														disabled={idx === branches.length - 1}
														onClick={() =>
															setBranches((bs) => swap(bs, idx, idx + 1))
														}
													>
														<ArrowDownwardIcon fontSize="small" />
													</IconButton>
												</GradedAction>
												<GradedAction opens="healthchecks/update_rules">
													<IconButton
														size="small"
														onClick={() => setDialog({ index: idx })}
													>
														<EditIcon fontSize="small" />
													</IconButton>
												</GradedAction>
												<GradedAction opens="healthchecks/update_rules">
													<IconButton
														size="small"
														onClick={() =>
															setBranches((bs) =>
																bs.filter((_, i) => i !== idx),
															)
														}
													>
														<DeleteIcon fontSize="small" />
													</IconButton>
												</GradedAction>
											</Stack>
										</TableCell>
									)}
								</TableRow>
							))}
						</TableBody>
					</Table>
				</TableContainer>
			)}
			{canEdit && (
				<Stack direction="row" spacing={1} sx={{ mt: 2 }}>
					<GradedAction calls="healthchecks/update_rules">
						<Button
							size="small"
							variant="contained"
							onClick={save}
							disabled={!dirty || update.pending}
						>
							Save rules
						</Button>
					</GradedAction>
					{branches.length > 0 && (
						<GradedAction calls="healthchecks/update_rules">
							<Button
								size="small"
								onClick={deleteAll}
								disabled={update.pending}
							>
								Delete all rules
							</Button>
						</GradedAction>
					)}
				</Stack>
			)}
			{update.error && (
				<Alert severity="error" sx={{ mt: 1 }}>
					{formatError(update.error)}
				</Alert>
			)}
			{dialog && (
				<BranchDialog
					initial={dialog.index != null ? branches[dialog.index] : null}
					sample={sample}
					knownTagKeys={knownTagKeys}
					onCancel={() => setDialog(null)}
					onSave={(b) => {
						setBranches((bs) => {
							if (dialog.index == null) return [...bs, b];
							const next = [...bs];
							next[dialog.index] = b;
							return next;
						});
						setDialog(null);
					}}
				/>
			)}
		</Paper>
	);
}

function swap<T>(arr: T[], i: number, j: number): T[] {
	const next = [...arr];
	[next[i], next[j]] = [next[j], next[i]];
	return next;
}

/// Fields available on the sample, projected as dotted paths.
function sampleVarOptions(sample: HealthcheckSample | null): string[] {
	if (!sample) return [];
	const opts: string[] = [];
	for (const k of Object.keys(sample.check_extra)) opts.push(`check.${k}`);
	for (const k of Object.keys(sample.status_extra)) opts.push(`status.${k}`);
	for (const k of Object.keys(sample.tags)) opts.push(`tag.${k}`);
	return opts.sort();
}

function BranchDialog({
	initial,
	sample,
	knownTagKeys,
	onCancel,
	onSave,
}: {
	initial: Branch | null;
	sample: HealthcheckSample | null;
	knownTagKeys: string[];
	onCancel: () => void;
	onSave: (b: Branch) => void;
}) {
	const [varPath, setVarPath] = useState(initial?.varPath ?? "status.");
	const [op, setOp] = useState<RuleOp>(initial?.op ?? "==");
	const [valueText, setValueText] = useState(
		initial ? valueToInputText(initial.value) : "",
	);
	const [result, setResult] = useState<CheckResult>(initial?.result ?? "failed");
	const varValid = VAR_PATTERN.test(varPath);

	// Did the sample carry a value for this exact var path? Powers the
	// pass/warn badge: green when the operator's predicating on a field
	// the most recent push actually had, yellow otherwise. A tag that
	// exists on some *other* server is fine to predicate on but stays
	// yellow here — at evaluation time the sampled server's tag map
	// won't carry it, so the rule branch would be false.
	const sampleKeys = useMemo(() => sampleVarOptions(sample), [sample]);
	const varInSample = varValid && sampleKeys.includes(varPath);

	// Autocomplete suggestions widen the sample with every known tag
	// key across the system. Sorted, de-duplicated.
	const options = useMemo(() => {
		const merged = new Set<string>(sampleKeys);
		for (const k of knownTagKeys) merged.add(`tag.${k}`);
		return [...merged].sort();
	}, [sampleKeys, knownTagKeys]);

	const submit = () => {
		if (!varValid) return;
		onSave({
			varPath,
			op,
			value: parseValueInput(valueText),
			result,
		});
	};

	const varAdornment = varValid ? (
		varInSample ? (
			<InputAdornment position="end">
				<CheckCircleIcon
					fontSize="small"
					color="success"
					titleAccess="Field is present in the sample push"
				/>
			</InputAdornment>
		) : sample ? (
			<InputAdornment position="end">
				<WarningAmberIcon
					fontSize="small"
					color="warning"
					titleAccess="Field is not present in the sampled push — the rule will be evaluated against an absent value (typically falsy)."
				/>
			</InputAdornment>
		) : undefined
	) : undefined;

	const varHelper = !varValid
		? "Must match check.<field>, status.<field>, or tag.<field>"
		: !sample
			? "No sample available to validate against. Path like status.bestoolVersion, check.used_pct, tag.environment."
			: varInSample
				? "Field is present in the sampled push."
				: "Field is not present in the sample — predicates referring to it will see an absent value at evaluation time.";

	return (
		<Dialog open onClose={onCancel} fullWidth maxWidth="sm">
			<DialogTitle>{initial ? "Edit rule" : "Add rule"}</DialogTitle>
			<DialogContent>
				<Stack spacing={2} sx={{ mt: 1 }}>
					<Autocomplete
						freeSolo
						options={options}
						inputValue={varPath}
						onInputChange={(_, v) => setVarPath(v)}
						renderInput={(params) => (
							<TextField
								{...params}
								label="Variable"
								helperText={varHelper}
								error={!varValid}
								size="small"
								slotProps={{
									...params.slotProps,
									input: {
										...params.slotProps.input,
										endAdornment: (
											<>
												{varAdornment}
												{params.slotProps.input.endAdornment}
											</>
										),
									},
								}}
							/>
						)}
					/>
					<Stack direction="row" spacing={1}>
						<TextField
							select
							label="Operator"
							size="small"
							value={op}
							onChange={(e) => setOp(e.target.value as RuleOp)}
							sx={{ minWidth: 220 }}
						>
							{RULE_OPS.map((o) => (
								<MenuItem key={o} value={o}>
									{OP_LABEL[o]}
								</MenuItem>
							))}
						</TextField>
						<TextField
							label="Value"
							size="small"
							fullWidth
							helperText={
								op === "in_range"
									? "semver range, e.g. >=2.4.0 <2.5.4 or ^2.6"
									: "number, string, or JSON literal"
							}
							value={valueText}
							onChange={(e) => setValueText(e.target.value)}
						/>
					</Stack>
					<TextField
						select
						label="Result"
						size="small"
						value={result}
						onChange={(e) => setResult(e.target.value as CheckResult)}
						sx={{ minWidth: 320 }}
					>
						{CHECK_RESULT_ORDER.map((r) => (
							<MenuItem key={r} value={r}>
								<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
									<CheckResultChip result={r} />
									<Typography variant="caption" color="text.secondary">
										{CHECK_RESULT_INTENT[r]}
									</Typography>
								</Stack>
							</MenuItem>
						))}
					</TextField>
					{varValid && sample && (
						<PreviewBox
							sample={sample}
							varPath={varPath}
							op={op}
							valueText={valueText}
							result={result}
						/>
					)}
				</Stack>
			</DialogContent>
			<DialogActions>
				<Button onClick={onCancel}>Cancel</Button>
				<Button variant="contained" onClick={submit} disabled={!varValid}>
					{initial ? "Save" : "Add"}
				</Button>
			</DialogActions>
		</Dialog>
	);
}

function PreviewBox({
	sample,
	varPath,
	op,
	valueText,
	result: branchResult,
}: {
	sample: HealthcheckSample;
	varPath: string;
	op: RuleOp;
	valueText: string;
	result: CheckResult;
}) {
	const result = useMemo(
		() =>
			evalCondition(
				{ varPath, op, value: parseValueInput(valueText) },
				sample,
			),
		[varPath, op, valueText, sample],
	);
	return (
		<Alert
			severity={result.matched ? "success" : "info"}
			variant="outlined"
			sx={{ alignItems: "center" }}
		>
			<Stack direction="row" spacing={1} sx={{ alignItems: "center", flexWrap: "wrap" }}>
				<Typography variant="body2">
					Against the sample push (
					<code>{sample.server_name ?? sample.server_host}</code>):
				</Typography>
				{result.matched ? (
					<Stack direction="row" spacing={0.5} sx={{ alignItems: "center" }}>
						<Typography variant="body2">would grade to</Typography>
						<CheckResultChip result={branchResult} />
					</Stack>
				) : (
					<Typography variant="body2">would not match → ceiling applies</Typography>
				)}
			</Stack>
			<Typography
				variant="caption"
				color="text.secondary"
				sx={{ display: "block", mt: 0.5 }}
			>
				{result.notes}
			</Typography>
		</Alert>
	);
}

function formatError(err: unknown): string {
	if (err instanceof ApiError) {
		const detail = err.detail as { title?: string } | null;
		if (detail?.title) return detail.title;
		return err.message;
	}
	if (err instanceof Error) return err.message;
	return String(err);
}
