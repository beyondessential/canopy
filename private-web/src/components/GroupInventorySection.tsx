import BuildOutlinedIcon from "@mui/icons-material/BuildOutlined";
import ContentCopyIcon from "@mui/icons-material/ContentCopy";
import DeleteIcon from "@mui/icons-material/Delete";
import {
	Alert,
	Box,
	Button,
	Chip,
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
	InventorySecretVariable,
	MaintenanceWindow,
	ServerRank,
} from "../types";
import { SERVER_RANK_ORDER } from "../types";
import ApplicationTypeChip from "./ApplicationTypeChip";
import DeclareMaintenanceDialog from "./DeclareMaintenanceDialog";
import TimeAgo from "./TimeAgo";

/// What a configuration run receives for each of this group's environments:
/// the applications it would act on, the address each is reached at, and the
/// variables that configure them, with the environment's values shown once
/// rather than repeated under every application.
///
/// A secret variable appears by name and never by value, and the assembled
/// inventory is admin-only, carrying those values.
// spec: INV#presentation
export default function GroupInventorySection({
	groupId,
	applications,
	maintenanceTick,
	onMaintenanceChange,
}: {
	groupId: string;
	applications: ReadonlyArray<{ rank?: ServerRank | null }>;
	/// Bumped when a window over the group is declared or lifted anywhere on
	/// the page, since that changes what a run here would be served.
	maintenanceTick: number;
	onMaintenanceChange: () => void;
}) {
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
							rank={rank}
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
	rank,
	maintenanceTick,
	onMaintenanceChange,
}: {
	groupId: string;
	rank: ServerRank;
	maintenanceTick: number;
	onMaintenanceChange: () => void;
}) {
	const isAdmin = useIsAdmin() === true;
	const [tick, setTick] = useState(0);
	const reload = () => setTick((n) => n + 1);

	const inventory = useApi(
		"inventory",
		"for_group",
		{ server_group_id: groupId, rank },
		[groupId, rank, tick],
		{ skip: !isAdmin },
	);
	const secrets = useApi(
		"inventory_secrets",
		"for_group",
		{ server_group_id: groupId },
		[groupId, tick],
	);

	const declared = secrets.status === "ok" ? secrets.data : [];
	const environmentSecrets = declared.filter(
		(variable) => variable.rank === rank && !variable.application_id,
	);
	const applicationSecrets = (applicationId: string) =>
		declared.filter((variable) => variable.application_id === applicationId);

	const hosts = inventory.status === "ok" ? inventory.data.hosts : [];

	return (
		<Paper variant="outlined" sx={{ p: 2 }} data-testid={`environment-${rank}`}>
			<Typography
				variant="overline"
				color="text.secondary"
				sx={{ display: "block" }}
			>
				{rank}
			</Typography>

			{(inventory.status === "loading" || secrets.status === "loading") && (
				<LinearProgress />
			)}

			{inventory.status === "error" && (
				<Alert severity="warning" sx={{ mt: 1 }}>
					{inventory.error.message}
				</Alert>
			)}

			{!isAdmin && (
				<Alert severity="info" sx={{ mt: 1 }} data-testid="inventory-needs-admin">
					The assembled inventory carries the environment's secret values, so
					reading it needs admin access. The names set here are below.
				</Alert>
			)}

			{!isAdmin && (
				<Box sx={{ mt: 2 }}>
					<Typography variant="body2" color="text.secondary">
						Secret variables
					</Typography>
					<Secrets items={environmentSecrets} />
				</Box>
			)}

			{isAdmin && inventory.status === "ok" && (
				<Stack spacing={2} sx={{ mt: 1 }}>
					<Run
						groupId={groupId}
						group={inventory.data.group}
						rank={rank}
						maintenanceTick={maintenanceTick}
						onDeclared={onMaintenanceChange}
					/>

					<Box>
						<Typography variant="body2" color="text.secondary" gutterBottom>
							Environment variables, carried by every application below
						</Typography>
						<Vars
							vars={inventory.data.vars}
							secret={inventory.data.secret_vars}
						/>
						<Secrets
							items={environmentSecrets}
							onRemove={reload}
							scope={{ server_group_id: groupId, rank }}
						/>
					</Box>

					{hosts.map((host) => (
						<Box key={host.id} data-testid="inventory-application">
							<Stack
								direction="row"
								spacing={1}
								sx={{ alignItems: "baseline", flexWrap: "wrap" }}
							>
								<Typography variant="subtitle2">{host.name}</Typography>
								<ApplicationTypeChip type={host.type} />
								<Typography
									variant="body2"
									color="text.secondary"
									sx={{ fontFamily: "monospace" }}
								>
									{host.address ?? "no address"}
								</Typography>
							</Stack>
							<Vars
								vars={host.own_vars}
								overrides={inventory.data.vars}
								secret={host.secret_vars}
								empty="Sets nothing of its own"
							/>
							<Secrets
								items={applicationSecrets(host.id)}
								onRemove={reload}
								scope={{ application_id: host.id }}
							/>
						</Box>
					))}

					<SetSecret
						groupId={groupId}
						rank={rank}
						hosts={hosts}
						onSet={reload}
					/>
				</Stack>
			)}
		</Paper>
	);
}

/// The invocation a run on this environment is started with, filled in with
/// canopy's address and the environment's identity, and the declaration that
/// makes the environment the operator's to run on.
// spec: INV#presentation
function Run({
	groupId,
	group,
	rank,
	maintenanceTick,
	onDeclared,
}: {
	groupId: string;
	group: string;
	rank: ServerRank;
	maintenanceTick: number;
	onDeclared: () => void;
}) {
	const [copied, setCopied] = useState(false);
	const [dialogOpen, setDialogOpen] = useState(false);
	const windows = useApi(
		"maintenance",
		"for_target",
		{ server_group_id: groupId },
		[groupId, maintenanceTick],
	);

	// The inventory is refused while a window someone else declared holds, so
	// one holding here is the operator's own.
	const declared =
		windows.status === "ok"
			? ((windows.data as MaintenanceWindow[]).find(
					(held) => held.ended_at === null,
				) ?? null)
			: null;

	const command = `CANOPY_URL=${window.location.origin} CANOPY_GROUP=${shellArg(group)} CANOPY_RANK=${rank} ansible-playbook -i inventory/canopy.yml <playbook>`;

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
			{declared ? (
				<Typography
					variant="caption"
					color="text.secondary"
					sx={{ display: "block", mt: 1 }}
					data-testid="run-declared"
				>
					Your work is declared, ending{" "}
					<TimeAgo timestamp={declared.expected_end} />, so this inventory is
					served to your run and refused to anyone else's.
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
						Declare it before running, and the inventory is refused to a second
						operator until you lift it.
					</Typography>
				</Stack>
			)}
			<DeclareMaintenanceDialog
				open={dialogOpen}
				onClose={() => setDialogOpen(false)}
				scope="group"
				id={groupId}
				targetLabel={group}
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

/// Variables as chips. Where a set of environment values is passed, a key
/// present in both is marked as overriding rather than adding, since that is
/// the case an operator chasing a value that isn't taking effect is looking
/// for. A secret's value is never among them.
function Vars({
	vars,
	overrides,
	secret = [],
	empty = "None set",
}: {
	vars: Record<string, unknown>;
	overrides?: Record<string, unknown>;
	secret?: ReadonlyArray<string>;
	empty?: string;
}) {
	const hidden = new Set(secret);
	const keys = Object.keys(vars)
		.filter((key) => !hidden.has(key))
		.sort();
	if (keys.length === 0) {
		return (
			<Typography variant="body2" color="text.secondary">
				{empty}
			</Typography>
		);
	}
	return (
		<Stack
			direction="row"
			spacing={0.5}
			useFlexGap
			sx={{ flexWrap: "wrap", mt: 0.5 }}
		>
			{keys.map((key) => {
				const overriding =
					overrides !== undefined && key in overrides && !hidden.has(key);
				const chip = (
					<Chip
						key={key}
						size="small"
						variant={overriding ? "filled" : "outlined"}
						label={`${key} = ${format(vars[key])}`}
						data-testid={overriding ? "overriding-var" : "var"}
						sx={{ fontFamily: "monospace", maxWidth: "100%" }}
					/>
				);
				return overriding ? (
					<Tooltip key={key} title="Overrides the environment's value">
						{chip}
					</Tooltip>
				) : (
					chip
				);
			})}
		</Stack>
	);
}

/// Secret variables by name, with where each is set and when it last changed.
/// Never a value: what a run receives is served to the run.
function Secrets({
	items,
	scope,
	onRemove,
}: {
	items: ReadonlyArray<InventorySecretVariable>;
	scope?: Record<string, unknown>;
	onRemove?: () => void;
}) {
	const remove = useApiAction("inventory_secrets", "remove");
	if (items.length === 0) return null;

	return (
		<>
			<Stack
				direction="row"
				spacing={0.5}
				useFlexGap
				sx={{ flexWrap: "wrap", mt: 0.5 }}
			>
			{items.map((variable) => (
				<Tooltip
					key={variable.id}
					title={`Set by ${variable.set_by ?? "unknown"}, ${new Date(
						variable.updated_at,
					).toLocaleString()}`}
				>
					<Chip
						size="small"
						variant="outlined"
						color="warning"
						label={`${variable.name} = secret`}
						data-testid="secret-var"
						sx={{ fontFamily: "monospace", maxWidth: "100%" }}
						onDelete={
							scope && onRemove
								? () => {
										remove
											.call({ ...scope, name: variable.name })
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
				</Tooltip>
			))}
			</Stack>
			{remove.error && (
				<Alert severity="warning" sx={{ mt: 1 }}>
					{remove.error.message}
				</Alert>
			)}
		</>
	);
}

const ENVIRONMENT = "environment";

function SetSecret({
	groupId,
	rank,
	hosts,
	onSet,
}: {
	groupId: string;
	rank: ServerRank;
	hosts: ReadonlyArray<{ id: string; name: string }>;
	onSet: () => void;
}) {
	const [where, setWhere] = useState<string>(ENVIRONMENT);
	const [name, setName] = useState("");
	const [value, setValue] = useState("");
	const set = useApiAction("inventory_secrets", "set");

	const submit = () => {
		const scope =
			where === ENVIRONMENT
				? { server_group_id: groupId, rank }
				: { application_id: where };
		set
			.call({ ...scope, name: name.trim(), value })
			.then(() => {
				setName("");
				setValue("");
				onSet();
			})
			.catch(() => {});
	};

	return (
		<Box data-testid="set-secret">
			<Typography variant="body2" color="text.secondary" gutterBottom>
				Set a secret variable
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
					<MenuItem value={ENVIRONMENT}>Whole environment</MenuItem>
					{hosts.map((host) => (
						<MenuItem key={host.id} value={host.id}>
							{host.name}
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
					type="password"
					value={value}
					onChange={(event) => setValue(event.target.value)}
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

function format(value: unknown): string {
	return typeof value === "string" ? value : JSON.stringify(value);
}
