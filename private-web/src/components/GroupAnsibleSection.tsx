import DeleteIcon from "@mui/icons-material/Delete";
import EditIcon from "@mui/icons-material/Edit";
import EditNoteIcon from "@mui/icons-material/EditNote";
import ErrorOutlineIcon from "@mui/icons-material/ErrorOutlined";
import ExpandLessIcon from "@mui/icons-material/ExpandLess";
import ExpandMoreIcon from "@mui/icons-material/ExpandMore";
import HistoryIcon from "@mui/icons-material/History";
import LockIcon from "@mui/icons-material/Lock";
import UploadFileIcon from "@mui/icons-material/UploadFile";
import {
	Alert,
	Box,
	Button,
	ButtonBase,
	Chip,
	Collapse,
	FormControlLabel,
	IconButton,
	LinearProgress,
	MenuItem,
	Paper,
	Snackbar,
	Stack,
	Switch,
	TextField,
	Tooltip,
	Typography,
} from "@mui/material";
import { useState } from "react";
import { useApi, useApiAction } from "../api";
import { useIsAdmin } from "../hooks/useIsAdmin";
import type { AnsibleVariableView, ServerRank } from "../types";
import { SERVER_RANK_ORDER } from "../types";
import AnsibleImportDialog from "./AnsibleImportDialog";
import AnsibleRuns from "./AnsibleRuns";
import { GradedAction } from "./GradedAction";
import TimeAgo from "./TimeAgo";
import ScopeTextEditor from "./ansible/ScopeTextEditor";
import VariableEditRow from "./ansible/VariableEditRow";
import VariableHistory from "./ansible/VariableHistory";
import VariableEntryRow from "./ansible/VariableEntryRow";
import { MANAGED, type ScopeKind, truthy } from "./ansible/schema";

type Machine = {
	id: string;
	name?: string | null;
	environment_rank?: ServerRank | null;
};

type Application = {
	machine_id: string;
	type: string;
};

/// What a configuration run would be served for one of this group's
/// environments: the variables that configure it, with a value inherited from a
/// wider scope told apart from one a machine sets itself. A secret appears by
/// name and never by value.
// spec: ANSVAR#presentation
export default function GroupAnsibleSection({
	groupId,
	groupName,
	machines,
	applications,
	maintenanceTick,
	onMaintenanceChange,
}: {
	groupId: string;
	groupName: string;
	machines: ReadonlyArray<Machine>;
	applications: ReadonlyArray<Application>;
	/// Bumped when a window over the group is declared or lifted anywhere on
	/// the page, since that changes whether a run here could take the lease.
	maintenanceTick: number;
	onMaintenanceChange: () => void;
}) {
	const [tick, setTick] = useState(0);
	const [selected, setSelected] = useState<ServerRank | null>(null);
	const reload = () => setTick((n) => n + 1);
	const variables = useApi(
		"ansible_variables",
		"for_group",
		{ server_group_id: groupId },
		[groupId, tick],
	);
	// Read once for the group: it changes at most hourly, not with each edit.
	const catalogue = useApi("ansible_variables", "catalogue", {}, [groupId]);

	// A group's environments are the ranks its machines sit at, whether or not
	// anything on them has reported. A pending machine is in none.
	// spec: GRP#environments
	const ranks = SERVER_RANK_ORDER.filter((rank) =>
		machines.some((machine) => machine.environment_rank === rank),
	);
	const rank =
		selected !== null && ranks.includes(selected) ? selected : ranks[0];

	return (
		<Box data-testid="group-ansible">
			<Typography variant="h6" gutterBottom>
				Ansible
			</Typography>
			{variables.status === "loading" && <LinearProgress />}
			{variables.status === "error" && (
				<Alert severity="warning">{variables.error.message}</Alert>
			)}
			{ranks.length === 0 ? (
				<Paper variant="outlined" sx={{ p: 2 }}>
					<Typography variant="body2" color="text.secondary">
						Nothing is ranked yet, so there is no environment to configure.
					</Typography>
				</Paper>
			) : (
				<EnvironmentAnsible
					groupId={groupId}
					groupName={groupName}
					rank={rank}
					ranks={ranks}
					onSelect={setSelected}
					machines={machines.filter(
						(machine) => machine.environment_rank === rank,
					)}
					applications={applications}
					variables={variables.status === "ok" ? variables.data : null}
					catalogue={catalogue.status === "ok" ? catalogue.data : null}
					onChanged={reload}
					maintenanceTick={maintenanceTick}
					onMaintenanceChange={onMaintenanceChange}
				/>
			)}
		</Box>
	);
}

function EnvironmentAnsible({
	groupId,
	groupName,
	rank,
	ranks,
	onSelect,
	machines,
	applications,
	variables,
	catalogue,
	onChanged,
	maintenanceTick,
	onMaintenanceChange,
}: {
	groupId: string;
	groupName: string;
	rank: ServerRank;
	ranks: ReadonlyArray<ServerRank>;
	onSelect: (rank: ServerRank) => void;
	machines: ReadonlyArray<Machine>;
	applications: ReadonlyArray<Application>;
	/** Null until they have loaded, since whether they start collapsed turns on
	 * how many there are. */
	variables: ReadonlyArray<AnsibleVariableView> | null;
	catalogue: Catalogue | null;
	onChanged: () => void;
	maintenanceTick: number;
	onMaintenanceChange: () => void;
}) {
	const loaded = variables !== null;
	const all = variables ?? [];
	// Whether a machine is managed is its switch, so it is not listed or counted
	// as a variable.
	const listed = all.filter((variable) => variable.name !== MANAGED);
	const managedOn = (machineId: string) =>
		all.some(
			(variable) =>
				variable.machine_id === machineId &&
				variable.name === MANAGED &&
				truthy(variable.value),
		);
	const machineIds = new Set(machines.map((machine) => machine.id));
	const groupVars = listed.filter(
		(variable) => variable.server_group_id && !variable.rank && !variable.machine_id,
	);
	const environmentVars = listed.filter((variable) => variable.rank === rank);
	const machineVars = (machineId: string) =>
		listed.filter((variable) => variable.machine_id === machineId);
	const environmentCount =
		environmentVars.length +
		listed.filter(
			(variable) => variable.machine_id && machineIds.has(variable.machine_id),
		).length;
	const count = groupVars.length + environmentCount;

	return (
		<Paper variant="outlined" sx={{ p: 2 }} data-testid={`environment-${rank}`}>
			{ranks.length > 1 ? (
				<TextField
					select
					size="small"
					label="Environment"
					value={rank}
					onChange={(event) => onSelect(event.target.value as ServerRank)}
					sx={{ minWidth: 180 }}
				>
					{ranks.map((option) => (
						<MenuItem key={option} value={option}>
							{option}
						</MenuItem>
					))}
				</TextField>
			) : (
				<Typography
					variant="overline"
					color="text.secondary"
					sx={{ display: "block" }}
				>
					{rank}
				</Typography>
			)}

			<Stack spacing={2} sx={{ mt: 1 }}>
				<AnsibleRuns
					groupId={groupId}
					groupName={groupName}
					rank={rank}
					machines={machines}
					applications={applications}
					catalogue={catalogue}
					maintenanceTick={maintenanceTick}
					onChanged={onMaintenanceChange}
				/>

				{loaded && (
				<Variables
					key={rank}
					groupId={groupId}
					groupName={groupName}
					rank={rank}
					machines={machines}
					applications={applications}
					groupVars={groupVars}
					environmentVars={environmentVars}
					machineVars={machineVars}
					managedOn={managedOn}
					count={count}
					importable={environmentCount === 0}
					catalogue={catalogue}
					onChanged={onChanged}
				/>
				)}
			</Stack>
		</Paper>
	);
}

type Catalogue = import("../types").CatalogueView;

/// The variables of an environment: collapsed to a count at rest, and by scope
/// when open.
function Variables({
	groupId,
	groupName,
	rank,
	machines,
	applications,
	groupVars,
	environmentVars,
	machineVars,
	managedOn,
	count,
	importable,
	catalogue,
	onChanged,
}: {
	groupId: string;
	groupName: string;
	rank: ServerRank;
	machines: ReadonlyArray<Machine>;
	applications: ReadonlyArray<Application>;
	groupVars: ReadonlyArray<AnsibleVariableView>;
	environmentVars: ReadonlyArray<AnsibleVariableView>;
	machineVars: (machineId: string) => ReadonlyArray<AnsibleVariableView>;
	managedOn: (machineId: string) => boolean;
	count: number;
	importable: boolean;
	catalogue: Catalogue | null;
	onChanged: () => void;
}) {
	const isAdmin = useIsAdmin() === true;
	const [open, setOpen] = useState(count === 0);
	const [importing, setImporting] = useState(false);

	const typesOn = (machineIds: ReadonlyArray<string>) => [
		...new Set(
			applications
				.filter((application) => machineIds.includes(application.machine_id))
				.map((application) => application.type),
		),
	];
	const environmentTypes = typesOn(machines.map((machine) => machine.id));

	return (
		<Box data-testid="ansible-variables">
			<ButtonBase
				onClick={() => setOpen((o) => !o)}
				aria-expanded={open}
				data-testid="variables-toggle"
				sx={{ gap: 0.5, alignItems: "center", borderRadius: 1, pr: 1 }}
			>
				{open ? <ExpandLessIcon fontSize="small" /> : <ExpandMoreIcon fontSize="small" />}
				<Typography variant="subtitle2">Variables</Typography>
				<Chip size="small" label={count} data-testid="variables-count" />
			</ButtonBase>
			<Collapse in={open} unmountOnExit>
				<Stack spacing={2} sx={{ mt: 1 }}>
					<Scope
						title={groupName}
						sub="all environments"
						scope={{ server_group_id: groupId }}
						kind="group"
						variables={groupVars}
						inherited={[]}
						applicationTypes={typesOn(applications.map((a) => a.machine_id))}
						catalogue={catalogue}
						isAdmin={isAdmin}
						onChanged={onChanged}
					/>
					<Nested>
						<Scope
							title={rank}
							scope={{ server_group_id: groupId, rank }}
							kind="environment"
							variables={environmentVars}
							inherited={groupVars}
							applicationTypes={environmentTypes}
							catalogue={catalogue}
							isAdmin={isAdmin}
							onChanged={onChanged}
							after={
								isAdmin && importable ? (
									<GradedAction
										calls="ansible/import_apply"
										action="Import an Ansible inventory file"
									>
										<Button
											size="small"
											variant="outlined"
											startIcon={<UploadFileIcon />}
											onClick={() => setImporting(true)}
											data-testid="import-inventory"
										>
											Import inventory file
										</Button>
									</GradedAction>
								) : null
							}
						/>
						<Nested>
							{machines.map((machine) => (
								<Scope
									key={machine.id}
									title={machine.name ?? machine.id}
									scope={{ machine_id: machine.id }}
									kind="machine"
									variables={machineVars(machine.id)}
									inherited={[...groupVars, ...environmentVars]}
									applicationTypes={typesOn([machine.id])}
									catalogue={catalogue}
									isAdmin={isAdmin}
									onChanged={onChanged}
									managed={managedOn(machine.id)}
									testId="ansible-machine"
								/>
							))}
						</Nested>
					</Nested>
				</Stack>
			</Collapse>
			{isAdmin && (
				<AnsibleImportDialog
					open={importing}
					onClose={() => setImporting(false)}
					groupId={groupId}
					rank={rank}
					onImported={onChanged}
				/>
			)}
		</Box>
	);
}

/// What a scope holds, indented beneath it.
function Nested({ children }: { children: React.ReactNode }) {
	return (
		<Stack spacing={2} sx={{ pl: 2, borderLeft: 1, borderColor: "divider" }}>
			{children}
		</Stack>
	);
}

/// Whether a run acts on a machine.
// spec: INV#managed-machines
function ManagedSwitch({
	machineId,
	name,
	managed,
	isAdmin,
	onChanged,
}: {
	machineId: string;
	name: string;
	managed: boolean;
	isAdmin: boolean;
	onChanged: () => void;
}) {
	const set = useApiAction("ansible_variables", "set");
	return (
		<>
			<GradedAction
				calls="ansible_variables/set"
				action={`Change whether Ansible manages ${name}`}
			>
				<FormControlLabel
					control={
						<Switch
							size="small"
							checked={managed}
							disabled={!isAdmin || set.pending}
							onChange={(event) =>
								set
									.call({
										machine_id: machineId,
										name: MANAGED,
										value: event.target.checked,
										secret: false,
									})
									.then(onChanged)
									.catch(() => {
										// Surfaced by the alert beside it.
									})
							}
							slotProps={{ input: { "aria-label": `${name} is managed by Ansible` } }}
							data-testid="managed-switch"
						/>
					}
					label={<Typography variant="caption">Managed by Ansible</Typography>}
					sx={{ ml: 1 }}
				/>
			</GradedAction>
			{set.error && (
				<Typography variant="caption" color="error">
					{set.error.message}
				</Typography>
			)}
		</>
	);
}

/// One scope's variables, its entry row, and the way to edit it as text. A
/// machine's scope carries its managed switch, and one that is not managed is
/// greyed and starts collapsed.
function Scope({
	title,
	sub,
	scope,
	kind,
	variables,
	inherited,
	applicationTypes,
	catalogue,
	isAdmin,
	onChanged,
	after,
	managed,
	testId,
}: {
	title: string;
	sub?: string;
	scope: Record<string, unknown>;
	kind: ScopeKind;
	variables: ReadonlyArray<AnsibleVariableView>;
	inherited: ReadonlyArray<AnsibleVariableView>;
	applicationTypes: ReadonlyArray<string>;
	catalogue: Catalogue | null;
	isAdmin: boolean;
	onChanged: () => void;
	after?: React.ReactNode;
	/** Whether the machine is managed by Ansible, for a machine's scope. */
	managed?: boolean;
	testId?: string;
}) {
	const [editing, setEditing] = useState(false);
	const [expanded, setExpanded] = useState(managed !== false);
	const greyed = managed === false;
	const remove = useApiAction("ansible_variables", "remove");
	const restore = useApiAction("ansible_variables", "restore");
	// The removal just made, offered to undo until the offer is dismissed.
	const [undo, setUndo] = useState<{ name: string; entryId: string } | null>(null);
	// The history open: of one variable, or of the whole scope where null.
	const [historyOf, setHistoryOf] = useState<{ name: string | null } | null>(null);
	const removed = useApi(
		"ansible_variables",
		"removed",
		scope,
		[
			JSON.stringify(scope),
			variables.map((variable) => `${variable.id}@${variable.updated_at}`).join(),
		],
		{ skip: !isAdmin },
	);
	const removeVariable = (name: string) =>
		remove.call({ ...scope, name }).then((removal) => {
			setUndo({ name, entryId: removal.entry_id });
			onChanged();
		});
	const undoRemoval = () => {
		if (!undo) return;
		setUndo(null);
		restore
			.call({ ...scope, name: undo.name, entry_id: undo.entryId })
			.then(onChanged)
			.catch(() => {
				// Surfaced by the alert on the scope.
			});
	};
	const taken = new Set(variables.map((variable) => variable.name));
	const sorted = [...variables].sort((a, b) => a.name.localeCompare(b.name));

	return (
		<Box data-testid={testId ?? `scope-${kind}`} onKeyDown={(event) => {
			// Ctrl+Shift+E edits the scope that holds focus as text.
			if (isAdmin && event.ctrlKey && event.shiftKey && event.key === "E") {
				event.preventDefault();
				setEditing(true);
			}
		}}>
			<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
				{managed !== undefined && (
					<IconButton
						size="small"
						aria-expanded={expanded}
						aria-label={expanded ? `Collapse ${title}` : `Expand ${title}`}
						onClick={() => setExpanded((e) => !e)}
						data-testid="scope-expand"
					>
						{expanded ? (
							<ExpandLessIcon fontSize="small" />
						) : (
							<ExpandMoreIcon fontSize="small" />
						)}
					</IconButton>
				)}
				<Typography
					variant="subtitle2"
					color={greyed ? "text.disabled" : undefined}
				>
					{title}
				</Typography>
				{sub && (
					<Typography variant="caption" color="text.secondary">
						{sub}
					</Typography>
				)}
				{managed !== undefined && "machine_id" in scope && (
					<ManagedSwitch
						machineId={String(scope.machine_id)}
						name={title}
						managed={managed}
						isAdmin={isAdmin}
						onChanged={onChanged}
					/>
				)}
				{isAdmin && (
					<Tooltip title="History">
						<IconButton
							size="small"
							aria-label={`History of ${title}`}
							onClick={() => setHistoryOf({ name: null })}
							data-testid="scope-history"
						>
							<HistoryIcon fontSize="small" />
						</IconButton>
					</Tooltip>
				)}
				{isAdmin && !editing && expanded && (
					<Tooltip title="Edit as text">
						<IconButton
							size="small"
							aria-label={`Edit ${title} as text`}
							onClick={() => setEditing(true)}
							data-testid="scope-edit"
						>
							<EditNoteIcon fontSize="small" />
						</IconButton>
					</Tooltip>
				)}
			</Stack>
			<Collapse in={expanded}>
				<Box sx={greyed ? { opacity: 0.6 } : undefined}>
					{editing ? (
						<ScopeTextEditor
							scope={scope}
							kind={kind}
							applicationTypes={applicationTypes}
							catalogue={catalogue}
							variables={variables.map((variable) => ({
								name: variable.name,
								value: variable.is_secret ? undefined : variable.value,
								secret: variable.is_secret,
							}))}
							onClose={() => setEditing(false)}
							onSaved={onChanged}
						/>
					) : (
						<>
							{sorted.length === 0 && !isAdmin && (
								<Typography variant="body2" color="text.secondary">
									None set
								</Typography>
							)}
							<Stack spacing={0.25} sx={{ mt: 0.5 }}>
								{sorted.map((variable) => (
									<VariableRow
										key={variable.id}
										variable={variable}
										overrides={inherited.find(
											(other) =>
												other.name === variable.name && other.id !== variable.id,
										)}
										scope={scope}
										catalogue={catalogue}
										onRemove={isAdmin ? removeVariable : undefined}
										onHistory={() => setHistoryOf({ name: variable.name })}
										onChanged={onChanged}
									/>
								))}
							</Stack>
							{(remove.error ?? restore.error) && (
								<Alert severity="warning" sx={{ mt: 1 }}>
									{(remove.error ?? restore.error)?.message}
								</Alert>
							)}
							{removed.status === "ok" && removed.data.length > 0 && (
								<Stack
									direction="row"
									spacing={0.5}
									useFlexGap
									sx={{ flexWrap: "wrap", alignItems: "center", mt: 0.5 }}
									data-testid="removed-variables"
								>
									<Typography variant="caption" color="text.secondary">
										Removed this week
									</Typography>
									{removed.data.map((variable) => (
										<Chip
											key={variable.name}
											size="small"
											variant="outlined"
											label={variable.name}
											onClick={() => setHistoryOf({ name: variable.name })}
											sx={{ fontFamily: "monospace" }}
										/>
									))}
								</Stack>
							)}
							{isAdmin && (
								<Box sx={{ mt: 0.5 }}>
									<VariableEntryRow
										label={title}
										scope={scope}
										kind={kind}
										applicationTypes={applicationTypes}
										catalogue={catalogue}
										taken={taken}
										onSet={onChanged}
									>
										{after}
									</VariableEntryRow>
								</Box>
							)}
							{!isAdmin && after}
						</>
					)}
				</Box>
			</Collapse>
			{historyOf !== null && (
				<VariableHistory
					scope={scope}
					name={historyOf.name}
					title={title}
					onClose={() => setHistoryOf(null)}
					onChanged={onChanged}
				/>
			)}
			<Snackbar
				open={undo !== null}
				autoHideDuration={10000}
				onClose={(_, reason) => {
					if (reason !== "clickaway") setUndo(null);
				}}
				message={undo ? `Removed ${undo.name}` : ""}
				action={
					<Button size="small" color="inherit" onClick={undoRemoval} data-testid="undo-removal">
						Undo
					</Button>
				}
			/>
		</Box>
	);
}

/// One variable of a scope: its name and value, a secret by name only, what it
/// overrides, and what the catalogue now says of it. An administrator can edit
/// its value in place, or remove it.
function VariableRow({
	variable,
	overrides,
	scope,
	catalogue,
	onRemove,
	onHistory,
	onChanged,
}: {
	variable: AnsibleVariableView;
	overrides?: AnsibleVariableView;
	scope: Record<string, unknown>;
	catalogue: Catalogue | null;
	/** Remove it, for an administrator. */
	onRemove?: (name: string) => Promise<unknown>;
	onHistory: () => void;
	onChanged: () => void;
}) {
	const [editing, setEditing] = useState(false);
	const testid = variable.is_secret
		? "secret-var"
		: overrides
			? "overriding-var"
			: "var";
	return (
		<Stack
			direction="row"
			spacing={1}
			sx={{ alignItems: "center", minHeight: 28 }}
			data-testid={testid}
		>
			<Typography
				variant="body2"
				sx={{ fontFamily: "monospace", width: "34%", flexShrink: 0 }}
			>
				{variable.name}
			</Typography>
			<Box sx={{ flex: 1, minWidth: 0 }}>
				{editing ? (
					<VariableEditRow
						name={variable.name}
						value={variable.value}
						secret={variable.is_secret}
						scope={scope}
						catalogue={catalogue}
						onClose={() => setEditing(false)}
						onSaved={onChanged}
					/>
				) : variable.is_secret ? (
					<Chip
						size="small"
						icon={<LockIcon />}
						label="secret"
						color="warning"
						variant="outlined"
					/>
				) : (
					<Typography
						variant="body2"
						sx={{ fontFamily: "monospace", wordBreak: "break-all" }}
					>
						{format(variable.value)}
					</Typography>
				)}
			</Box>
			{overrides && (
				<Chip
					size="small"
					variant="outlined"
					label={`overrides ${overrides.rank ?? "the group"}`}
				/>
			)}
			{variable.problems.length > 0 && (
				<Tooltip title={variable.problems.join("; ")}>
					<ErrorOutlineIcon
						fontSize="small"
						color="warning"
						data-testid="variable-problem"
					/>
				</Tooltip>
			)}
			<Typography variant="caption" color="text.secondary">
				<TimeAgo timestamp={variable.updated_at} />
			</Typography>
			{onRemove && !editing && (
				<Tooltip title="History">
					<IconButton
						size="small"
						aria-label={`history of ${variable.name}`}
						data-testid={`history-${variable.name}`}
						onClick={onHistory}
					>
						<HistoryIcon fontSize="small" />
					</IconButton>
				</Tooltip>
			)}
			{onRemove && !editing && (
				<GradedAction calls="ansible_variables/set" action={`Edit ${variable.name}`}>
					<IconButton
						size="small"
						aria-label={`edit ${variable.name}`}
						data-testid={`edit-${variable.name}`}
						onClick={() => setEditing(true)}
					>
						<EditIcon fontSize="small" />
					</IconButton>
				</GradedAction>
			)}
			{onRemove && (
				<GradedAction
					calls="ansible_variables/remove"
					action={`Remove ${variable.name}`}
				>
					<IconButton
						size="small"
						aria-label={`remove ${variable.name}`}
						data-testid={`remove-${variable.name}`}
						onClick={() =>
							onRemove(variable.name).catch(() => {
								// Surfaced by the alert on the scope.
							})
						}
					>
						<DeleteIcon fontSize="small" />
					</IconButton>
				</GradedAction>
			)}
		</Stack>
	);
}

function format(value: unknown): string {
	return JSON.stringify(value);
}
