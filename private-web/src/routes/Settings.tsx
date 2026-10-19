import { Box, Tab, Tabs } from "@mui/material";
import { Outlet, useLocation, useNavigate } from "react-router-dom";
import { useIsAdmin } from "../hooks/useIsAdmin";

/// Settings groups the infrequently-used admin pages (operator admins, recovery
/// vault) under one top-level nav item so they don't each hold space in the bar.
const TABS: Array<{
	value: string;
	label: string;
	to: string;
	adminOnly?: boolean;
}> = [
	{ value: "admins", label: "Admins", to: "/settings/admins" },
	{
		value: "healthchecks",
		label: "Healthchecks",
		to: "/settings/healthchecks",
	},
	{
		value: "backup-defaults",
		label: "Backup defaults",
		to: "/settings/backup-defaults",
	},
	{ value: "recovery", label: "Recovery vault", to: "/settings/recovery" },
	{
		value: "restore-consumers",
		label: "Restore consumers",
		to: "/settings/restore-consumers",
	},
	{ value: "clusters", label: "Clusters", to: "/settings/clusters" },
	{ value: "mcp-tokens", label: "MCP access", to: "/settings/mcp-tokens" },
	{
		value: "certificate-authority",
		label: "Certificate authority",
		to: "/settings/certificate-authority",
	},
	{
		value: "audit-log",
		label: "Audit log",
		to: "/settings/audit-log",
		adminOnly: true,
	},
];

function valueFromPath(pathname: string): string {
	if (pathname.startsWith("/settings/recovery")) return "recovery";
	if (pathname.startsWith("/settings/backup-defaults")) return "backup-defaults";
	if (pathname.startsWith("/settings/healthchecks")) return "healthchecks";
	if (pathname.startsWith("/settings/restore-consumers"))
		return "restore-consumers";
	if (pathname.startsWith("/settings/clusters")) return "clusters";
	if (pathname.startsWith("/settings/mcp-tokens")) return "mcp-tokens";
	if (pathname.startsWith("/settings/certificate-authority"))
		return "certificate-authority";
	if (pathname.startsWith("/settings/audit-log")) return "audit-log";
	if (pathname.startsWith("/settings/admins")) return "admins";
	return "";
}

export default function Settings() {
	const location = useLocation();
	const navigate = useNavigate();
	const isAdmin = useIsAdmin() === true;
	const tabs = TABS.filter((t) => isAdmin || !t.adminOnly);
	const value = valueFromPath(location.pathname);

	return (
		<Box>
			<Tabs
				value={tabs.some((t) => t.value === value) ? value : false}
				variant="scrollable"
				scrollButtons="auto"
				allowScrollButtonsMobile
				onChange={(_, v) => {
					const target = tabs.find((t) => t.value === v);
					if (target) navigate(target.to);
				}}
				sx={{ mb: 2 }}
			>
				{tabs.map((t) => (
					<Tab key={t.value} value={t.value} label={t.label} />
				))}
			</Tabs>
			<Outlet />
		</Box>
	);
}
