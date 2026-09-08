import BuildOutlinedIcon from "@mui/icons-material/BuildOutlined";
import ContentCopyIcon from "@mui/icons-material/ContentCopy";
import DeleteIcon from "@mui/icons-material/Delete";
import {
	Alert,
	Box,
	Button,
	Checkbox,
	Chip,
	FormControlLabel,
	IconButton,
	LinearProgress,
	MenuItem,
	Paper,
	Stack,
	TextField,
	Tooltip,
	Typography,
} from "@mui/material";
import { useState } from "react";
import { useApi, useApiAction } from "../api";
import { useIsAdmin } from "../hooks/useIsAdmin";
import type {
	InventoryLease,
	InventoryVariable,
	MaintenanceWindow,
	ServerRank,
} from "../types";
import { SERVER_RANK_ORDER } from "../types";
import DeclareMaintenanceDialog from "./DeclareMaintenanceDialog";
import TimeAgo from "./TimeAgo";

type Machine = { id: string; name?: string | null };

/// What a configuration run receives for each of this group's environments:
/// the machines it acts on, and the variables that configure them, with a
/// value inherited from a wider scope told apart from one the machine sets
/// itself. A secret appears by name and never by value.
// spec: INV#presentation
export default function GroupInventorySection({
	groupId,
	groupName,
	applications,
	machines,
	maintenanceTick,
	onMaintenanceChange,
}: {
	groupId: string;
	groupName: string;
	applications: ReadonlyArray<{
		machine_id: string;
		rank?: ServerRank | null;
	}>;
	machines: ReadonlyArray<Machine>;
	/// Bumped when a window over the group is declared or lifted anywhere on
	/// the page, since that changes whether a run here could take the lease.
	maintenanceTick: number;
	onMaintenanceChange: () => void;
}) {
	const [tick, setTick] = useState(0);
	const reload = () => setTick((n) => n + 1);
	const variables = useApi(
		"inventory_variables",
		"for_group",
		{ server_group_id: groupId },
		[groupId, tick],
	);

	// Rank is an application's, so a group's environments are the ranks its
	// applications sit at, and one carrying no rank sits at the default.
	const ranks = SERVER_RANK_ORDER.filter((rank) =>
		applications.some((application) => (application.rank ?? "dev") === rank),
	);

	return (
		<Box data-testid="group-inventory">
			<Typography variant="h6" gutterBottom>
				Inventory
			</Typography>
			{variables.status === "loading" && <LinearProgress />}
			{variables.status === "error" && (
				<Alert severity="warning">{variables.error.message}</Alert>
			)}
			{ranks.length === 0 ? (
				<Paper variant="outlined" sx={{ p: 2 }}>
					<Typography variant="body2" color="text.secondary">
						No live applications, so there is no environment to configure.
					</Typography>
				</Paper>
			) : (
				<Stack spacing={2}>
					{ranks.map((rank) => (
						<EnvironmentInventory
							key={rank}
							groupId={groupId}
							groupName={groupName}
							rank={rank}
							machines={machines.filter((machine) =>
								applications.some(
									(application) =>
										application.machine_id === machine.id &&
										(application.rank ?? "dev") === rank,
								),
							)}
							variables={
								variables.status === "ok" ? variables.data : []
							}
							onChanged={reload}
							maintenanceTick={maintenanceTick}
							onMaintenanceChange={onMaintenanceChange}
						/>
					))}
				</Stack>
			)}
		</Box>
	);
}

function EnvironmentInventory({
	groupId,
	groupName,
	rank,
	machines,
	variables,
	onChanged,
	maintenanceTick,
	onMaintenanceChange,
}: {
	groupId: string;
	groupName: string;
	rank: ServerRank;
	machines: ReadonlyArray<Machine>;
	variables: ReadonlyArray<InventoryVariable>;
	onChanged: () => void;
	maintenanceTick: number;
	onMaintenanceChange: () => void;
}) {
	const isAdmin = useIsAdmin() === true;

	const groupVars = variables.filter(
		(variable) => variable.server_group_id && !variable.rank,
	);
	const environmentVars = variables.filter(
		(variable) => variable.rank === rank,
	);
	const machineVars = (machineId: string) =>
		variables.filter((variable) => variable.machine_id === machineId);

	return (
		<Paper variant="outlined" sx={{ p: 2 }} data-testid={`environment-${rank}`}>
			<Typography
				variant="overline"
				color="text.secondary"
				sx={{ display: "block" }}
			>
				{rank}
			</Typography>

			<Stack spacing={2} sx={{ mt: 1 }}>
				<Run
					groupId={groupId}
					groupName={groupName}
					rank={rank}
					maintenanceTick={maintenanceTick}
					onDeclared={onMaintenanceChange}
				/>

				<Box>
					<Typography variant="body2" color="text.secondary" gutterBottom>
						Group and environment variables, carried by every machine below
					</Typography>
					<Vars
						items={[...groupVars, ...environmentVars]}
						inherited={groupVars}
						scopeOf={(variable) =>
							variable.rank
								? { server_group_id: groupId, rank }
								: { server_group_id: groupId }
						}
						onRemove={isAdmin ? onChanged : undefined}
					/>
				</Box>

				{machines.map((machine) => (
					<Box key={machine.id} data-testid="inventory-machine">
						<Typography variant="subtitle2">
							{machine.name ?? machine.id}
						</Typography>
						<Vars
							items={machineVars(machine.id)}
							inherited={[...groupVars, ...environmentVars]}
							scopeOf={() => ({ machine_id: machine.id })}
							onRemove={isAdmin ? onChanged : undefined}
							empty="Sets nothing of its own"
						/>
					</Box>
				))}

				{isAdmin && (
					<SetVariable
						groupId={groupId}
						rank={rank}
						machines={machines}
						onSet={onChanged}
					/>
				)}
			</Stack>
		</Paper>
	);
}

/// The invocation a run on this environment is started with, filled in with
/// canopy's address and the environment's identity, and the lease and window
/// state that says whether a run would be served.
// spec: INV#presentation
function Run({
	groupId,
	groupName,
	rank,
	maintenanceTick,
	onDeclared,
}: {
	groupId: string;
	groupName: string;
	rank: ServerRank;
	maintenanceTick: number;
	onDeclared: () => void;
}) {
	const [copied, setCopied] = useState(false);
	const [dialogOpen, setDialogOpen] = useState(false);
	const state = useApi(
		"inventory",
		"run_state",
		{ server_group_id: groupId, rank },
		[groupId, rank, maintenanceTick],
	);

	const held: InventoryLease | null =
		state.status === "ok" ? state.data.lease : null;
	const declared: MaintenanceWindow | null =
		state.status === "ok" ? state.data.window : null;
	const refuses = state.status === "ok" && state.data.refuses;

	const command = `CANOPY_URL=${window.location.origin} CANOPY_GROUP=${shellArg(groupName)} CANOPY_RANK=${rank} ansible-playbook -i inventory/canopy.yml <playbook>`;

	const copy = async () => {
		try {
			await navigator.clipboard.writeText(command);
			setCopied(true);
			window.setTimeout(() => setCopied(false), 2000);
		} catch {
			/* clipboard may be unavailable; the line is on the page */
		}
	};

	return (
		<Box data-testid="inventory-run">
			<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
				<Typography variant="body2" color="text.secondary">
					Run
				</Typography>
				<Tooltip title={copied ? "Copied" : "Copy the command"}>
					<IconButton size="small" onClick={copy} aria-label="Copy the command">
						<ContentCopyIcon fontSize="small" />
					</IconButton>
				</Tooltip>
			</Stack>
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
				}}
			>
				{command}
			</Box>
			{held && (
				<Typography
					variant="caption"
					color="text.secondary"
					sx={{ display: "block", mt: 1 }}
					data-testid="run-leased"
				>
					{held.held_by ?? "An operator"} is running here, until{" "}
					<TimeAgo timestamp={held.expires_at} />
					{held.note ? `: ${held.note}` : ""}
				</Typography>
			)}
			{declared ? (
				<Typography
					variant="caption"
					color="text.secondary"
					sx={{ display: "block", mt: 1 }}
					data-testid="run-declared"
				>
					Work is declared here, ending{" "}
					<TimeAgo timestamp={declared.expected_end} />
					{refuses
						? `, so only ${declared.declared_by ?? "whoever declared it"} can take the lease.`
						: "."}
				</Typography>
			) : (
				<Stack spacing={0.5} sx={{ mt: 1, alignItems: "flex-start" }}>
					<Button
						size="small"
						variant="outlined"
						startIcon={<BuildOutlinedIcon />}
						onClick={() => setDialogOpen(true)}
						data-testid="declare-work"
					>
						Declare the work
					</Button>
					<Typography variant="caption" color="text.secondary">
						Declare it before running, and no one else can take the lease until
						you lift it.
					</Typography>
				</Stack>
			)}
			<DeclareMaintenanceDialog
				open={dialogOpen}
				onClose={() => setDialogOpen(false)}
				scope="group"
				id={groupId}
				targetLabel={groupName}
				prefill={{ note: `configuring ${rank}` }}
				onDone={onDeclared}
			/>
		</Box>
	);
}

/// A group name is free text, and the line is copied to be pasted into a shell.
function shellArg(value: string): string {
	if (/^[\w.:/@-]+$/.test(value)) return value;
	return `'${value.replace(/'/g, "'\\''")}'`;
}

/// Variables as chips. A secret shows its name and never its value. Where a
/// set of wider-scope variables is passed, a name present in both is marked as
/// overriding rather than adding, since that is the case an operator chasing a
/// value that isn't taking effect is looking for.
function Vars({
	items,
	inherited = [],
	scopeOf,
	onRemove,
	empty = "None set",
}: {
	items: ReadonlyArray<InventoryVariable>;
	inherited?: ReadonlyArray<InventoryVariable>;
	scopeOf: (variable: InventoryVariable) => Record<string, unknown>;
	onRemove?: () => void;
	empty?: string;
}) {
	const remove = useApiAction("inventory_variables", "remove");

	if (items.length === 0) {
		return (
			<Typography variant="body2" color="text.secondary">
				{empty}
			</Typography>
		);
	}

	return (
		<>
			<Stack
				direction="row"
				spacing={0.5}
				useFlexGap
				sx={{ flexWrap: "wrap", mt: 0.5 }}
			>
				{[...items]
					.sort((a, b) => a.name.localeCompare(b.name))
					.map((variable) => {
						const overriding = inherited.some(
							(other) =>
								other.name === variable.name && other.id !== variable.id,
						);
						const chip = (
							<Chip
								size="small"
								variant={overriding ? "filled" : "outlined"}
								color={variable.is_secret ? "warning" : "default"}
								label={`${variable.name} = ${
									variable.is_secret ? "secret" : format(variable.value)
								}`}
								data-testid={
									variable.is_secret
										? "secret-var"
										: overriding
											? "overriding-var"
											: "var"
								}
								sx={{ fontFamily: "monospace", maxWidth: "100%" }}
								onDelete={
									onRemove
										? () => {
												remove
													.call({
														...scopeOf(variable),
														name: variable.name,
													})
													.then(onRemove)
													.catch(() => {
														// Surfaced by the alert below.
													});
											}
										: undefined
								}
								deleteIcon={
									<DeleteIcon
										data-testid={`remove-${variable.name}`}
										titleAccess={`remove ${variable.name}`}
									/>
								}
							/>
						);
						return (
							<Tooltip
								key={variable.id}
								title={`Set by ${variable.set_by ?? "unknown"}, ${new Date(
									variable.updated_at,
								).toLocaleString()}${
									overriding ? "; overrides a wider scope" : ""
								}`}
							>
								{chip}
							</Tooltip>
						);
					})}
			</Stack>
			{remove.error && (
				<Alert severity="warning" sx={{ mt: 1 }}>
					{remove.error.message}
				</Alert>
			)}
		</>
	);
}

const GROUP = "group";
const ENVIRONMENT = "environment";

function SetVariable({
	groupId,
	rank,
	machines,
	onSet,
}: {
	groupId: string;
	rank: ServerRank;
	machines: ReadonlyArray<Machine>;
	onSet: () => void;
}) {
	const [where, setWhere] = useState<string>(ENVIRONMENT);
	const [name, setName] = useState("");
	const [value, setValue] = useState("");
	const [secret, setSecret] = useState(false);
	const set = useApiAction("inventory_variables", "set");

	const scope =
		where === GROUP
			? { server_group_id: groupId }
			: where === ENVIRONMENT
				? { server_group_id: groupId, rank }
				: { machine_id: where };

	const submit = () => {
		set
			.call({ ...scope, name: name.trim(), value: parse(value), secret })
			.then(() => {
				setName("");
				setValue("");
				onSet();
			})
			.catch(() => {});
	};

	return (
		<Box data-testid="set-variable">
			<Typography variant="body2" color="text.secondary" gutterBottom>
				Set a variable
			</Typography>
			<Stack direction="row" spacing={1} useFlexGap sx={{ flexWrap: "wrap" }}>
				<TextField
					select
					size="small"
					label="Scope"
					value={where}
					onChange={(event) => setWhere(event.target.value)}
					sx={{ minWidth: 180 }}
				>
					<MenuItem value={GROUP}>Whole group</MenuItem>
					<MenuItem value={ENVIRONMENT}>This environment</MenuItem>
					{machines.map((machine) => (
						<MenuItem key={machine.id} value={machine.id}>
							{machine.name ?? machine.id}
						</MenuItem>
					))}
				</TextField>
				<TextField
					size="small"
					label="Name"
					value={name}
					onChange={(event) => setName(event.target.value)}
				/>
				<TextField
					size="small"
					label="Value"
					type={secret ? "password" : "text"}
					value={value}
					onChange={(event) => setValue(event.target.value)}
				/>
				<FormControlLabel
					control={
						<Checkbox
							size="small"
							checked={secret}
							onChange={(event) => setSecret(event.target.checked)}
							data-testid="value-is-secret"
						/>
					}
					label="Secret"
				/>
				<Button
					variant="outlined"
					onClick={submit}
					disabled={set.pending || name.trim() === "" || value === ""}
				>
					Set
				</Button>
			</Stack>
			{set.error && (
				<Alert severity="warning" sx={{ mt: 1 }}>
					{set.error.message}
				</Alert>
			)}
		</Box>
	);
}

/// A typed value is JSON where it parses as something other than a bare string,
/// so `true`, `3` and `["a"]` land as themselves and everything else as text.
function parse(value: string): unknown {
	try {
		const parsed = JSON.parse(value);
		return typeof parsed === "string" ? value : parsed;
	} catch {
		return value;
	}
}

function format(value: unknown): string {
	return typeof value === "string" ? value : JSON.stringify(value);
}
