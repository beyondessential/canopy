//! A machine's backup capabilities: what the box advertises it can back up,
//! with an admin-only enable toggle, and the latest snapshot for each.
//!
//! A backup captures a box's data, so this belongs to the machine — a box
//! shared by two workloads backs up once, and showing it per workload would
//! say it twice.
// spec: BAK

import {
	Alert,
	Box,
	Button,
	Collapse,
	Divider,
	LinearProgress,
	Link as MuiLink,
	Paper,
	Stack,
	Switch,
	Typography,
} from "@mui/material";
import ExpandMoreIcon from "@mui/icons-material/ExpandMore";
import RestoreDataIcon from "@mui/icons-material/SettingsBackupRestore";
import { useEffect, useState } from "react";
import { Link as RouterLink } from "react-router-dom";
import { useApi, useApiAction } from "../api";
import { BackupLiveProgress } from "./BackupLiveProgress";
import { BackupProcessingChip } from "./BackupProcessingChip";
import { GradedAction } from "./GradedAction";
import { LatestSnapshot } from "./SnapshotId";
import TimeAgo from "./TimeAgo";
import { useReloadInterval } from "../hooks/useReloadInterval";
import type { MachineBackupCapabilityView } from "../types";

/// The all-zero group id, used to query backup config for an ungrouped box: it
/// always resolves to "no config" rather than erroring on a missing group.
const NIL_UUID = "00000000-0000-0000-0000-000000000000";

/// Per-(machine, type) backup capabilities with an admin-only enable toggle.
/// Reads `backups.capabilities`; the switch calls `backups.set_capability` and
/// refetches. Capabilities are advertised by bestool, so a box with none yet
/// renders an explicit empty state rather than disappearing.
export default function BackupCapabilitiesSection({
	machineId,
	groupId,
	isAdmin,
}: {
	machineId: string;
	groupId: string | null;
	isAdmin: boolean;
}) {
	// Poll faster while a backup of any type is in flight, so its figures advance
	// and the "backing up…" chip appears and clears on its own; back off when
	// nothing is running. Without this the section was frozen at page load.
	const [inFlight, setInFlight] = useState(false);
	const backupTick = useReloadInterval(
		inFlight ? 5_000 : 30_000,
		"canopy-data-changed",
	);
	const caps = useApi(
		"backups",
		"capabilities",
		{ machine_id: machineId },
		[machineId, backupTick],
	);
	const anyInFlight =
		caps.status === "ok" && caps.data.some((c) => c.processing_since != null);
	useEffect(() => {
		setInFlight(anyInFlight);
	}, [anyInFlight]);
	// Whether the group has an *active* (ready) backup config. An ungrouped box
	// queries the nil group, which always returns no config. While this is loading
	// we optimistically treat the section as active to avoid a grey→normal flash.
	const config = useApi(
		"backups",
		"get",
		{ server_group_id: groupId ?? NIL_UUID },
		[groupId],
	);
	// The machine's restore window: while open, an operator can run an ad-hoc
	// `bestool canopy restore` on the box. The window is the box's, so it shows
	// here as well as on the group backups page.
	// spec: BKO#allowing-a-restore
	const restore = useApi(
		"backups",
		"restore_window",
		{ machine_id: machineId },
		[machineId],
	);
	const allowRestore = useApiAction("backups", "allow_restore");
	const disallowRestore = useApiAction("backups", "disallow_restore");
	const restoreUntil =
		restore.status === "ok" ? restore.data.allowed_until : null;
	const onAllowRestore = async () => {
		try {
			await allowRestore.call({ machine_id: machineId });
			restore.reload();
		} catch {
			/* surfaced via allowRestore.error */
		}
	};
	const onDisallowRestore = async () => {
		try {
			await disallowRestore.call({ machine_id: machineId });
			restore.reload();
		} catch {
			/* surfaced via disallowRestore.error */
		}
	};

	const inactive =
		config.status === "ok" &&
		!(config.data != null && config.data.status === "ready");
	const inactiveMessage = !groupId
		? "This machine isn't in a group, so backups can't be configured for it."
		: config.status === "ok" && config.data == null
			? "Backups aren't set up for this group yet, so these settings have no effect."
			: "Backups for this group are still being set up, so these settings have no effect yet.";

	const [showInactive, setShowInactive] = useState(false);

	const rows = (
		<Stack divider={<Divider />}>
			{caps.status === "ok" &&
				caps.data.map((cap) => (
					<BackupCapabilityRow
						key={cap.type}
						machineId={machineId}
						cap={cap}
						isAdmin={isAdmin}
						onChanged={caps.reload}
					/>
				))}
		</Stack>
	);

	return (
		<Paper id="backups" variant="outlined" sx={{ p: 2 }}>
			<Stack
				direction="row"
				spacing={2}
				sx={{ alignItems: "baseline", justifyContent: "space-between" }}
			>
				<Typography variant="h6" component="h2" gutterBottom>
					Backups
				</Typography>
				{groupId && (
					<MuiLink
						component={RouterLink}
						to={`/fleet/groups/${groupId}/backups`}
						variant="body2"
						underline="hover"
					>
						Group backups ›
					</MuiLink>
				)}
			</Stack>

			{groupId && isAdmin && (
				<Box sx={{ mb: 1 }}>
					{restoreUntil ? (
						<Alert
							severity="warning"
							icon={<RestoreDataIcon fontSize="inherit" />}
							action={
								<GradedAction calls="backups/disallow_restore">
									<Button
										size="small"
										onClick={onDisallowRestore}
										disabled={allowRestore.pending || disallowRestore.pending}
									>
										Disable
									</Button>
								</GradedAction>
							}
						>
							Restores are allowed for this machine until{" "}
							<TimeAgo timestamp={restoreUntil} />. While open, the box can
							restore backups on demand.
						</Alert>
					) : (
						<GradedAction calls="backups/allow_restore">
							<Button
								size="small"
								variant="outlined"
								startIcon={<RestoreDataIcon />}
								onClick={onAllowRestore}
								disabled={allowRestore.pending || disallowRestore.pending}
							>
								Allow restores (24h)
							</Button>
						</GradedAction>
					)}
					{(allowRestore.error || disallowRestore.error) && (
						<Alert severity="error" sx={{ mt: 1 }}>
							{(allowRestore.error || disallowRestore.error)!.message}
						</Alert>
					)}
				</Box>
			)}

			{inactive && (
				<Alert
					severity="info"
					sx={{ mb: 1 }}
					action={
						groupId && isAdmin ? (
							<Button
								component={RouterLink}
								to={`/fleet/groups/${groupId}/backups`}
								color="inherit"
								size="small"
							>
								Set up
							</Button>
						) : undefined
					}
				>
					{inactiveMessage}
				</Alert>
			)}

			{caps.status === "loading" || caps.status === "idle" ? (
				<LinearProgress />
			) : caps.status === "error" ? (
				<Alert severity="error">{caps.error.message}</Alert>
			) : caps.data.length === 0 ? (
				<Typography variant="body2" color="text.secondary">
					No backup types registered for this machine.
				</Typography>
			) : inactive ? (
				// Collapsed + greyed: the toggles still work (they record intent for
				// when backups are set up), but it's clear they're dormant right now.
				<>
					<Button
						size="small"
						onClick={() => setShowInactive((s) => !s)}
						endIcon={
							<ExpandMoreIcon
								sx={{
									transform: showInactive ? "rotate(180deg)" : "none",
									transition: "transform 150ms",
								}}
							/>
						}
					>
						{showInactive
							? "Hide backup types"
							: `Show backup types (${caps.data.length})`}
					</Button>
					<Collapse in={showInactive}>
						<Box sx={{ opacity: 0.6, mt: 1 }}>{rows}</Box>
					</Collapse>
				</>
			) : (
				rows
			)}
		</Paper>
	);
}


function BackupCapabilityRow({
	machineId,
	cap,
	isAdmin,
	onChanged,
}: {
	machineId: string;
	cap: MachineBackupCapabilityView;
	isAdmin: boolean;
	onChanged: () => void;
}) {
	const setCapability = useApiAction("backups", "set_capability");
	const onToggle = async (enabled: boolean) => {
		try {
			await setCapability.call({
				machine_id: machineId,
				type: cap.type,
				enabled,
			});
			onChanged();
		} catch {
			/* surfaced via setCapability.error */
		}
	};
	return (
		<Stack
			direction="row"
			spacing={2}
			sx={{ alignItems: "center", justifyContent: "space-between", py: 0.5 }}
		>
			<Stack spacing={0.25} sx={{ minWidth: 0 }}>
				<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
					<Typography variant="body2" sx={{ fontFamily: "monospace" }}>
						{cap.type}
					</Typography>
					<BackupProcessingChip since={cap.processing_since} />
				</Stack>
				<BackupLiveProgress progress={cap.progress} />
				<LatestSnapshot
					id={cap.latest_snapshot_id}
					at={cap.latest_snapshot_at}
					bytes={cap.latest_snapshot_bytes}
				/>
			</Stack>
			<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
				{setCapability.error && (
					<Typography variant="caption" color="error">
						{setCapability.error.message}
					</Typography>
				)}
				<GradedAction calls="backups/set_capability">
					<Switch
						checked={cap.enabled}
						disabled={!isAdmin || setCapability.pending}
						onChange={(e) => onToggle(e.target.checked)}
						slotProps={{
							input: { "aria-label": `Enable ${cap.type} backups` },
						}}
					/>
				</GradedAction>
			</Stack>
		</Stack>
	);
}
