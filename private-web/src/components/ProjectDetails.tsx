import {
	Alert,
	Autocomplete,
	Avatar,
	Box,
	Button,
	Dialog,
	DialogActions,
	DialogContent,
	DialogTitle,
	Paper,
	Stack,
	Table,
	TableBody,
	TableCell,
	TableRow,
	TextField,
	Tooltip,
	Typography,
} from "@mui/material";
import EditIcon from "@mui/icons-material/Edit";
import { useState } from "react";
import { useApi, useApiAction } from "../api";
import { useAccessLevel } from "../hooks/useIsAdmin";
import type { NamedProjectDetails, PmCandidate } from "../types";
import { GradedAction } from "./GradedAction";

/// How a project detail not filled in reads.
/// spec: GRP#project-details
export function NoneText() {
	return (
		<Typography component="span" variant="inherit" color="text.secondary">
			None
		</Typography>
	);
}

function initials(name: string): string {
	const words = name.split(/[\s@._-]+/).filter(Boolean);
	return words
		.slice(0, 2)
		.map((word) => word.charAt(0).toUpperCase())
		.join("");
}

/// The PM in charge, by the display name Canopy knows for their login, and as
/// entered where it knows none.
/// spec: GRP#project-details
export function PmInCharge({ details }: { details: NamedProjectDetails }) {
	const login = details.pm_in_charge;
	if (!login) return <NoneText />;
	const name = details.pm_in_charge_name ?? login;
	const person = (
		<Box component="span" sx={{ display: "inline-flex", alignItems: "center", gap: 0.75 }}>
			<Avatar sx={{ width: 22, height: 22, fontSize: 11 }} aria-hidden>
				{initials(name)}
			</Avatar>
			{name}
		</Box>
	);
	return details.pm_in_charge_name ? <Tooltip title={login}>{person}</Tooltip> : person;
}

/// A group's project details, in a panel of their own on its page, with the
/// form editing them offered from the PM level up.
/// spec: GRP#project-details
export function ProjectPanel({
	groupId,
	groupName,
	details,
	onSaved,
}: {
	groupId: string;
	groupName: string;
	details: NamedProjectDetails;
	onSaved: () => void;
}) {
	const level = useAccessLevel();
	const [editing, setEditing] = useState(false);
	const canEdit = level === "pm" || level === "admin";
	const rows: Array<[string, React.ReactNode]> = [
		["Project", details.project || <NoneText />],
		["PM in charge", <PmInCharge details={details} />],
		["Contact on the ground", details.ground_contact || <NoneText />],
	];
	return (
		<Paper variant="outlined" sx={{ p: 2 }} component="section" aria-labelledby="project-heading">
			<Stack direction="row" sx={{ alignItems: "center", justifyContent: "space-between" }}>
				<Typography id="project-heading" variant="h6" component="h2">
					Project
				</Typography>
				{canEdit && (
					<GradedAction
						opens="fleet/groups/update_project_details"
						action={`Edit project details of group ${groupName}`}
					>
						<Button size="small" startIcon={<EditIcon />} onClick={() => setEditing(true)}>
							Edit project
						</Button>
					</GradedAction>
				)}
			</Stack>
			<Table size="small" sx={{ width: "auto" }}>
				<TableBody>
					{rows.map(([label, value]) => (
						<TableRow key={label} sx={{ "& td": { borderBottom: 0, px: 0 } }}>
							<TableCell component="th" scope="row" sx={{ color: "text.secondary", pr: 4 }}>
								{label}
							</TableCell>
							<TableCell sx={{ whiteSpace: "pre-wrap" }}>{value}</TableCell>
						</TableRow>
					))}
				</TableBody>
			</Table>
			{editing && (
				<EditProjectDetailsDialog
					groupId={groupId}
					groupName={groupName}
					details={details}
					onClose={() => setEditing(false)}
					onSaved={() => {
						setEditing(false);
						onSaved();
					}}
				/>
			)}
		</Paper>
	);
}

/// The form for a group's project details, holding only the three, so editing
/// them leads to nothing else a PM cannot change.
/// spec: GRP#project-details
function EditProjectDetailsDialog({
	groupId,
	groupName,
	details,
	onClose,
	onSaved,
}: {
	groupId: string;
	groupName: string;
	details: NamedProjectDetails;
	onClose: () => void;
	onSaved: () => void;
}) {
	const [project, setProject] = useState(details.project ?? "");
	const [pmInCharge, setPmInCharge] = useState(details.pm_in_charge ?? "");
	const [groundContact, setGroundContact] = useState(details.ground_contact ?? "");
	const save = useApiAction("fleet/groups", "update_project_details");

	const submit = async () => {
		try {
			await save.call({
				server_group_id: groupId,
				details: {
					project: project.trim() || null,
					pm_in_charge: pmInCharge.trim() || null,
					ground_contact: groundContact.trim() || null,
				},
			});
			onSaved();
		} catch {
			/* surfaced below */
		}
	};

	return (
		<Dialog open onClose={onClose} fullWidth maxWidth="sm">
			<DialogTitle>Edit project</DialogTitle>
			<DialogContent>
				<Stack spacing={2} sx={{ pt: 1 }}>
					<TextField
						label="Project"
						value={project}
						onChange={(e) => setProject(e.target.value)}
						autoFocus
					/>
					<PmInChargeField value={pmInCharge} onChange={setPmInCharge} />
					<TextField
						label="Contact on the ground"
						value={groundContact}
						onChange={(e) => setGroundContact(e.target.value)}
						multiline
						minRows={2}
					/>
					{save.error && <Alert severity="error">{save.error.message}</Alert>}
				</Stack>
			</DialogContent>
			<DialogActions>
				<Button onClick={onClose}>Cancel</Button>
				<GradedAction
					calls="fleet/groups/update_project_details"
					action={`Edit project details of group ${groupName}`}
				>
					<Button variant="contained" onClick={submit} disabled={save.pending}>
						Save
					</Button>
				</GradedAction>
			</DialogActions>
		</Dialog>
	);
}

/// The PM in charge, suggested from the tailnet's members and the people who
/// have used Canopy, taking any login or email address typed in as well.
/// spec: GRP#project-details
function PmInChargeField({
	value,
	onChange,
}: {
	value: string;
	onChange: (value: string) => void;
}) {
	const candidates = useApi("fleet/groups", "pm_candidates");
	const options: PmCandidate[] = candidates.status === "ok" ? candidates.data : [];
	return (
		<Autocomplete
			freeSolo
			options={options}
			inputValue={value}
			onInputChange={(_, next, reason) => {
				if (reason !== "reset") onChange(next);
			}}
			onChange={(_, picked) => {
				if (picked && typeof picked !== "string") onChange(picked.login);
			}}
			getOptionLabel={(option) => (typeof option === "string" ? option : option.login)}
			filterOptions={(all, { inputValue }) => {
				const term = inputValue.trim().toLowerCase();
				if (!term) return all;
				return all.filter(
					(option) =>
						option.name.toLowerCase().includes(term) ||
						option.login.toLowerCase().includes(term),
				);
			}}
			renderOption={({ key, ...props }, option) => (
				<Box component="li" key={key} {...props} sx={{ gap: 1 }}>
					<Avatar
						src={option.profile_pic ?? undefined}
						sx={{ width: 24, height: 24, fontSize: 11 }}
						aria-hidden
					>
						{initials(option.name)}
					</Avatar>
					{option.name}
					<Typography component="span" variant="body2" color="text.secondary">
						{option.login}
					</Typography>
				</Box>
			)}
			renderInput={(params) => (
				<TextField
					{...params}
					label="PM in charge"
					helperText="A tailnet login or an email address."
				/>
			)}
		/>
	);
}
