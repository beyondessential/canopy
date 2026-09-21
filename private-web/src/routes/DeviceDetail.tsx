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
	Typography,
} from "@mui/material";
import EditIcon from "@mui/icons-material/Edit";
import RefreshIcon from "@mui/icons-material/Refresh";
import { useEffect, useState } from "react";
import { useParams } from "react-router-dom";
import ServerShorty, {
	type ServerInfo as ServerShortyInfo,
} from "../components/ServerShorty";
import TailnetIdentitySection from "../components/TailnetIdentitySection";
import ProvisionCredentialDialog from "../components/ProvisionCredentialDialog";
import AddPublicKeyDialog from "../components/AddPublicKeyDialog";
import { type ApiState, callApi, useApi, useApiAction } from "../api";
import { deviceDisplayName } from "../components/DeviceShorty";
import TimeAgo from "../components/TimeAgo";
import { usePageTitle } from "../hooks/usePageTitle";
import { humanDuration } from "../lib/humanDuration";
import {
	compareServersByRankThenType,
	type DeviceConnectionData,
	type DeviceInfo,
	type DeviceKeyInfo,
	type DeviceRole,
} from "../types";

const TRUSTABLE_ROLES: DeviceRole[] = [
	"machine",
	"releaser",
	"admin",
	"backup-restore",
	"relay",
];

export default function DeviceDetail() {
	const { id = "" } = useParams<{ id: string }>();
	const [tick, setTick] = useState(0);
	const detail = useApi(
		"devices",
		"get_device_by_id",
		{ device_id: id },
		[id, tick],
	);
	const refresh = () => setTick((t) => t + 1);
	usePageTitle(
		detail.status === "ok"
			? `Device ${deviceDisplayName(detail.data)}`
			: "Device",
	);

	if (detail.status === "loading" || detail.status === "idle") return <LinearProgress />;
	if (detail.status === "error")
		return <Alert severity="error">{detail.error.message}</Alert>;

	return <DeviceView device={detail.data} refresh={refresh} />;
}

function DeviceView({
	device,
	refresh,
}: {
	device: DeviceInfo;
	refresh: () => void;
}) {
	const name = deviceDisplayName(device);
	return (
		<Stack spacing={3}>
			<Typography variant="h4" component="h1">
				Device {name}
			</Typography>
			<DeviceInfoBox device={device} />
			<TailnetIdentitySection device={device} refresh={refresh} />
			<KeysBox device={device} refresh={refresh} />
			<RoleControls device={device} refresh={refresh} />
			<AssociatedServersSection deviceId={device.device.id} />
			<ConnectionHistory deviceId={device.device.id} />
		</Stack>
	);
}

function DeviceInfoBox({ device }: { device: DeviceInfo }) {
	const conn = device.latest_connection;
	const items: Array<{ label: string; value: React.ReactNode; mono?: boolean }> = [];
	if (conn) items.push({ label: "Address", value: conn.ip, mono: true });
	items.push({
		label: "First seen",
		value: <TimeAgo timestamp={device.device.created_at} />,
	});
	if (conn)
		items.push({
			label: "Last seen",
			value: <TimeAgo timestamp={conn.created_at} />,
		});
	items.push({
		label: "Last updated",
		value: <TimeAgo timestamp={device.device.updated_at} />,
	});
	if (conn?.user_agent)
		items.push({ label: "User-agent", value: conn.user_agent });

	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<Stack
				direction="row"
				spacing={4}
				useFlexGap
				sx={{ flexWrap: "wrap" }}
			>
				{items.map(({ label, value, mono }) => (
					<Stack key={label} spacing={0.25}>
						<Typography variant="caption" color="text.secondary">
							{label}
						</Typography>
						<Typography
							variant="body2"
							component="div"
							sx={mono ? { fontFamily: "monospace" } : undefined}
						>
							{value}
						</Typography>
					</Stack>
				))}
			</Stack>
		</Paper>
	);
}

function KeysBox({
	device,
	refresh,
}: {
	device: DeviceInfo;
	refresh: () => void;
}) {
	const [generateOpen, setGenerateOpen] = useState(false);
	const [addOpen, setAddOpen] = useState(false);
	const [confirmDisableAll, setConfirmDisableAll] = useState(false);
	const disableAll = useApiAction("devices", "disable_all_keys");
	const hasActiveKey = device.keys.some((k) => k.is_active);

	const onDisableAll = async () => {
		try {
			await disableAll.call({ device_id: device.device.id });
			setConfirmDisableAll(false);
			refresh();
		} catch {
			/* surfaced via disableAll.error */
		}
	};

	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<Stack
				direction="row"
				spacing={1}
				useFlexGap
				sx={{
					alignItems: "center",
					justifyContent: "space-between",
					flexWrap: "wrap",
					mb: 1,
				}}
			>
				<Typography variant="h6" component="h2">
					Public Keys ({device.keys.length})
				</Typography>
				<Stack direction="row" spacing={1} useFlexGap sx={{ flexWrap: "wrap" }}>
					<Button variant="outlined" onClick={() => setAddOpen(true)}>
						Add from public key
					</Button>
					<Button variant="contained" onClick={() => setGenerateOpen(true)}>
						Generate new key
					</Button>
					{hasActiveKey &&
						(confirmDisableAll ? (
							<>
								<Button
									variant="contained"
									color="error"
									onClick={onDisableAll}
									disabled={disableAll.pending}
								>
									{disableAll.pending ? "Disabling…" : "Confirm disable all"}
								</Button>
								<Button
									variant="outlined"
									onClick={() => setConfirmDisableAll(false)}
									disabled={disableAll.pending}
								>
									Cancel
								</Button>
							</>
						) : (
							<Button
								variant="outlined"
								color="error"
								onClick={() => setConfirmDisableAll(true)}
							>
								Disable all keys
							</Button>
						))}
				</Stack>
			</Stack>
			{disableAll.error && (
				<Alert severity="error" sx={{ mb: 1 }}>
					{disableAll.error.message}
				</Alert>
			)}
			<Stack spacing={2}>
				{device.keys.map((k) => (
					<KeyRow key={k.id} keyData={k} onSaved={refresh} />
				))}
			</Stack>
			<ProvisionCredentialDialog
				open={generateOpen}
				onClose={() => setGenerateOpen(false)}
				deviceId={device.device.id}
				role={device.device.role}
				onProvisioned={refresh}
			/>
			<AddPublicKeyDialog
				open={addOpen}
				onClose={() => setAddOpen(false)}
				deviceId={device.device.id}
				onAdded={refresh}
			/>
		</Paper>
	);
}

function KeyRow({
	keyData,
	onSaved,
}: {
	keyData: DeviceKeyInfo;
	onSaved: () => void;
}) {
	const [editing, setEditing] = useState(false);
	const [name, setName] = useState(keyData.name ?? "");
	const action = useApiAction("devices", "update_key_name");
	const disable = useApiAction("devices", "deactivate_key");
	const enable = useApiAction("devices", "reactivate_key");
	const toggling = disable.pending || enable.pending;

	const save = async () => {
		const trimmed = name.trim();
		try {
			await action.call({
				key_id: keyData.id,
				name: trimmed === "" ? null : trimmed,
			});
			setEditing(false);
			onSaved();
		} catch {
			/* surfaced via action.error */
		}
	};

	const onToggleActive = async () => {
		try {
			if (keyData.is_active) await disable.call({ key_id: keyData.id });
			else await enable.call({ key_id: keyData.id });
			onSaved();
		} catch {
			/* surfaced via disable/enable.error */
		}
	};

	return (
		<Box sx={{ opacity: keyData.is_active ? 1 : 0.55 }}>
			{editing ? (
				<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
					<TextField
						size="small"
						fullWidth
						value={name}
						onChange={(e) => setName(e.target.value)}
						placeholder="Key name"
						disabled={action.pending}
					/>
					<Button
						variant="contained"
						onClick={save}
						disabled={action.pending}
					>
						{action.pending ? "Saving…" : "Save"}
					</Button>
					<Button
						variant="outlined"
						color="error"
						onClick={() => {
							setName(keyData.name ?? "");
							setEditing(false);
						}}
						disabled={action.pending}
					>
						Cancel
					</Button>
				</Stack>
			) : (
				<Stack
					direction="row"
					spacing={1}
					sx={{ alignItems: "center", justifyContent: "space-between" }}
				>
					<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
						<Typography variant="subtitle1">
							{keyData.name ?? "Unnamed key"}
						</Typography>
						{!keyData.is_active && (
							<Chip size="small" label="disabled" color="default" />
						)}
					</Stack>
					<Stack direction="row" spacing={0.5} sx={{ alignItems: "center" }}>
						<Button
							size="small"
							variant="outlined"
							color={keyData.is_active ? "error" : "primary"}
							onClick={onToggleActive}
							disabled={toggling}
						>
							{keyData.is_active
								? disable.pending
									? "Disabling…"
									: "Disable"
								: enable.pending
									? "Enabling…"
									: "Enable"}
						</Button>
						<IconButton
							aria-label={`edit name for ${keyData.name ?? "key"}`}
							size="small"
							onClick={() => setEditing(true)}
						>
							<EditIcon fontSize="small" />
						</IconButton>
					</Stack>
				</Stack>
			)}
			{(action.error || disable.error || enable.error) && (
				<Alert severity="error" sx={{ mt: 1 }}>
					{(action.error ?? disable.error ?? enable.error)?.message}
				</Alert>
			)}
			<Box
				component="pre"
				sx={{
					mt: 1,
					p: 1.5,
					borderRadius: 1,
					bgcolor: "action.hover",
					fontFamily: "monospace",
					fontSize: "0.75em",
					overflow: "auto",
				}}
			>
				{keyData.pem_data}
			</Box>
		</Box>
	);
}

function RoleControls({
	device,
	refresh,
}: {
	device: DeviceInfo;
	refresh: () => void;
}) {
	const role = device.device.role;
	const updateRoleAction = useApiAction("devices", "update_role");
	const [selected, setSelected] = useState<DeviceRole>(role);

	const onSave = async () => {
		try {
			await updateRoleAction.call({
				device_id: device.device.id,
				role: selected,
			});
			refresh();
		} catch {
			/* surfaced via updateRoleAction.error */
		}
	};

	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
				<Typography variant="body2">Role:</Typography>
				<TextField
					select
					size="small"
					value={selected}
					onChange={(e) => setSelected(e.target.value as DeviceRole)}
					disabled={updateRoleAction.pending}
				>
					{TRUSTABLE_ROLES.map((r) => (
						<MenuItem key={r} value={r}>
							{r}
						</MenuItem>
					))}
				</TextField>
				<Button
					variant="contained"
					onClick={onSave}
					disabled={updateRoleAction.pending || selected === role}
				>
					{updateRoleAction.pending ? "Saving…" : "Save"}
				</Button>
			</Stack>
			{updateRoleAction.error && (
				<Alert severity="error" sx={{ mt: 1 }}>
					{updateRoleAction.error.message}
				</Alert>
			)}
		</Paper>
	);
}

function AssociatedServersSection({ deviceId }: { deviceId: string }) {
	const result = useApi(
		"devices",
		"get_servers_for_device",
		{ device_id: deviceId },
		[deviceId],
	);
	return (
		<ServersListSection
			title="Associated servers"
			result={result}
			emptyText="No servers are associated with this device."
			sort
		/>
	);
}

function ServersListSection({
	title,
	result,
	emptyText,
	sort = false,
}: {
	title: string;
	result: ApiState<ServerShortyInfo[]> & { reload: () => void };
	emptyText: string;
	sort?: boolean;
}) {
	const items =
		result.status === "ok" && sort
			? [...result.data].sort(compareServersByRankThenType)
			: result.status === "ok"
				? result.data
				: [];
	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<Stack
				direction="row"
				spacing={1}
				sx={{ alignItems: "center", justifyContent: "space-between", mb: 1 }}
			>
				<Typography variant="h6" component="h3">
					{title}
				</Typography>
				<IconButton
					aria-label="Refresh"
					size="small"
					onClick={result.reload}
				>
					<RefreshIcon fontSize="small" />
				</IconButton>
			</Stack>
			{result.status === "loading" || result.status === "idle" ? (
				<LinearProgress />
			) : result.status === "error" ? (
				<Alert severity="error">{result.error.message}</Alert>
			) : items.length === 0 ? (
				<Alert severity="info">{emptyText}</Alert>
			) : (
				<Stack spacing={1}>
					{items.map((s) => (
						<ServerShorty key={s.id} server={s} />
					))}
				</Stack>
			)}
		</Paper>
	);
}

const HISTORY_BATCH = 1000;

interface ConnectionGroup {
	ip: string;
	user_agent: string | null;
	count: number;
	earliest: string;
	latest: string;
}

function ConnectionHistory({ deviceId }: { deviceId: string }) {
	const [show, setShow] = useState(false);
	const count = useApi(
		"devices",
		"connection_count",
		{ device_id: deviceId },
		[deviceId],
	);
	const countLabel = count.status === "ok" ? ` (${count.data})` : "";

	return (
		<Box>
			<Button variant="outlined" onClick={() => setShow((s) => !s)}>
				{show ? "Hide connection history" : "Show connection history"}
				{countLabel}
			</Button>
			{show && <ConnectionHistoryDetail deviceId={deviceId} />}
		</Box>
	);
}

function ConnectionHistoryDetail({ deviceId }: { deviceId: string }) {
	const [connections, setConnections] = useState<DeviceConnectionData[]>([]);
	const [hasMore, setHasMore] = useState(false);
	const [loading, setLoading] = useState(false);
	const [error, setError] = useState<string | null>(null);

	const loadBatch = async (
		before: { created_at: string; id: string } | null,
	) => {
		setLoading(true);
		setError(null);
		try {
			const batch = await callApi(
				"devices",
				"connection_history",
				{ device_id: deviceId, limit: HISTORY_BATCH, before },
			);
			setHasMore(batch.length === HISTORY_BATCH);
			setConnections((prev) => {
				const map = new Map(prev.map((c) => [c.id, c]));
				for (const c of batch) map.set(c.id, c);
				return Array.from(map.values());
			});
		} catch (err) {
			setError(err instanceof Error ? err.message : String(err));
			setHasMore(false);
		} finally {
			setLoading(false);
		}
	};

	useEffect(() => {
		void loadBatch(null);
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [deviceId]);

	const sorted = [...connections].sort((a, b) =>
		b.created_at.localeCompare(a.created_at),
	);
	const groups = groupConnections(sorted);

	return (
		<Box sx={{ mt: 2 }}>
			{loading && connections.length === 0 && <LinearProgress />}
			{error && <Alert severity="error">{error}</Alert>}
			{!loading && !error && connections.length === 0 && (
				<Alert severity="info">No connection history found.</Alert>
			)}
			{groups.length > 0 && (
				<Paper variant="outlined" sx={{ p: 2 }}>
					<Stack spacing={1}>
						{groups.map((g, i) => (
							<ConnectionGroupRow key={i} group={g} />
						))}
					</Stack>
				</Paper>
			)}
			{hasMore && (
				<Box sx={{ mt: 1 }}>
					<Button
						variant="outlined"
						onClick={() => {
							const earliest = sorted[sorted.length - 1];
							if (earliest)
								void loadBatch({
									created_at: earliest.created_at,
									id: earliest.id,
								});
						}}
						disabled={loading}
					>
						{loading ? "Loading…" : `Load more (${HISTORY_BATCH})`}
					</Button>
				</Box>
			)}
		</Box>
	);
}

function ConnectionGroupRow({ group }: { group: ConnectionGroup }) {
	const span = humanDuration(group.earliest, group.latest);
	return (
		<Stack
			direction="row"
			spacing={2}
			sx={{ alignItems: "center", justifyContent: "space-between" }}
			useFlexGap
		>
			<Stack
				direction="row"
				spacing={2}
				sx={{ alignItems: "center", flexWrap: "wrap" }}
				useFlexGap
			>
				<Typography variant="body2" component="div">
					<TimeAgo timestamp={group.earliest} /> to{" "}
					<TimeAgo timestamp={group.latest} />
				</Typography>
				<Typography variant="body2" color="text.secondary">
					{span}
				</Typography>
				<Typography variant="body2" sx={{ fontFamily: "monospace" }}>
					{group.ip}
				</Typography>
				{group.count > 1 && (
					<Typography variant="body2" color="text.secondary">
						{group.count}×
					</Typography>
				)}
			</Stack>
			{group.user_agent && (
				<Typography variant="body2" color="text.secondary">
					{group.user_agent}
				</Typography>
			)}
		</Stack>
	);
}

function groupConnections(
	connections: DeviceConnectionData[],
): ConnectionGroup[] {
	if (connections.length === 0) return [];
	const groups: ConnectionGroup[] = [];
	let current: DeviceConnectionData[] = [connections[0]];
	for (const c of connections.slice(1)) {
		const last = current[current.length - 1];
		if (c.ip === last.ip && c.user_agent === last.user_agent) {
			current.push(c);
		} else {
			groups.push(toGroup(current));
			current = [c];
		}
	}
	groups.push(toGroup(current));
	return groups;
}

function toGroup(conns: DeviceConnectionData[]): ConnectionGroup {
	const first = conns[0];
	const last = conns[conns.length - 1];
	return {
		ip: first.ip,
		user_agent: first.user_agent,
		count: conns.length,
		earliest: last.created_at,
		latest: first.created_at,
	};
}

