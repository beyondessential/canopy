import { Box, Chip, Link as MuiLink, Stack } from "@mui/material";
import { Link as RouterLink } from "react-router-dom";
import type { DeviceInfo, DeviceRole } from "../types";

const ROLE_COLORS: Record<DeviceRole, "primary" | "warning" | "info"> = {
	machine: "primary",
	releaser: "warning",
	admin: "info",
	"backup-restore": "primary",
	relay: "info",
};

/// What the device is known by, or null when nothing names it yet.
export function deviceName(info: DeviceInfo): string | null {
	const namedKey = info.keys.findLast(
		(k) => k.name && k.name !== "Initial Key",
	);
	return (
		namedKey?.name ||
		info.device.tailscale_node_name ||
		info.tailnet_live?.display_name ||
		info.latest_connection?.ip ||
		null
	);
}

export function deviceDisplayName(info: DeviceInfo): string {
	return deviceName(info) ?? "Unnamed device";
}

export default function DeviceShorty({ device }: { device: DeviceInfo }) {
	const name = deviceDisplayName(device);
	const hasTailnet = device.device.tailscale_node_id != null;
	const hasMtls = device.keys.length > 0;
	return (
		<MuiLink
			component={RouterLink}
			to={`/devices/${device.device.id}`}
			underline="none"
			color="inherit"
			sx={{ display: "block" }}
		>
			<Stack
				direction="row"
				spacing={2}
				sx={(theme) => ({
					p: 1.5,
					border: 1,
					borderColor: "divider",
					borderRadius: 1,
					alignItems: "center",
					transition: theme.transitions.create("background-color"),
					"&:hover": { bgcolor: "action.hover" },
				})}
			>
				<Box sx={{ fontWeight: 500 }}>{name}</Box>
				<Stack
					direction="row"
					spacing={0.5}
					sx={{ ml: "auto", alignItems: "center" }}
				>
					{hasTailnet && (
						<Chip
							size="small"
							variant="outlined"
							color="success"
							label="tailnet"
						/>
					)}
					{hasMtls && (
						<Chip
							size="small"
							variant="outlined"
							label="mTLS"
						/>
					)}
					<Chip
						size="small"
						variant="outlined"
						color={ROLE_COLORS[device.device.role]}
						label={device.device.role}
						sx={{ textTransform: "capitalize" }}
					/>
				</Stack>
			</Stack>
		</MuiLink>
	);
}
