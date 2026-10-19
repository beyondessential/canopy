import {
	AppBar,
	Box,
	Container,
	Drawer,
	IconButton,
	Toolbar,
	useMediaQuery,
} from "@mui/material";
import { useTheme } from "@mui/material/styles";
import MenuIcon from "@mui/icons-material/Menu";
import { useState } from "react";
import { NavLink, Navigate, Route, Routes, useParams } from "react-router-dom";
import { useApi } from "./api";
import AdminProbeBanner from "./components/AdminProbeBanner";
import {
	AppSidebar,
	Brand,
	IncidentCount,
	navSections,
} from "./components/AppSidebar";
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
import AuditLog from "./routes/AuditLog";
import McpTokens from "./routes/McpTokens";
import KubernetesClusters from "./routes/KubernetesClusters";
import RestoreConsumers from "./routes/RestoreConsumers";
import SelfAlerts from "./routes/SelfAlerts";
import SelfAlertsBanner from "./components/SelfAlertsBanner";
import Bestool from "./routes/Bestool";
import BestoolSnippetDetail from "./routes/BestoolSnippetDetail";
import BestoolSnippets from "./routes/BestoolSnippets";
import DeviceDetail from "./routes/DeviceDetail";
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
import ClusterDetail from "./routes/ClusterDetail";
import ClusterEdit from "./routes/ClusterEdit";
import MachineDetail from "./routes/MachineDetail";
import MachineEdit from "./routes/MachineEdit";
import ServerDetail from "./routes/ServerDetail";
import ServerEdit from "./routes/ServerEdit";
import ArchivedList from "./routes/ArchivedList";
import FleetFigures from "./routes/FleetFigures";
import Servers from "./routes/Servers";
import Sql from "./routes/Sql";
import VersionDetail from "./routes/VersionDetail";
import Upgrades from "./routes/Upgrades";
import Versions from "./routes/Versions";

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

const SIDEBAR_WIDTH = 240;

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
	const sections = navSections(
		sqlAvailable.status === "ok" && !!sqlAvailable.data,
	);
	const theme = useTheme();
	const wide = useMediaQuery(theme.breakpoints.up("md"), { noSsr: true });
	const [drawerOpen, setDrawerOpen] = useState(false);
	// The app bar's own grey, so the sidebar reads as the same chrome.
	const chrome =
		theme.palette.mode === "light"
			? theme.palette.grey[100]
			: theme.palette.grey[900];

	const externalLinks: Array<{ label: string; href: string }> = [
		...(publicUrl.status === "ok" && publicUrl.data
			? [{ label: "Public", href: publicUrl.data }]
			: []),
		...(serverVersionsUrl.status === "ok" && serverVersionsUrl.data
			? [{ label: "Server Versions", href: serverVersionsUrl.data }]
			: []),
	];
	const sidebar = {
		sections,
		externalLinks,
		openIncidents: openIncidentsCount,
	};

	return (
		<AdminProvider>
		<SafetyModeProvider>
		<ApplicationTypesProvider>
		<Box sx={{ display: "flex" }}>
			{wide ? (
				<Drawer
					variant="permanent"
					sx={{
						width: SIDEBAR_WIDTH,
						flexShrink: 0,
						"& .MuiDrawer-paper": {
							width: SIDEBAR_WIDTH,
							boxSizing: "border-box",
							backgroundColor: chrome,
						},
					}}
				>
					<AppSidebar
						{...sidebar}
						showModeBand
					/>
				</Drawer>
			) : (
				<Drawer
					variant="temporary"
					open={drawerOpen}
					onClose={() => setDrawerOpen(false)}
					sx={{
						"& .MuiDrawer-paper": {
							width: 256,
							boxSizing: "border-box",
							backgroundColor: chrome,
						},
					}}
				>
					<AppSidebar
						{...sidebar}
						showModeBand={false}
						onNavigate={() => setDrawerOpen(false)}
					/>
				</Drawer>
			)}
			<Box sx={{ flex: 1, minWidth: 0 }}>
			{!wide && (
				<AppBar position="static" color="default" elevation={1}>
					<Toolbar
						variant="dense"
						sx={{ gap: 1, pl: { xs: 1 }, pr: { xs: 0 } }}
					>
						<IconButton
							aria-label="Open navigation"
							onClick={() => setDrawerOpen(true)}
						>
							<MenuIcon />
						</IconButton>
						<Brand />
						<Box sx={{ flex: 1 }} />
						{/* Kept out of the drawer, so an open incident is never
						    hidden behind the menu button. */}
						<Box
							component={NavLink}
							to="/incidents"
							sx={{
								display: "flex",
								alignItems: "center",
								gap: 1,
								mr: 2,
								color: "text.secondary",
								fontWeight: 500,
								textDecoration: "none",
								"&.active": { color: "secondary.main" },
							}}
						>
							Incidents
							{openIncidentsCount > 0 && (
								<IncidentCount count={openIncidentsCount} />
							)}
						</Box>
						<SafetyModeControl />
					</Toolbar>
				</AppBar>
			)}
			<AdminProbeBanner />
			<SelfAlertsBanner reloadTick={reloadTick} />
			<Container component="main" maxWidth="lg" sx={{ py: 3 }}>
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
					<Route
						path="/fleet/clusters/:id"
						element={<ClusterDetail />}
					/>
					<Route
						path="/fleet/clusters/:id/edit"
						element={<ClusterEdit />}
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
					<Route path="/settings">
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
						<Route path="audit-log" element={<AuditLog />} />
						<Route
							path="certificate-authority"
							element={<CertificateAuthority />}
						/>
					</Route>
					<Route path="/devices">
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
		</Box>
		</ApplicationTypesProvider>
		</SafetyModeProvider>
		</AdminProvider>
	);
}
