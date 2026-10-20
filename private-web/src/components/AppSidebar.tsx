import {
	Box,
	Collapse,
	IconButton,
	List,
	ListItem,
	ListItemButton,
	ListItemText,
	Typography,
} from "@mui/material";
import ExpandLessIcon from "@mui/icons-material/ExpandLess";
import ExpandMoreIcon from "@mui/icons-material/ExpandMore";
import OpenInNewIcon from "@mui/icons-material/OpenInNew";
import { useEffect, useState } from "react";
import { Link, NavLink, useLocation } from "react-router-dom";
import { useIsAdmin } from "../hooks/useIsAdmin";
import { SafetyModeBand } from "./SafetyModeControl";

/** Whether `pathname` is `path` or somewhere beneath it. */
function under(path: string) {
	return (pathname: string) =>
		pathname === path || pathname.startsWith(`${path}/`);
}

interface NavPage {
	label: string;
	to: string;
	current: (pathname: string) => boolean;
	adminOnly?: boolean;
}

interface NavGroup {
	heading?: string;
	pages: NavPage[];
}

export interface NavSection {
	label: string;
	to: string;
	groups?: NavGroup[];
	/** Listed only where the SQL console is available. */
	needsSql?: boolean;
}

function page(label: string, to: string, adminOnly?: boolean): NavPage {
	return { label, to, current: under(to), adminOnly };
}

const SECTIONS: NavSection[] = [
	{ label: "Status", to: "/status" },
	{ label: "Incidents", to: "/incidents" },
	{
		label: "Fleet",
		to: "/fleet",
		// No ungrouped page: a machine is created in a group and an application
		// takes its machine's, so there is nothing left for that listing to list.
		// spec: FLT
		groups: [
			{
				pages: [
					{
						label: "Groups",
						to: "/fleet",
						// Everything the fleet holds is reached from its groups.
						current: (pathname) =>
							under("/fleet")(pathname) &&
							!under("/fleet/archived")(pathname) &&
							!under("/fleet/figures")(pathname),
					},
					page("Archived", "/fleet/archived"),
					page("Figures", "/fleet/figures"),
				],
			},
		],
	},
	{ label: "Versions", to: "/versions" },
	{ label: "Upgrades", to: "/upgrades" },
	{ label: "Maintenance", to: "/maintenance" },
	{
		label: "Devices",
		to: "/devices",
		groups: [
			{
				pages: [
					{
						label: "Search",
						to: "/devices",
						current: (pathname) => pathname === "/devices",
					},
					page("All devices", "/devices/all"),
				],
			},
		],
	},
	{ label: "Bestool", to: "/bestool" },
	{
		label: "Settings",
		to: "/settings",
		groups: [
			{
				heading: "Access",
				pages: [
					page("Admins", "/settings/admins"),
					page("MCP access", "/settings/mcp-tokens"),
					page("Certificate authority", "/settings/certificate-authority"),
					page("Identity provider", "/settings/identity-provider"),
					page("Audit log", "/settings/audit-log", true),
				],
			},
			{
				heading: "Monitoring",
				pages: [
					page("Healthchecks", "/settings/healthchecks"),
					page("Clusters", "/settings/clusters"),
				],
			},
			{
				heading: "Backups",
				pages: [
					page("Backup defaults", "/settings/backup-defaults"),
					page("Recovery vault", "/settings/recovery"),
					page("Restore consumers", "/settings/restore-consumers"),
				],
			},
		],
	},
	{ label: "SQL", to: "/sql", needsSql: true },
];

/** The sections offered, given whether the SQL console is available. */
export function navSections(sqlAvailable: boolean): NavSection[] {
	return SECTIONS.filter((section) => sqlAvailable || !section.needsSql);
}

/** The Canopy mark and name, linking home. */
export function Brand() {
	return (
		<Box
			component={NavLink}
			to="/"
			sx={{
				display: "flex",
				alignItems: "center",
				gap: 1,
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
	);
}

/** The number of open incidents, beside wherever Incidents is offered. */
export function IncidentCount({ count }: { count: number }) {
	return (
		<Box
			component="span"
			sx={{
				ml: "auto",
				minWidth: 20,
				height: 20,
				px: 0.75,
				borderRadius: "10px",
				bgcolor: "error.main",
				color: "error.contrastText",
				fontSize: "0.75rem",
				lineHeight: "20px",
				textAlign: "center",
			}}
		>
			{count}
		</Box>
	);
}

function SectionPages({
	section,
	onNavigate,
}: {
	section: NavSection;
	onNavigate?: () => void;
}) {
	const { pathname } = useLocation();
	const isAdmin = useIsAdmin() === true;

	return (
		<Box
			role="group"
			aria-label={`${section.label} pages`}
			sx={{ pl: 1.5, mt: 0.25, mb: 1 }}
		>
			{section.groups?.map((group, index) => {
				const pages = group.pages.filter((p) => isAdmin || !p.adminOnly);
				return (
					<Box key={group.heading ?? index}>
						{group.heading && (
							<Typography
								variant="overline"
								component="h3"
								color="text.secondary"
								sx={{
									display: "block",
									px: 2,
									pt: index === 0 ? 0.5 : 1.5,
									pb: 0.25,
									fontSize: "0.6875rem",
									fontWeight: 500,
									lineHeight: 1.66,
								}}
							>
								{group.heading}
							</Typography>
						)}
						<List dense disablePadding>
							{pages.map((p) => {
								const current = p.current(pathname);
								return (
									<ListItemButton
										key={p.to}
										component={Link}
										to={p.to}
										onClick={onNavigate}
										selected={current}
										aria-current={current ? "page" : undefined}
										sx={{
											borderRadius: 1,
											py: 0.5,
											px: 2,
											mt: "2px",
											"&.Mui-selected": { color: "primary.main" },
										}}
									>
										<ListItemText
											primary={p.label}
											slotProps={{
												primary: {
													sx: { fontWeight: current ? 500 : undefined },
												},
											}}
											sx={{ my: 0 }}
										/>
									</ListItemButton>
								);
							})}
						</List>
					</Box>
				);
			})}
		</Box>
	);
}

const sectionSx = {
	borderRadius: 1,
	py: 0.75,
	px: 2,
	color: "text.secondary",
	fontWeight: 500,
} as const;

/**
 * Every section of the surface, the current section's pages beneath it, and
 * the mode control across the foot.
 */
export function AppSidebar({
	sections,
	externalLinks,
	openIncidents,
	showModeBand,
	onNavigate,
}: {
	sections: NavSection[];
	externalLinks: Array<{ label: string; href: string }>;
	openIncidents: number;
	showModeBand: boolean;
	onNavigate?: () => void;
}) {
	const { pathname } = useLocation();
	const currentSection = sections.find((s) => under(s.to)(pathname))?.to;

	// Sections opened or closed by hand. The current section is open unless
	// closed; arriving in a section opens it again.
	const [toggled, setToggled] = useState<Record<string, boolean>>({});
	useEffect(() => {
		if (!currentSection) return;
		setToggled((was) => {
			if (!(currentSection in was)) return was;
			const { [currentSection]: _, ...rest } = was;
			return rest;
		});
	}, [currentSection]);

	return (
		<Box sx={{ display: "flex", flexDirection: "column", height: "100%" }}>
			<Box sx={{ px: 3, py: 1.5 }}>
				<Brand />
			</Box>
			<Box
				component="nav"
				aria-label="Main"
				sx={{ flex: 1, minHeight: 0, overflowY: "auto", p: 1 }}
			>
				<List disablePadding>
					{sections.map((section) => {
						const current = section.to === currentSection;
						const expanded = toggled[section.to] ?? current;
						return (
							<Box component="li" key={section.to} sx={{ listStyle: "none" }}>
								<ListItem
									component="div"
									disablePadding
									secondaryAction={
										section.groups && (
											<IconButton
												size="small"
												edge="end"
												aria-label={`${expanded ? "Collapse" : "Expand"} ${section.label}`}
												aria-expanded={expanded}
												onClick={() =>
													setToggled((was) => ({
														...was,
														[section.to]: !expanded,
													}))
												}
											>
												{expanded ? (
													<ExpandLessIcon fontSize="small" />
												) : (
													<ExpandMoreIcon fontSize="small" />
												)}
											</IconButton>
										)
									}
								>
									<ListItemButton
										component={Link}
										to={section.to}
										onClick={onNavigate}
										sx={{
											...sectionSx,
											...(current && { color: "secondary.main" }),
										}}
									>
										{section.label}
										{section.to === "/incidents" && openIncidents > 0 && (
											<IncidentCount count={openIncidents} />
										)}
									</ListItemButton>
								</ListItem>
								{section.groups && (
									<Collapse in={expanded} unmountOnExit>
										<SectionPages section={section} onNavigate={onNavigate} />
									</Collapse>
								)}
							</Box>
						);
					})}
					{externalLinks.map(({ label, href }, index) => (
						<Box
							component="li"
							key={label}
							sx={{ listStyle: "none", mt: index === 0 ? 1 : 0 }}
						>
							<ListItemButton
								component="a"
								href={href}
								target="_blank"
								rel="noopener"
								sx={{ ...sectionSx, gap: 0.5 }}
							>
								{label}
								<OpenInNewIcon sx={{ fontSize: "1em" }} />
							</ListItemButton>
						</Box>
					))}
				</List>
			</Box>
			{showModeBand && <SafetyModeBand />}
		</Box>
	);
}
