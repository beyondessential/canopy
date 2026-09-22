import {
	AppBar,
	Badge,
	Box,
	Container,
	Link as MuiLink,
	Toolbar,
	Typography,
} from "@mui/material";
import OpenInNewIcon from "@mui/icons-material/OpenInNew";
import { NavLink, Navigate, Route, Routes, useParams } from "react-router-dom";
import { useApi } from "./api";
import AdminProbeBanner from "./components/AdminProbeBanner";
import { AdminProvider } from "./hooks/useIsAdmin";
import { SafetyModeProvider } from "./hooks/useSafetyMode";
import { SafetyModeControl } from "./components/SafetyModeControl";
import { ApplicationTypesProvider } from "./hooks/useApplicationTypes";
import { useReloadInterval } from "./hooks/useReloadInterval";
import Admins from "./routes/Admins";
import BackupConfig from "./routes/BackupConfig";
import BackupDefaults from "./routes/BackupDefaults";
import RecoveryVault from "./routes/RecoveryVault";
import BackupPanel from "./routes/BackupPanel";
import CertificateAuthority from "./routes/CertificateAuthority";
import McpTokens from "./routes/McpTokens";
import KubernetesClusters from "./routes/KubernetesClusters";
import RestoreConsumers from "./routes/RestoreConsumers";
import SelfAlerts from "./routes/SelfAlerts";
import SelfAlertsBanner from "./components/SelfAlertsBanner";
import Bestool from "./routes/Bestool";
import BestoolSnippetDetail from "./routes/BestoolSnippetDetail";
import BestoolSnippets from "./routes/BestoolSnippets";
import DeviceDetail from "./routes/DeviceDetail";
import Devices from "./routes/Devices";
import DevicesList from "./routes/DevicesList";
import DevicesSearch from "./routes/DevicesSearch";
import GroupDetail from "./routes/GroupDetail";
import GroupEdit from "./routes/GroupEdit";
import GroupsList from "./routes/GroupsList";
import CheckDetail from "./routes/CheckDetail";
import CheckRedirect from "./routes/CheckRedirect";
import HealthcheckSettings from "./routes/HealthcheckSettings";
import Healthchecks from "./routes/Healthchecks";
import SourcesSettings from "./routes/SourcesSettings";
import IncidentDetail from "./routes/IncidentDetail";
import Incidents from "./routes/Incidents";
import Maintenance from "./routes/Maintenance";
import Status from "./routes/Status";
import MachineCreate from "./routes/MachineCreate";
import MachineDetail from "./routes/MachineDetail";
import MachineEdit from "./routes/MachineEdit";
import ServerDetail from "./routes/ServerDetail";
import ServerEdit from "./routes/ServerEdit";
import ArchivedList from "./routes/ArchivedList";
import FleetFigures from "./routes/FleetFigures";
import Servers from "./routes/Servers";
import Settings from "./routes/Settings";
import Sql from "./routes/Sql";
import VersionDetail from "./routes/VersionDetail";
import Upgrades from "./routes/Upgrades";
import Versions from "./routes/Versions";

interface NavItem {
	label: string;
	to: string;
}

/// Send an address from before the fleet was namespaced to where it lives now.
///
/// `servers` named the box and the workload at once, and the fleet's own pages
/// sat beside its records — so `/servers/figures` and `/servers/{id}` were told
/// apart only by route ranking. Everything the fleet holds is under `/fleet`
/// now: its listings, its applications, its machines.
///
/// Replaces rather than pushes, so the back button goes where the operator came
/// from instead of bouncing off the redirect.
function Moved({ to }: { to: (id: string) => string }) {
	const { id = "" } = useParams<{ id: string }>();
	return <Navigate to={to(id)} replace />;
}

const BASE_NAV: NavItem[] = [
	{ label: "Status", to: "/status" },
	{ label: "Incidents", to: "/incidents" },
	{ label: "Fleet", to: "/fleet" },
	{ label: "Versions", to: "/versions" },
	{ label: "Upgrades", to: "/upgrades" },
	{ label: "Maintenance", to: "/maintenance" },
	{ label: "Devices", to: "/devices" },
	{ label: "Bestool", to: "/bestool" },
	{ label: "Settings", to: "/settings" },
];

export default function App() {
	const sqlAvailable = useApi("sql", "is_sql_available");
	const publicUrl = useApi("commons", "public_url");
	const serverVersionsUrl = useApi("commons", "server_versions_url");
	// Polled at a slow cadence; mutations also fire `canopy-data-changed`
	// (via useApiAction), so the badge updates immediately after the user
	// acks / resolves / opens an incident on any page.
	const reloadTick = useReloadInterval(60_000, "canopy-data-changed");
	const openIncidents = useApi(
		"incidents",
		"list_active",
		{},
		[reloadTick],
	);
	const openIncidentsCount =
		openIncidents.status === "ok" ? openIncidents.data.length : 0;
	const navItems: NavItem[] = [
		...BASE_NAV,
		...(sqlAvailable.status === "ok" && sqlAvailable.data
			? [{ label: "SQL", to: "/sql" }]
			: []),
	];

	const externalLinks: Array<{ label: string; href: string }> = [
		...(publicUrl.status === "ok" && publicUrl.data
			? [{ label: "Public", href: publicUrl.data }]
			: []),
		...(serverVersionsUrl.status === "ok" && serverVersionsUrl.data
			? [{ label: "Server Versions", href: serverVersionsUrl.data }]
			: []),
	];

	return (
		<AdminProvider>
		<SafetyModeProvider>
		<ApplicationTypesProvider>
		<Box>
			<AppBar position="static" color="default" elevation={1}>
				<Toolbar variant="dense" sx={{ gap: 2 }}>
					<Box
						component={NavLink}
						to="/"
						sx={{
							display: "flex",
							alignItems: "center",
							gap: 1,
							mr: 2,
							color: "primary.main",
							textDecoration: "none",
						}}
					>
						<Box
							component="img"
							src="/favicon.svg"
							alt=""
							aria-hidden
							sx={{ height: 24, width: 24 }}
						/>
						<Typography variant="h6" component="span">
							Canopy
						</Typography>
					</Box>
					{navItems.map(({ label, to }) => {
						const showBadge = to === "/incidents" && openIncidentsCount > 0;
						const inner = (
							<Typography
								key={to}
								component={NavLink}
								to={to}
								sx={({ palette }) => ({
									textDecoration: "none",
									color: palette.text.secondary,
									fontWeight: 500,
									"&.active": { color: palette.secondary.main },
								})}
							>
								{label}
							</Typography>
						);
						if (showBadge) {
							return (
								<Badge
									key={to}
									badgeContent={openIncidentsCount}
									color="error"
									overlap="rectangular"
								>
									{inner}
								</Badge>
							);
						}
						return inner;
					})}
					<Box sx={{ flex: 1 }} />
					<SafetyModeControl />
					{externalLinks.map(({ label, href }) => (
						<Typography
							key={label}
							component={MuiLink}
							href={href}
							target="_blank"
							rel="noopener"
							sx={({ palette }) => ({
								textDecoration: "none",
								color: palette.text.secondary,
								fontWeight: 500,
								display: "inline-flex",
								alignItems: "center",
								gap: 0.5,
							})}
						>
							{label}
							<OpenInNewIcon sx={{ fontSize: "1em" }} />
						</Typography>
					))}
				</Toolbar>
			</AppBar>
			<AdminProbeBanner />
			<SelfAlertsBanner reloadTick={reloadTick} />
			<Container maxWidth="lg" sx={{ py: 3 }}>
				<Routes>
					<Route path="/" element={<Navigate to="/status" replace />} />
					<Route path="/status" element={<Status />} />
					<Route path="/alerts" element={<SelfAlerts />} />
					<Route path="/incidents" element={<Incidents />} />
					<Route path="/incidents/:id" element={<IncidentDetail />} />
					<Route
						path="/healthchecks/:source/:namespace/:check"
						element={<CheckDetail />}
					/>
					<Route
						path="/healthchecks/:source/:check"
						element={<CheckRedirect settings={false} />}
					/>
					<Route path="/upgrades" element={<Upgrades />} />
					<Route path="/maintenance" element={<Maintenance />} />
					<Route path="/versions" element={<Versions />} />
					<Route path="/versions/:version" element={<VersionDetail />} />
					<Route path="/fleet" element={<Servers />}>
						<Route index element={<GroupsList />} />
						<Route path="archived" element={<ArchivedList />} />
						<Route path="figures" element={<FleetFigures />} />
					</Route>
					<Route path="/fleet/groups/new" element={<GroupEdit />} />
					<Route path="/fleet/groups/:id" element={<GroupDetail />} />
					<Route
						path="/fleet/groups/:id/edit"
						element={<GroupEdit />}
					/>
					<Route
						path="/fleet/groups/:id/machines/new"
						element={<MachineCreate />}
					/>
					<Route
						path="/fleet/groups/:id/backups"
						element={<BackupPanel />}
					/>
					<Route
						path="/fleet/groups/:id/backups/config"
						element={<BackupConfig />}
					/>
					<Route
						path="/fleet/applications/:id"
						element={<ServerDetail />}
					/>
					<Route
						path="/fleet/applications/:id/edit"
						element={<ServerEdit />}
					/>
					<Route
						path="/fleet/machines/:id"
						element={<MachineDetail />}
					/>
					<Route
						path="/fleet/machines/:id/edit"
						element={<MachineEdit />}
					/>

					{/* Where the fleet used to live. A link into Canopy outlives
					    a rename — a bookmark, a Slack message, an incident
					    writeup — so every old address still lands. */}
					<Route
						path="/servers"
						element={<Navigate to="/fleet" replace />}
					/>
					<Route
						path="/servers/archived"
						element={<Navigate to="/fleet/archived" replace />}
					/>
					<Route
						path="/servers/figures"
						element={<Navigate to="/fleet/figures" replace />}
					/>
					<Route
						path="/servers/:id"
						element={<Moved to={(id) => `/fleet/applications/${id}`} />}
					/>
					<Route
						path="/servers/:id/edit"
						element={<Moved to={(id) => `/fleet/applications/${id}/edit`} />}
					/>
					<Route
						path="/applications/:id"
						element={<Moved to={(id) => `/fleet/applications/${id}`} />}
					/>
					<Route
						path="/applications/:id/edit"
						element={<Moved to={(id) => `/fleet/applications/${id}/edit`} />}
					/>
					<Route
						path="/machines/:id"
						element={<Moved to={(id) => `/fleet/machines/${id}`} />}
					/>
					<Route
						path="/machines/:id/edit"
						element={<Moved to={(id) => `/fleet/machines/${id}/edit`} />}
					/>
					<Route
						path="/groups/new"
						element={<Navigate to="/fleet/groups/new" replace />}
					/>
					<Route
						path="/groups/:id"
						element={<Moved to={(id) => `/fleet/groups/${id}`} />}
					/>
					<Route
						path="/groups/:id/edit"
						element={<Moved to={(id) => `/fleet/groups/${id}/edit`} />}
					/>
					<Route
						path="/groups/:id/machines/new"
						element={
							<Moved to={(id) => `/fleet/groups/${id}/machines/new`} />
						}
					/>
					<Route
						path="/groups/:id/backups"
						element={<Moved to={(id) => `/fleet/groups/${id}/backups`} />}
					/>
					<Route
						path="/groups/:id/backups/config"
						element={
							<Moved to={(id) => `/fleet/groups/${id}/backups/config`} />
						}
					/>
					<Route path="/settings" element={<Settings />}>
						<Route
							index
							element={<Navigate to="/settings/admins" replace />}
						/>
						<Route path="admins" element={<Admins />} />
						<Route path="backup-defaults" element={<BackupDefaults />} />
						<Route path="recovery" element={<RecoveryVault />} />
						<Route path="healthchecks" element={<Healthchecks />} />
						<Route
							path="healthchecks/sources"
							element={<SourcesSettings />}
						/>
						<Route
							path="healthchecks/:source/:namespace/:checkName"
							element={<HealthcheckSettings />}
						/>
						<Route
							path="healthchecks/:source/:check"
							element={<CheckRedirect settings />}
						/>
						<Route path="restore-consumers" element={<RestoreConsumers />} />
						<Route path="clusters" element={<KubernetesClusters />} />
						<Route path="mcp-tokens" element={<McpTokens />} />
						<Route
							path="certificate-authority"
							element={<CertificateAuthority />}
						/>
					</Route>
					<Route path="/devices" element={<Devices />}>
						<Route index element={<DevicesSearch />} />
						<Route path="all" element={<DevicesList />} />
					</Route>
					<Route path="/devices/:id" element={<DeviceDetail />} />
					<Route path="/bestool" element={<Bestool />}>
						<Route
							index
							element={<Navigate to="/bestool/snippets" replace />}
						/>
						<Route path="snippets" element={<BestoolSnippets />} />
						<Route
							path="snippets/:id"
							element={<BestoolSnippetDetail />}
						/>
					</Route>
					<Route path="/sql" element={<Sql />} />
				</Routes>
			</Container>
		</Box>
		</ApplicationTypesProvider>
		</SafetyModeProvider>
		</AdminProvider>
	);
}
