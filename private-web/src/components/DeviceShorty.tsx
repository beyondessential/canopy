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

export function deviceDisplayName(info: DeviceInfo): string {
	const namedKey = info.keys.findLast(
		(k) => k.name && k.name !== "Initial Key",
	);
	if (namedKey?.name) return namedKey.name;
	if (info.latest_connection) return info.latest_connection.ip;
	return info.device.id;
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
