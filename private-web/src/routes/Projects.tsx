import {
	Alert,
	Box,
	InputAdornment,
	LinearProgress,
	Link as MuiLink,
	Paper,
	Stack,
	Table,
	TableBody,
	TableCell,
	TableHead,
	TableRow,
	TextField,
	Typography,
} from "@mui/material";
import SearchIcon from "@mui/icons-material/Search";
import { useState } from "react";
import { Link as RouterLink } from "react-router-dom";
import { useApi } from "../api";
import { NoneText, PmInCharge } from "../components/ProjectDetails";
import TargetName from "../components/TargetName";
import { usePageTitle } from "../hooks/usePageTitle";
import {
	applicationName,
	applicationTypeLabel,
	type ProjectGroup,
	type ProjectTamanu,
} from "../types";

/// Whether `group` is one of those a search for `term` narrows to: its name,
/// project, PM in charge by display name or login, or contact on the ground
/// contains the term, ignoring case.
/// spec: PRJ#searching
function matches(group: ProjectGroup, term: string): boolean {
	if (!term) return true;
	const needle = term.toLowerCase();
	const details = group.project_details;
	return [
		group.name,
		details.project,
		details.pm_in_charge,
		details.pm_in_charge_name,
		details.ground_contact,
	].some((field) => field?.toLowerCase().includes(needle));
}

/// The fleet's projects: every live group with a production environment, the
/// viewer's own first, each with its details and how to reach its Tamanu.
/// spec: PRJ
export default function Projects() {
	usePageTitle("Projects");
	const projects = useApi("fleet/groups", "projects", {}, []);
	const [search, setSearch] = useState("");

	if (projects.status === "loading" || projects.status === "idle") {
		return <LinearProgress />;
	}
	if (projects.status === "error") {
		return <Alert severity="error">{projects.error.message}</Alert>;
	}

	const term = search.trim();
	const shown = projects.data.filter((group) => matches(group, term));
	const yours = shown.filter((group) => group.yours);
	const others = shown.filter((group) => !group.yours);

	return (
		<Stack spacing={2}>
			<TextField
				size="small"
				placeholder="Search groups, projects, people"
				value={search}
				onChange={(e) => setSearch(e.target.value)}
				sx={{ maxWidth: 480 }}
				slotProps={{
					htmlInput: { "aria-label": "Search projects" },
					input: {
						startAdornment: (
							<InputAdornment position="start">
								<SearchIcon fontSize="small" />
							</InputAdornment>
						),
					},
				}}
			/>
			{projects.data.length === 0 ? (
				<Alert severity="info">No group has a production environment yet.</Alert>
			) : shown.length === 0 ? (
				<Alert severity="info">No project matches “{term}”.</Alert>
			) : (
				<>
					{yours.length > 0 && (
						<ProjectSection heading="Yours" groups={yours} highlighted />
					)}
					{others.length > 0 && (
						<ProjectSection
							heading={yours.length > 0 ? "Everyone else's" : null}
							groups={others}
						/>
					)}
				</>
			)}
		</Stack>
	);
}

function ProjectSection({
	heading,
	groups,
	highlighted = false,
}: {
	/// None where the section stands alone, having nothing to be told apart from.
	heading: string | null;
	groups: ProjectGroup[];
	highlighted?: boolean;
}) {
	return (
		<Box component="section" aria-label={heading ?? "Projects"}>
			{heading && (
				<Typography variant="overline" component="h2" color="text.secondary">
					{heading}
				</Typography>
			)}
			<Stack spacing={1}>
				{groups.map((group) => (
					<ProjectCard key={group.id} group={group} highlighted={highlighted} />
				))}
			</Stack>
		</Box>
	);
}

function ProjectCard({
	group,
	highlighted,
}: {
	group: ProjectGroup;
	highlighted: boolean;
}) {
	const details = group.project_details;
	return (
		<Paper
			variant="outlined"
			component="article"
			aria-label={group.name}
			sx={{
				p: 2,
				...(highlighted && { borderLeft: 3, borderLeftColor: "secondary.main" }),
			}}
		>
			<Stack direction="row" spacing={2} sx={{ alignItems: "baseline", flexWrap: "wrap" }}>
				<MuiLink
					component={RouterLink}
					to={`/fleet/groups/${group.id}`}
					underline="hover"
					color="text.primary"
					variant="subtitle1"
					sx={{ fontWeight: 500 }}
				>
					{group.name}
				</MuiLink>
				<Typography variant="body2">
					{details.project || <NoneText />}
				</Typography>
			</Stack>
			<Stack
				direction="row"
				sx={{ columnGap: 4, rowGap: 1, my: 1, flexWrap: "wrap" }}
			>
				<Labelled label="PM in charge">
					<PmInCharge details={details} />
				</Labelled>
				<Labelled label="Contact">{details.ground_contact || <NoneText />}</Labelled>
			</Stack>
			{group.tamanu.length === 0 ? (
				<Typography variant="body2" color="text.secondary">
					No Tamanu in production.
				</Typography>
			) : (
				<TamanuTable group={group} />
			)}
		</Paper>
	);
}

function Labelled({ label, children }: { label: string; children: React.ReactNode }) {
	return (
		<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
			<Typography variant="body2" color="text.secondary">
				{label}
			</Typography>
			<Typography variant="body2" component="span" sx={{ whiteSpace: "pre-wrap" }}>
				{children}
			</Typography>
		</Stack>
	);
}

/// The Tamanu in a group's production environment, central first, each named
/// as everywhere in the fleet, and the machine telling apart any sharing a
/// name.
/// spec: PRJ#what-each-group-shows
/// spec: FLT#naming
function TamanuTable({ group }: { group: ProjectGroup }) {
	const names = group.tamanu.map(applicationName);
	const shared = new Set(names.filter((name, i) => names.indexOf(name) !== i));
	return (
		<Table size="small" aria-label={`${group.name} Tamanu`}>
			<TableHead>
				<TableRow>
					<TableCell sx={{ width: "32%" }}>Tamanu</TableCell>
					<TableCell sx={{ width: "34%" }}>Public</TableCell>
					<TableCell>Tailnet</TableCell>
				</TableRow>
			</TableHead>
			<TableBody>
				{group.tamanu.map((app, i) => (
					<TableRow key={app.application_id}>
						<TableCell>
							<TamanuName app={app} toldApart={shared.has(names[i]!)} />
						</TableCell>
						<TableCell>
							<Address href={app.url} />
						</TableCell>
						<TableCell>
							<Address href={app.tailnet_name ? `https://${app.tailnet_name}` : null} />
						</TableCell>
					</TableRow>
				))}
			</TableBody>
		</Table>
	);
}

function TamanuName({ app, toldApart }: { app: ProjectTamanu; toldApart: boolean }) {
	return (
		<>
			<TargetName
				parts={[
					...(toldApart && app.machine_name
						? [
								{
									label: app.machine_name,
									to: app.machine_id ? `/fleet/machines/${app.machine_id}` : null,
								},
							]
						: []),
					{ label: applicationName(app), to: `/fleet/applications/${app.application_id}` },
				]}
			/>
			{app.name && (
				<Typography component="span" variant="body2" color="text.secondary" sx={{ ml: 1 }}>
					{applicationTypeLabel(app.type)}
				</Typography>
			)}
		</>
	);
}

/// A way to reach an application, opening in a new tab, or none where it isn't
/// known.
/// spec: PRJ#what-each-group-shows
function Address({ href }: { href: string | null }) {
	if (!href) return <NoneText />;
	return (
		<MuiLink href={href} target="_blank" rel="noopener noreferrer" underline="hover">
			{href.replace(/^https?:\/\//, "").replace(/\/$/, "")}
		</MuiLink>
	);
}
