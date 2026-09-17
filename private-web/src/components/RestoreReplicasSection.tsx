import AddIcon from "@mui/icons-material/Add";
import DeleteIcon from "@mui/icons-material/Delete";
import EditIcon from "@mui/icons-material/Edit";
import KeyboardArrowDownIcon from "@mui/icons-material/KeyboardArrowDown";
import KeyboardArrowUpIcon from "@mui/icons-material/KeyboardArrowUp";
import OpenInNewIcon from "@mui/icons-material/OpenInNew";
import {
	Alert,
	Box,
	Button,
	Chip,
	Collapse,
	Dialog,
	DialogActions,
	DialogContent,
	DialogTitle,
	FormControl,
	FormControlLabel,
	IconButton,
	InputLabel,
	Link,
	LinearProgress,
	MenuItem,
	Paper,
	Select,
	Stack,
	Switch,
	Table,
	TableBody,
	TableCell,
	TableHead,
	TableRow,
	TextField,
	Tooltip,
	Typography,
} from "@mui/material";
import { useEffect, useState } from "react";
import { ApiError, callApi, useApi } from "../api";
import TimeAgo from "./TimeAgo";
import { humanSeconds } from "../lib/humanDuration";
import type {
	IntentDescriptor,
	ParamSpec,
	RestoreActivity,
	RestoreConsumerView,
	RestoreReplicaView,
} from "../types";
import { REDACTION_GAP_LABELS, REDACTION_PARAMS } from "../types";

function kebabCase(s: string): string {
	return s
		.trim()
		.toLowerCase()
		.replace(/[^a-z0-9]+/g, "-")
		.replace(/^-+|-+$/g, "");
}

function formatError(err: unknown): string {
	if (err instanceof ApiError) {
		const detail = err.detail as { title?: string } | null;
		return detail?.title ?? err.message;
	}
	if (err instanceof Error) return err.message;
	return String(err);
}

/** Pull a string `url` out of a check's opaque health details, if present — the
 * link a `url`-semantic intent attaches to its running replica. */
function healthUrl(details: unknown): string | null {
	if (details && typeof details === "object" && "url" in details) {
		const url = (details as { url?: unknown }).url;
		if (typeof url === "string" && url.length > 0) return url;
	}
	return null;
}

/** Managed restore replicas for one group — the declarations Canopy drives and
 * the restore-health reports it has received back, shown on the group's backup
 * page. */
export default function RestoreReplicasSection({
	groupId,
	isAdmin,
}: {
	groupId: string;
	isAdmin: boolean;
}) {
	const [tick, setTick] = useState(0);
	const reload = () => setTick((t) => t + 1);

	const replicas = useApi(
		"restore_replicas",
		"for_group",
		{ server_group_id: groupId },
		[groupId, tick],
	);
	const consumers = useApi("restore_replicas", "consumers", {}, [tick]);
	const checks = useApi(
		"restore_replicas",
		"checks",
		{ server_group_id: groupId },
		[groupId, tick],
	);

	const [createOpen, setCreateOpen] = useState(false);
	const [editingReplica, setEditingReplica] = useState<RestoreReplicaView | null>(
		null,
	);
	const [error, setError] = useState<string | null>(null);

	const onDelete = async (id: string) => {
		try {
			await callApi("restore_replicas", "delete", { id });
			reload();
		} catch (err) {
			setError(formatError(err));
		}
	};

	// `update` replaces every field, so toggling `enabled` from the table
	// carries the rest of the row through unchanged.
	const onToggle = async (r: RestoreReplicaView, enabled: boolean) => {
		try {
			await callApi("restore_replicas", "update", {
				id: r.id,
				consumer_device_id: r.consumer_device_id,
				group_id: r.group_id,
				machine_id: r.machine_id,
				type: r.type,
				intent: r.intent,
				name: r.name,
				overdue_after: r.overdue_after,
				params: r.params as Record<string, unknown>,
				redacts: r.redacts,
				publishes_schemas: r.publishes_schemas,
				enabled,
			});
			reload();
		} catch (err) {
			setError(formatError(err));
		}
	};

	return (
		<Box>
			<Stack
				direction="row"
				sx={{ alignItems: "center", justifyContent: "space-between", mb: 1 }}
			>
				<Typography variant="h6" component="h2">
					Restore replicas
				</Typography>
				{isAdmin && (
					<Button
						size="small"
						variant="outlined"
						startIcon={<AddIcon />}
						onClick={() => setCreateOpen(true)}
					>
						Declare replica
					</Button>
				)}
			</Stack>

			<Typography variant="body2" color="text.secondary" sx={{ mb: 1 }}>
				Canopy drives a restore consumer to keep these replicas, each restored
				from the latest snapshot for its server.
			</Typography>

			{error && (
				<Alert severity="error" sx={{ mb: 1 }} onClose={() => setError(null)}>
					{error}
				</Alert>
			)}

			{replicas.status === "loading" || replicas.status === "idle" ? (
				<LinearProgress />
			) : replicas.status === "error" ? (
				<Alert severity="error">{replicas.error.message}</Alert>
			) : replicas.data.length === 0 ? (
				<Alert severity="info">No restore replicas declared for this group.</Alert>
			) : (
				<Paper variant="outlined">
					<Table size="small">
						<TableHead>
							<TableRow>
								<TableCell>Name</TableCell>
								<TableCell>Consumer</TableCell>
								<TableCell>Scope</TableCell>
								<TableCell>Type</TableCell>
								<TableCell>Intent</TableCell>
								<TableCell>Overdue after</TableCell>
								<TableCell>Redaction</TableCell>
								<TableCell>Params</TableCell>
								<TableCell>Enabled</TableCell>
								{isAdmin && <TableCell align="right">Actions</TableCell>}
							</TableRow>
						</TableHead>
						<TableBody>
							{replicas.data.map((r) => (
								<TableRow key={r.id}>
									<TableCell>{r.name}</TableCell>
									<TableCell>
										{r.consumer_name ?? r.consumer_device_id.slice(0, 8)}
									</TableCell>
									<TableCell>
										{r.machine_id ? "one machine" : "whole group"}
									</TableCell>
									<TableCell>{r.type}</TableCell>
									<TableCell>
										<Stack
											direction="row"
											spacing={0.5}
											sx={{ alignItems: "center" }}
										>
											<span>{r.intent}</span>
											{r.gap && (
												<Tooltip title="The consumer does not currently advertise this intent, so Canopy is not dispatching it.">
													<Chip label="gap" color="warning" size="small" />
												</Tooltip>
											)}
											{r.publishes_schemas && (
												<Tooltip title="This consumer may publish the group's reporting schema, which every application in the group is offered.">
													<Chip
														label="publishes schema"
														color="info"
														size="small"
													/>
												</Tooltip>
											)}
										</Stack>
									</TableCell>
									<TableCell>{r.overdue_after ?? "no bound"}</TableCell>
									<TableCell>
										<RedactionCell
											replica={r}
											activity={checks.status === "ok" ? checks.data : []}
										/>
									</TableCell>
									<TableCell>
										<ParamSummary params={r.params} />
									</TableCell>
									<TableCell>
										<Switch
											checked={r.enabled}
											disabled={!isAdmin}
											onChange={(e) => onToggle(r, e.target.checked)}
											slotProps={{ input: { "aria-label": `toggle ${r.name}` } }}
										/>
									</TableCell>
									{isAdmin && (
										<TableCell align="right">
											<IconButton
												aria-label={`edit ${r.name}`}
												onClick={() => setEditingReplica(r)}
											>
												<EditIcon />
											</IconButton>
											<IconButton
												edge="end"
												aria-label={`delete ${r.name}`}
												onClick={() => onDelete(r.id)}
											>
												<DeleteIcon />
											</IconButton>
										</TableCell>
									)}
								</TableRow>
							))}
						</TableBody>
					</Table>
				</Paper>
			)}

			<Typography variant="subtitle2" sx={{ mt: 2, mb: 1 }}>
				Recent restore checks
			</Typography>
			{checks.status === "ok" && checks.data.length === 0 ? (
				<Alert severity="info">No restore-health reports yet.</Alert>
			) : checks.status === "ok" ? (
				<Paper variant="outlined">
					<Table size="small">
						<TableHead>
							<TableRow>
								<TableCell padding="checkbox" />
								<TableCell>When</TableCell>
								<TableCell>Machine</TableCell>
								<TableCell>Type</TableCell>
								<TableCell>Intent</TableCell>
								<TableCell>Outcome</TableCell>
								<TableCell>Duration</TableCell>
								<TableCell>PG version</TableCell>
								<TableCell>Redaction</TableCell>
								<TableCell>Snapshot</TableCell>
								<TableCell>Replica</TableCell>
							</TableRow>
						</TableHead>
						<TableBody>
							{checks.data.map((c) => (
								<CheckRow key={c.key} check={c} />
							))}
						</TableBody>
					</Table>
				</Paper>
			) : null}

			{createOpen && (
				<CreateReplicaDialog
					groupId={groupId}
					onClose={() => setCreateOpen(false)}
					onCreated={() => {
						setCreateOpen(false);
						reload();
					}}
					consumers={consumers.status === "ok" ? consumers.data : []}
					existing={replicas.status === "ok" ? replicas.data : []}
				/>
			)}

			{editingReplica && (
				<EditReplicaDialog
					groupId={groupId}
					replica={editingReplica}
					consumers={consumers.status === "ok" ? consumers.data : []}
					onClose={() => setEditingReplica(null)}
					onUpdated={() => {
						setEditingReplica(null);
						reload();
					}}
				/>
			)}
		</Box>
	);
}

/** Compact one-line summary of a replica's stored parameter values. */
function ParamSummary({ params }: { params: unknown }) {
	if (!params || typeof params !== "object") return <span>—</span>;
	const entries = Object.entries(params as Record<string, unknown>);
	if (entries.length === 0) return <span>—</span>;
	return (
		<Typography variant="caption" color="text.secondary">
			{entries.map(([k, v]) => `${k}=${String(v)}`).join(", ")}
		</Typography>
	);
}

/** A single typed parameter input for the declare-replica form. Empty leaves the
 * parameter unset (the consumer receives its default, or null). */
function ParamField({
	name,
	spec,
	value,
	onChange,
}: {
	name: string;
	spec: ParamSpec;
	value: string;
	onChange: (value: string) => void;
}) {
	const def = spec.default;
	const defHint = def != null ? `default: ${String(def)}` : "optional";

	if (spec.type === "boolean") {
		return (
			<FormControl fullWidth size="small">
				<InputLabel id={`param-${name}-label`}>{name}</InputLabel>
				<Select
					labelId={`param-${name}-label`}
					label={name}
					value={value}
					onChange={(e) => onChange(e.target.value)}
				>
					<MenuItem value="">
						<em>{defHint}</em>
					</MenuItem>
					<MenuItem value="true">true</MenuItem>
					<MenuItem value="false">false</MenuItem>
				</Select>
			</FormControl>
		);
	}

	// Duration and size values are typed with human units and resolved to raw
	// seconds/bytes by the backend.
	const unit =
		spec.type === "duration" ? "duration" : spec.type === "bytes" ? "size" : "";
	const example =
		spec.type === "duration"
			? "e.g. 2h 30m"
			: spec.type === "bytes"
				? "e.g. 20Gi"
				: "";
	return (
		<TextField
			size="small"
			fullWidth
			type={spec.type === "integer" ? "number" : "text"}
			label={unit ? `${name} (${unit})` : name}
			placeholder={def != null ? String(def) : (example || "optional")}
			helperText={example ? `${example} — ${defHint}` : defHint}
			value={value}
			onChange={(e) => onChange(e.target.value)}
		/>
	);
}

/** The "Parameters" heading plus one typed input per entry in `paramSchema`,
 * shared between the declare and edit dialogs. Renders nothing for an intent
 * with no parameters (or one Canopy has no schema for, e.g. a gap). */
function ParamFieldsEditor({
	paramSchema,
	values,
	onChange,
}: {
	paramSchema: Record<string, ParamSpec>;
	values: Record<string, string>;
	onChange: (key: string, value: string) => void;
}) {
	const entries = Object.entries(paramSchema);
	if (entries.length === 0) return null;
	return (
		<>
			<Typography variant="subtitle2">Parameters</Typography>
			{entries.map(([key, spec]) => (
				<ParamField
					key={key}
					name={key}
					spec={spec}
					value={values[key] ?? ""}
					onChange={(v) => onChange(key, v)}
				/>
			))}
		</>
	);
}

/** Whether a declaration redacts, and any servers it covers whose masking
 * Canopy can't currently line up — a redacting declaration that quietly
 * restores nothing for a server, or resolves a manifest that was never
 * published, is otherwise indistinguishable from one that is working. */
function RedactionCell({
	replica,
	activity,
}: {
	replica: RestoreReplicaView;
	activity: RestoreActivity[];
}) {
	if (!replica.redacts) {
		return (
			<Typography variant="body2" color="text.secondary">
				not redacted
			</Typography>
		);
	}
	const gaps = replica.redaction_gaps;
	// What the declaration asks for is not what it got: the most recent
	// report that carried a redaction says whether the replicas are actually
	// masked, and a partial one is the case worth seeing from the list.
	const reported = activity.find(
		(c) =>
			c.redaction_outcome != null &&
			c.type === replica.type &&
			c.intent === replica.intent &&
			(replica.machine_id == null || c.machine_id === replica.machine_id),
	);
	const state = reported?.redaction_outcome;
	return (
		<Stack direction="row" spacing={0.5} sx={{ alignItems: "center" }}>
			{state === "partial" || state === "failed" ? (
				<Tooltip
					title={
						state === "partial"
							? "Live and mostly masked, with columns in the clear."
							: "No masking took effect; the replica is held on its previous data."
					}
				>
					<Chip label={state} color="warning" size="small" />
				</Tooltip>
			) : (
				<Chip label="redacted" color="success" size="small" />
			)}
			{gaps.length > 0 && (
				<Tooltip
					title={
						<>
							Canopy has no masking for:
							{gaps.map((g) => (
								<div key={g.server_id}>
									{g.server_name ?? g.server_id.slice(0, 8)} —{" "}
									{REDACTION_GAP_LABELS[g.reason] ?? g.reason}
									{g.version ? ` (${g.version})` : ""}
								</div>
							))}
						</>
					}
				>
					<Chip
						label={`${gaps.length} unmaskable`}
						color="warning"
						size="small"
					/>
				</Tooltip>
			)}
		</Stack>
	);
}

/** The redaction switch, shown only for an intent that can redact. Canopy
 * resolves the masking manifest from the server's product, so there is
 * nothing else for the operator to fill in. */
function RedactionField({
	value,
	onChange,
}: {
	value: boolean;
	onChange: (value: boolean) => void;
}) {
	return (
		<FormControlLabel
			control={
				<Switch
					size="small"
					checked={value}
					onChange={(e) => onChange(e.target.checked)}
				/>
			}
			label={
				<Stack>
					<Typography variant="body2">Redact this replica</Typography>
					<Typography variant="caption" color="text.secondary">
						Masks the data before it is served, using the manifest published
						for the version restored.
					</Typography>
				</Stack>
			}
		/>
	);
}

/** The publishing switch, shown only for an intent that builds reporting
 * schemas. A build is per group from data the masking manifest has not
 * altered, so a machine-scoped or redacting declaration cannot carry it. */
function PublishesSchemasField({
	value,
	onChange,
	disabled,
	why,
}: {
	value: boolean;
	onChange: (value: boolean) => void;
	disabled: boolean;
	why: string;
}) {
	return (
		<FormControlLabel
			disabled={disabled}
			control={
				<Switch
					size="small"
					checked={value}
					onChange={(e) => onChange(e.target.checked)}
				/>
			}
			label={
				<Stack>
					<Typography variant="body2">
						Publish this group's reporting schema
					</Typography>
					<Typography variant="caption" color="text.secondary">
						{disabled
							? why
							: "Lets this consumer register the schema every application in the group is offered."}
					</Typography>
				</Stack>
			}
		/>
	);
}

/** Convert the typed form fields into the wire params object, omitting any the
 * operator left unset (the consumer resolves those to their default or null).
 * Returns an error message string if a numeric field doesn't parse. */
function paramValuesToWire(
	paramSchema: Record<string, ParamSpec>,
	paramValues: Record<string, string>,
): Record<string, unknown> | string {
	const out: Record<string, unknown> = {};
	for (const [key, spec] of Object.entries(paramSchema)) {
		const raw = paramValues[key];
		if (raw == null || raw === "") continue;
		if (spec.type === "boolean") {
			out[key] = raw === "true";
		} else if (spec.type === "integer") {
			const n = Number(raw);
			if (!Number.isFinite(n)) return `Parameter "${key}" must be a number`;
			out[key] = n;
		} else if (spec.type === "bytes" || spec.type === "duration") {
			// Duration and size values go over the wire as human-unit strings
			// (e.g. "2h 30m", "20Gi"); the backend resolves them to raw
			// seconds/bytes, or rejects them with an error shown inline.
			if (raw.trim() === "") continue;
			out[key] = raw.trim();
		} else {
			out[key] = raw;
		}
	}
	return out;
}

/** Stringify a replica's stored parameter values for the edit form's typed
 * inputs, keyed by the intent's current schema. Values for keys the schema no
 * longer describes are dropped — there'd be no field to show them in. */
function wireParamsToValues(
	paramSchema: Record<string, ParamSpec>,
	params: unknown,
): Record<string, string> {
	const source = (
		params && typeof params === "object" ? params : {}
	) as Record<string, unknown>;
	const out: Record<string, string> = {};
	for (const [key, spec] of Object.entries(paramSchema)) {
		const raw = source[key];
		if (raw == null) continue;
		out[key] = spec.type === "boolean" ? String(Boolean(raw)) : String(raw);
	}
	return out;
}

/** A group's servers (unarchived) and the backup types available to declare
 * against, shared by the declare and edit dialogs. */
function useGroupScopeData(groupId: string) {
	const detail = useApi(
		"fleet/groups",
		"get",
		{ server_group_id: groupId },
		[groupId],
	);
	const typeDefaults = useApi("backups", "type_defaults");
	// Machines, not applications: a declaration is over the box whose snapshot
	// gets restored.
	const servers =
		detail.status === "ok"
			? detail.data.machines.map((m) => ({ id: m.id, name: m.name }))
			: [];
	const typeOptions =
		typeDefaults.status === "ok" && typeDefaults.data.length > 0
			? typeDefaults.data.map((t) => t.type)
			: ["tamanu-postgres"];
	const groupName = detail.status === "ok" ? detail.data.group.name : "";
	return { servers, typeOptions, groupName };
}

type ScopeServer = { id: string; name?: string | null };

/** The intents a consumer advertises, and the schema for the one currently
 * selected — shared by the declare and edit dialogs so param fields re-derive
 * consistently when the consumer or intent changes. */
function useIntentSchema(
	consumers: RestoreConsumerView[],
	consumerId: string,
	intent: string,
) {
	const selectedConsumer = consumers.find((c) => c.device_id === consumerId);
	const intentOptions: IntentDescriptor[] = selectedConsumer?.intents ?? [];
	const selectedDescriptor = intentOptions.find((d) => d.intent === intent);
	const advertised =
		(selectedDescriptor?.params as Record<string, ParamSpec> | undefined) ?? {};
	const canRedact = selectedDescriptor?.semantics?.includes("redact") ?? false;
	const canPublishSchemas =
		selectedDescriptor?.semantics?.includes("reporting-schema") ?? false;
	// Canopy owns the masking parameters for a `redact` intent in both states,
	// so they get no field: the redaction switch is the whole of the operator's
	// say in it.
	const paramSchema: Record<string, ParamSpec> = canRedact
		? Object.fromEntries(
				Object.entries(advertised).filter(
					([key]) => !REDACTION_PARAMS.includes(key),
				),
			)
		: advertised;
	return {
		intentOptions,
		selectedDescriptor,
		paramSchema,
		canRedact,
		canPublishSchemas,
	};
}

/** Consumer, server (or whole-group), type, and intent selects, shared by the
 * declare and edit dialogs. The current `type`/`intent` value is always shown
 * even if it falls outside the enumerated options — a declaration can carry a
 * custom type or a gap intent the consumer no longer advertises. */
function ScopeFields({
	consumers,
	servers,
	typeOptions,
	consumerId,
	onConsumerChange,
	serverId,
	onServerChange,
	type,
	onTypeChange,
	intent,
	onIntentChange,
	intentOptions,
	description,
}: {
	consumers: RestoreConsumerView[];
	servers: ScopeServer[];
	typeOptions: string[];
	consumerId: string;
	onConsumerChange: (id: string) => void;
	serverId: string;
	onServerChange: (id: string) => void;
	type: string;
	onTypeChange: (type: string) => void;
	intent: string;
	onIntentChange: (intent: string) => void;
	intentOptions: IntentDescriptor[];
	description?: string | null;
}) {
	const typeSelectOptions = typeOptions.includes(type)
		? typeOptions
		: [...typeOptions, type].filter(Boolean);
	const intentSelectOptions = intentOptions.some((d) => d.intent === intent)
		? intentOptions
		: [
				...intentOptions,
				...(intent
					? [
							{
								intent,
								description: null,
								semantics: [],
								params: {},
							} as IntentDescriptor,
						]
					: []),
			];

	return (
		<>
			<FormControl fullWidth size="small">
				<InputLabel id="consumer-label">Consumer</InputLabel>
				<Select
					labelId="consumer-label"
					label="Consumer"
					value={consumerId}
					onChange={(e) => onConsumerChange(e.target.value)}
				>
					{consumers.map((c) => (
						<MenuItem key={c.device_id} value={c.device_id}>
							{c.name ?? c.device_id}
						</MenuItem>
					))}
				</Select>
			</FormControl>

			<FormControl fullWidth size="small">
				<InputLabel id="server-label">Machine</InputLabel>
				<Select
					labelId="server-label"
					label="Machine"
					value={serverId}
					onChange={(e) => onServerChange(e.target.value)}
				>
					<MenuItem value="">All machines in the group</MenuItem>
					{servers.map((s) => (
						<MenuItem key={s.id} value={s.id}>
							{s.name ?? s.id}
						</MenuItem>
					))}
				</Select>
			</FormControl>

			<FormControl fullWidth size="small">
				<InputLabel id="type-label">Type</InputLabel>
				<Select
					labelId="type-label"
					label="Type"
					value={type}
					onChange={(e) => onTypeChange(e.target.value)}
				>
					{typeSelectOptions.map((t) => (
						<MenuItem key={t} value={t}>
							{t}
						</MenuItem>
					))}
				</Select>
			</FormControl>

			<FormControl fullWidth size="small">
				<InputLabel id="intent-label">Intent</InputLabel>
				<Select
					labelId="intent-label"
					label="Intent"
					value={intent}
					onChange={(e) => onIntentChange(e.target.value)}
				>
					{intentSelectOptions.map((d) => (
						<MenuItem key={d.intent} value={d.intent}>
							{d.intent}
						</MenuItem>
					))}
				</Select>
			</FormControl>

			{description && (
				<Typography variant="body2" color="text.secondary">
					{description}
				</Typography>
			)}
		</>
	);
}

function CreateReplicaDialog({
	groupId,
	onClose,
	onCreated,
	consumers,
	existing,
}: {
	groupId: string;
	onClose: () => void;
	onCreated: () => void;
	consumers: RestoreConsumerView[];
	existing: RestoreReplicaView[];
}) {
	const { servers, typeOptions, groupName } = useGroupScopeData(groupId);

	const [consumerId, setConsumerId] = useState("");
	const [serverId, setServerId] = useState(""); // "" = whole group
	const [type, setType] = useState("tamanu-postgres");
	const [intent, setIntent] = useState("");
	const [name, setName] = useState("");
	const [nameEdited, setNameEdited] = useState(false);
	const [overdue, setOverdue] = useState("");
	const [paramValues, setParamValues] = useState<Record<string, string>>({});
	const [redacts, setRedacts] = useState(false);
	const [publishesSchemas, setPublishesSchemas] = useState(false);
	const [pending, setPending] = useState(false);
	const [error, setError] = useState<string | null>(null);

	const {
		intentOptions,
		selectedDescriptor,
		paramSchema,
		canRedact,
		canPublishSchemas,
	} = useIntentSchema(consumers, consumerId, intent);

	// Auto-select the sole consumer, if there's only one to choose from.
	useEffect(() => {
		if (!consumerId && consumers.length === 1) {
			setConsumerId(consumers[0].device_id);
		}
	}, [consumers, consumerId]);

	// Keep the intent on a value the selected consumer actually advertises.
	useEffect(() => {
		const names = intentOptions.map((d) => d.intent);
		if (names.length > 0 && !names.includes(intent)) {
			setIntent(names[0]);
		}
	}, [intentOptions, intent]);

	// Reset parameter values whenever the intent changes — a param set is
	// specific to one intent's schema.
	useEffect(() => {
		setParamValues({});
	}, [intent]);

	// An intent that can't redact can't carry the flag, and the backend
	// refuses it rather than storing an intent it can't honour.
	useEffect(() => {
		if (!canRedact) setRedacts(false);
	}, [canRedact]);

	// A build is dispatched per group from unmasked data, so narrowing the
	// declaration to a machine or turning redaction on drops the flag with it.
	useEffect(() => {
		if (!canPublishSchemas || redacts || serverId) setPublishesSchemas(false);
	}, [canPublishSchemas, redacts, serverId]);

	// Suggest a name from the group, (if picked) server, and intent, until the
	// operator types their own. The intent is part of it because names are
	// unique per consumer: without it, declaring a second intent for the same
	// server would suggest a name already taken.
	const selectedServer = servers.find((s) => s.id === serverId);
	const serverName = selectedServer
		? (selectedServer.name ?? selectedServer.id)
		: "";
	const baseName = kebabCase(
		[groupName, serverName, intent].filter(Boolean).join("-"),
	);
	// Several replicas of one scope are allowed and told apart by name, so the
	// same scope suggests the same base twice. Count past the consumer's names
	// already in use rather than handing the operator a name that's taken.
	const taken = new Set(
		existing
			.filter((r) => r.consumer_device_id === consumerId)
			.map((r) => r.name),
	);
	let suggestedName = baseName;
	for (let n = 2; baseName && taken.has(suggestedName); n++) {
		suggestedName = `${baseName}-${n}`;
	}
	useEffect(() => {
		if (!nameEdited) setName(suggestedName);
	}, [suggestedName, nameEdited]);

	const onSubmit = async () => {
		if (!consumerId) return setError("Pick a consumer");
		if (!intent) return setError("Pick an intent the consumer advertises");
		if (!name.trim()) return setError("Name cannot be empty");
		// The backend parses the human-friendly duration (e.g. "2h 30m") and
		// rejects anything invalid with an error shown inline.
		const overdue_after = overdue.trim() === "" ? null : overdue.trim();
		const params = paramValuesToWire(paramSchema, paramValues);
		if (typeof params === "string") return setError(params);
		setPending(true);
		setError(null);
		try {
			await callApi("restore_replicas", "create", {
				consumer_device_id: consumerId,
				group_id: groupId,
				machine_id: serverId || null,
				type,
				intent,
				name: name.trim(),
				overdue_after,
				params,
				redacts,
				publishes_schemas: publishesSchemas,
			});
			onCreated();
		} catch (err) {
			setError(formatError(err));
			setPending(false);
		}
	};

	return (
		<Dialog open onClose={() => !pending && onClose()} fullWidth maxWidth="sm">
			<DialogTitle>Declare restore replica</DialogTitle>
			<DialogContent>
				<Stack spacing={2} sx={{ mt: 1 }}>
					<ScopeFields
						consumers={consumers}
						servers={servers}
						typeOptions={typeOptions}
						consumerId={consumerId}
						onConsumerChange={(id) => {
							setConsumerId(id);
							setError(null);
						}}
						serverId={serverId}
						onServerChange={setServerId}
						type={type}
						onTypeChange={setType}
						intent={intent}
						onIntentChange={setIntent}
						intentOptions={intentOptions}
						description={selectedDescriptor?.description}
					/>

					<TextField
						size="small"
						fullWidth
						label="Name"
						value={name}
						onChange={(e) => {
							setName(e.target.value);
							setNameEdited(true);
						}}
					/>

					<TextField
						size="small"
						fullWidth
						label="Overdue after (optional)"
						placeholder="no bound"
						helperText="e.g. 2h 30m, 36h, 1d 12h"
						value={overdue}
						onChange={(e) => setOverdue(e.target.value)}
					/>

					{canRedact && (
						<RedactionField value={redacts} onChange={setRedacts} />
					)}

					{canPublishSchemas && (
						<PublishesSchemasField
							value={publishesSchemas}
							onChange={setPublishesSchemas}
							disabled={redacts || Boolean(serverId)}
							why={
								redacts
									? "A redacting replica builds no schema: masking alters the configuration a schema follows from."
									: "A build is per group: pick every machine in the group to publish its schema."
							}
						/>
					)}

					<ParamFieldsEditor
						paramSchema={paramSchema}
						values={paramValues}
						onChange={(key, v) =>
							setParamValues((prev) => ({ ...prev, [key]: v }))
						}
					/>

					{error && <Alert severity="error">{error}</Alert>}
				</Stack>
			</DialogContent>
			<DialogActions>
				<Button onClick={onClose} disabled={pending}>
					Cancel
				</Button>
				<Button variant="contained" onClick={onSubmit} disabled={pending}>
					{pending ? "Declaring…" : "Declare"}
				</Button>
			</DialogActions>
		</Dialog>
	);
}

/** Edit every field of a declared replica, including its scope (consumer,
 * server or whole-group, type, intent) — the most useful edit being a change
 * of intent, e.g. retargeting a `verify` replica to `analytics`. Parameter
 * fields re-derive from the selected consumer+intent's current schema
 * whenever either changes, carrying forward values for parameter names that
 * still exist and dropping the rest. When the selected consumer+intent has
 * no schema (a gap), there are no fields to render and the stored parameter
 * values are carried through unchanged, matching create's gap behaviour. A
 * scope change that collides with another declaration comes back as an
 * inline error. */
function EditReplicaDialog({
	groupId,
	replica,
	consumers,
	onClose,
	onUpdated,
}: {
	groupId: string;
	replica: RestoreReplicaView;
	consumers: RestoreConsumerView[];
	onClose: () => void;
	onUpdated: () => void;
}) {
	const { servers, typeOptions } = useGroupScopeData(groupId);

	const [consumerId, setConsumerId] = useState(replica.consumer_device_id);
	const [serverId, setServerId] = useState(replica.machine_id ?? ""); // "" = whole group
	const [type, setType] = useState(replica.type);
	const [intent, setIntent] = useState(replica.intent);
	const [name, setName] = useState(replica.name);
	const [overdue, setOverdue] = useState(replica.overdue_after ?? "");
	const [enabled, setEnabled] = useState(replica.enabled);
	const [redacts, setRedacts] = useState(replica.redacts);
	const [publishesSchemas, setPublishesSchemas] = useState(
		replica.publishes_schemas,
	);
	const [paramValues, setParamValues] = useState<Record<string, string>>(() => {
		const initialDescriptor = consumers
			.find((c) => c.device_id === replica.consumer_device_id)
			?.intents.find((d) => d.intent === replica.intent);
		const initialSchema =
			(initialDescriptor?.params as Record<string, ParamSpec> | undefined) ?? {};
		return wireParamsToValues(initialSchema, replica.params);
	});
	const [pending, setPending] = useState(false);
	const [error, setError] = useState<string | null>(null);

	const {
		intentOptions,
		selectedDescriptor,
		paramSchema,
		canRedact,
		canPublishSchemas,
	} = useIntentSchema(consumers, consumerId, intent);

	// Retargeting to an intent that can't redact drops the flag with it, so the
	// declaration doesn't carry an intent the new consumer can't honour.
	useEffect(() => {
		if (!canRedact) setRedacts(false);
	}, [canRedact]);

	// A build is dispatched per group from unmasked data, so narrowing the
	// declaration to a machine or turning redaction on drops the flag with it.
	useEffect(() => {
		if (!canPublishSchemas || redacts || serverId) setPublishesSchemas(false);
	}, [canPublishSchemas, redacts, serverId]);

	// Re-derive parameter values whenever the consumer or intent changes: keep
	// values for parameter names the new schema still has, drop the rest.
	useEffect(() => {
		setParamValues((prev) => {
			const next: Record<string, string> = {};
			for (const key of Object.keys(paramSchema)) {
				if (prev[key] !== undefined) next[key] = prev[key];
			}
			return next;
		});
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [consumerId, intent]);

	const onSubmit = async () => {
		if (!consumerId) return setError("Pick a consumer");
		if (!intent) return setError("Pick an intent the consumer advertises");
		if (!name.trim()) return setError("Name cannot be empty");
		// The backend parses the human-friendly duration (e.g. "2h 30m") and
		// rejects anything invalid with an error shown inline.
		const overdue_after = overdue.trim() === "" ? null : overdue.trim();
		// With no schema (a gap intent) there are no param fields; carry the
		// stored values through unchanged rather than wiping them.
		let params: unknown = replica.params;
		if (Object.keys(paramSchema).length > 0) {
			const built = paramValuesToWire(paramSchema, paramValues);
			if (typeof built === "string") return setError(built);
			params = built;
		}
		setPending(true);
		setError(null);
		try {
			await callApi("restore_replicas", "update", {
				id: replica.id,
				consumer_device_id: consumerId,
				group_id: groupId,
				machine_id: serverId || null,
				type,
				intent,
				name: name.trim(),
				overdue_after,
				params,
				redacts,
				publishes_schemas: publishesSchemas,
				enabled,
			});
			onUpdated();
		} catch (err) {
			setError(formatError(err));
			setPending(false);
		}
	};

	return (
		<Dialog open onClose={() => !pending && onClose()} fullWidth maxWidth="sm">
			<DialogTitle>Edit restore replica</DialogTitle>
			<DialogContent>
				<Stack spacing={2} sx={{ mt: 1 }}>
					<ScopeFields
						consumers={consumers}
						servers={servers}
						typeOptions={typeOptions}
						consumerId={consumerId}
						onConsumerChange={(id) => {
							setConsumerId(id);
							setError(null);
						}}
						serverId={serverId}
						onServerChange={setServerId}
						type={type}
						onTypeChange={setType}
						intent={intent}
						onIntentChange={setIntent}
						intentOptions={intentOptions}
						description={selectedDescriptor?.description}
					/>

					<TextField
						size="small"
						fullWidth
						label="Name"
						value={name}
						onChange={(e) => setName(e.target.value)}
					/>

					<TextField
						size="small"
						fullWidth
						label="Overdue after (optional)"
						placeholder="no bound"
						helperText="e.g. 2h 30m, 36h, 1d 12h"
						value={overdue}
						onChange={(e) => setOverdue(e.target.value)}
					/>

					<FormControlLabel
						control={
							<Switch
								checked={enabled}
								onChange={(e) => setEnabled(e.target.checked)}
							/>
						}
						label="Enabled"
					/>

					{canRedact && (
						<RedactionField value={redacts} onChange={setRedacts} />
					)}

					{canPublishSchemas && (
						<PublishesSchemasField
							value={publishesSchemas}
							onChange={setPublishesSchemas}
							disabled={redacts || Boolean(serverId)}
							why={
								redacts
									? "A redacting replica builds no schema: masking alters the configuration a schema follows from."
									: "A build is per group: pick every machine in the group to publish its schema."
							}
						/>
					)}

					<ParamFieldsEditor
						paramSchema={paramSchema}
						values={paramValues}
						onChange={(key, v) =>
							setParamValues((prev) => ({ ...prev, [key]: v }))
						}
					/>

					{error && <Alert severity="error">{error}</Alert>}
				</Stack>
			</DialogContent>
			<DialogActions>
				<Button onClick={onClose} disabled={pending}>
					Cancel
				</Button>
				<Button variant="contained" onClick={onSubmit} disabled={pending}>
					{pending ? "Saving…" : "Save"}
				</Button>
			</DialogActions>
		</Dialog>
	);
}

/** The outcome/status chip for a restore-activity row. A reported check shows
 * healthy/failed; an inferred row shows in-progress (creds still valid, no report
 * yet) or unknown (creds expired without a report). */
function RestoreOutcomeChip({ check }: { check: RestoreActivity }) {
	if (check.status === "reported") {
		const ok = check.outcome === "success" && check.replica_healthy === true;
		return (
			<Chip
				label={ok ? "healthy" : "failed"}
				color={ok ? "success" : "error"}
				size="small"
			/>
		);
	}
	if (check.status === "in_progress") {
		return (
			<Tooltip
				title={
					<>
						Credentials issued <TimeAgo timestamp={check.started_at ?? check.at} />;
						awaiting the restore report.
					</>
				}
			>
				<Chip size="small" color="info" variant="outlined" label="in progress" />
			</Tooltip>
		);
	}
	return (
		<Tooltip title="Restore credentials were issued but no report was ever received.">
			<Chip size="small" variant="outlined" label="unknown" />
		</Tooltip>
	);
}

/** What the masking manifest did, for a report from a replica that redacts.
 * A partial redaction is the one that needs saying out loud: the replica is
 * live and mostly masked, with columns in the clear that only the consumer's
 * logs name. */
function RedactionOutcomeChip({ check }: { check: RestoreActivity }) {
	if (check.redaction_outcome == null) return <>—</>;
	const masked = check.redaction_columns_masked;
	const skipped = check.redaction_columns_skipped;
	const version = check.redaction_manifest_version;
	const detail =
		check.redaction_outcome === "failed"
			? (check.redaction_error ??
				"No masking took effect; the replica stayed on its previous data.")
			: [
					masked != null ? `${masked} columns masked` : null,
					skipped ? `${skipped} left in the clear` : null,
					version ? `manifest ${version}` : null,
				]
					.filter(Boolean)
					.join(", ");
	const color =
		check.redaction_outcome === "complete"
			? ("success" as const)
			: ("warning" as const);
	return (
		<Tooltip title={detail}>
			<Chip label={check.redaction_outcome} color={color} size="small" />
		</Tooltip>
	);
}

/** One labelled line of a report's promoted detail; renders nothing when the
 * value is absent, so an unreported field leaves no empty row behind. */
function DetailLine({
	label,
	value,
}: {
	label: string;
	value: string | number | null | undefined;
}) {
	if (value == null) return null;
	return (
		<Typography variant="body2">
			<Box component="span" sx={{ color: "text.secondary" }}>
				{label}:{" "}
			</Box>
			{value}
		</Typography>
	);
}

/** One restore-activity row: a reported health check, or a restore inferred from
 * a credential issuance that never reported. When the consumer sent arbitrary
 * `health_details`, or the report carried a redaction, the row expands to
 * reveal them; a `url` in the details is surfaced as a link to the running
 * replica. */
function CheckRow({ check }: { check: RestoreActivity }) {
	const [open, setOpen] = useState(false);
	const url = healthUrl(check.health_details);
	const hasHealthJson =
		check.health_details != null &&
		!(
			typeof check.health_details === "object" &&
			Object.keys(check.health_details).length === 0
		);
	const hasDetails = hasHealthJson || check.redaction_outcome != null;
	return (
		<>
			<TableRow sx={hasDetails ? { "& > *": { borderBottom: "unset" } } : undefined}>
				<TableCell padding="checkbox">
					{hasDetails && (
						<IconButton
							size="small"
							aria-label={open ? "Hide health details" : "Show health details"}
							onClick={() => setOpen((o) => !o)}
						>
							{open ? <KeyboardArrowUpIcon /> : <KeyboardArrowDownIcon />}
						</IconButton>
					)}
				</TableCell>
				<TableCell>
					<TimeAgo timestamp={check.at} />
				</TableCell>
				<TableCell>
					{check.machine_id ? check.machine_id.slice(0, 8) : "—"}
				</TableCell>
				<TableCell>{check.type}</TableCell>
				<TableCell>{check.intent ?? "—"}</TableCell>
				<TableCell>
					<RestoreOutcomeChip check={check} />
				</TableCell>
				<TableCell>
					{check.duration_seconds != null ? (
						<span>{humanSeconds(check.duration_seconds)}</span>
					) : (
						"—"
					)}
				</TableCell>
				<TableCell>{check.postgres_version ?? "—"}</TableCell>
				<TableCell>
					<RedactionOutcomeChip check={check} />
				</TableCell>
				<TableCell>
					{check.snapshot_id ? check.snapshot_id.slice(0, 12) : "—"}
				</TableCell>
				<TableCell>
					{url ? (
						<Link
							href={url}
							target="_blank"
							rel="noopener noreferrer"
							sx={{ display: "inline-flex", alignItems: "center", gap: 0.5 }}
						>
							open
							<OpenInNewIcon fontSize="inherit" />
						</Link>
					) : (
						"—"
					)}
				</TableCell>
			</TableRow>
			{hasDetails && (
				<TableRow>
					<TableCell sx={{ py: 0 }} colSpan={11}>
						<Collapse in={open} timeout="auto" unmountOnExit>
							{check.redaction_outcome != null && (
								<Box sx={{ my: 1 }}>
									<Typography variant="caption" color="text.secondary">
										Redaction
									</Typography>
									<Stack sx={{ mt: 0.5 }}>
										<DetailLine label="Outcome" value={check.redaction_outcome} />
										<DetailLine
											label="Manifest version"
											value={check.redaction_manifest_version}
										/>
										<DetailLine
											label="Columns masked"
											value={check.redaction_columns_masked}
										/>
										<DetailLine
											label="Columns left in the clear"
											value={check.redaction_columns_skipped}
										/>
										<DetailLine label="Error" value={check.redaction_error} />
									</Stack>
								</Box>
							)}
							{hasHealthJson && (
								<Box sx={{ my: 1 }}>
									<Typography variant="caption" color="text.secondary">
										Health details
									</Typography>
									<Box
										component="pre"
										sx={{
											m: 0,
											mt: 0.5,
											p: 1,
											fontSize: "0.75rem",
											overflowX: "auto",
											bgcolor: "action.hover",
											borderRadius: 1,
										}}
									>
										{JSON.stringify(check.health_details, null, 2)}
									</Box>
								</Box>
							)}
						</Collapse>
					</TableCell>
				</TableRow>
			)}
		</>
	);
}
