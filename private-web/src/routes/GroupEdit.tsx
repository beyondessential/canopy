import {
	Alert,
	Button,
	LinearProgress,
	Paper,
	Stack,
	TextField,
	Typography,
} from "@mui/material";
import ArchiveIcon from "@mui/icons-material/ArchiveOutlined";
import { useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import TagsEditor from "../components/TagsEditor";
import { useApi, useApiAction } from "../api";
import { GradedAction } from "../components/GradedAction";
import { usePageTitle } from "../hooks/usePageTitle";
import type { ServerGroup, TagMap } from "../types";

export default function GroupEdit() {
	const { id } = useParams<{ id?: string }>();
	const isCreate = id == null;
	usePageTitle(isCreate ? "New group" : "Edit group");
	const result = useApi(
		"fleet/groups",
		"get",
		{ server_group_id: id ?? "" },
		[id ?? ""],
	);

	if (isCreate) return <CreateForm />;

	if (result.status === "loading" || result.status === "idle") {
		return <LinearProgress />;
	}
	if (result.status === "error") {
		return <Alert severity="error">{result.error.message}</Alert>;
	}
	return (
		<EditForm
			group={result.data.group}
			memberCount={result.data.applications.length}
		/>
	);
}

/// Create mode. Collects just a name (notes/tags/cooldown are editable
/// afterwards) and, on success, drops the operator straight into the
/// add-server flow for the freshly-created group.
function CreateForm() {
	const navigate = useNavigate();
	const create = useApiAction("fleet/groups", "create");
	const [name, setName] = useState("");
	const [notes, setNotes] = useState("");
	const [tags, setTags] = useState<TagMap>({});

	const onSubmit = async (e: React.FormEvent) => {
		e.preventDefault();
		try {
			const group = await create.call({ name, notes, tags });
			navigate(`/fleet/groups/${group.id}/machines/new`);
		} catch {
			/* surfaced via create.error */
		}
	};

	return (
		<Paper variant="outlined" sx={{ p: 3 }} component="form" onSubmit={onSubmit}>
			<Stack spacing={2}>
				<Typography variant="h5" component="h1">
					New group
				</Typography>

				<TextField
					label="Name"
					value={name}
					onChange={(e) => setName(e.target.value)}
					disabled={create.pending}
					required
				/>

				<TextField
					label="Notes"
					multiline
					minRows={3}
					value={notes}
					onChange={(e) => setNotes(e.target.value)}
					disabled={create.pending}
					helperText="Plain text shown on the group's detail page."
				/>

				<Stack spacing={1}>
					<Typography variant="subtitle1">Tags</Typography>
					<TagsEditor value={tags} onChange={setTags} disabled={create.pending} />
					<Typography variant="caption" color="text.secondary">
						These tags are inherited by every server in the group.
					</Typography>
				</Stack>

				{create.error && (
					<Alert severity="error">{create.error.message}</Alert>
				)}

				<Stack direction="row" spacing={1}>
					<GradedAction calls="fleet/groups/create">
						<Button type="submit" variant="contained" disabled={create.pending}>
							{create.pending ? "Creating…" : "Create group"}
						</Button>
					</GradedAction>
					<Button
						type="button"
						variant="outlined"
						color="error"
						onClick={() => navigate("/fleet")}
						disabled={create.pending}
					>
						Cancel
					</Button>
				</Stack>
			</Stack>
		</Paper>
	);
}

function EditForm({
	group,
	memberCount,
}: {
	group: ServerGroup;
	memberCount: number;
}) {
	const navigate = useNavigate();
	const update = useApiAction("fleet/groups", "update");
	const remove = useApiAction("fleet/groups", "delete");

	const [name, setName] = useState(group.name);
	const [notes, setNotes] = useState(group.notes ?? "");
	const [tags, setTags] = useState<TagMap>(group.tags ?? {});
	// Slack open cooldown: how long a newly-opened incident's Slack notice
	// sits in the outbox before the drainer is allowed to ship it. UI works
	// in minutes; 0 = ship immediately.
	const [slackOpenDelayMinutes, setSlackOpenDelayMinutes] = useState<string>(
		Math.max(0, Math.round(group.slack_open_delay / 60)).toString(),
	);
	// Linger window: how long an incident stays open after its last failure
	// recovers, so a failure coming straight back continues the same
	// incident instead of opening (and re-notifying) a new one.
	const [slackCloseDelayMinutes, setSlackCloseDelayMinutes] = useState<string>(
		Math.max(0, Math.round(group.slack_close_delay / 60)).toString(),
	);

	const onSubmit = async (e: React.FormEvent) => {
		e.preventDefault();
		try {
			await update.call({
				server_group_id: group.id,
				data: {
					name,
					notes,
					tags,
					slack_open_delay: Math.max(
						0,
						Math.round(Number(slackOpenDelayMinutes) * 60),
					),
					slack_close_delay: Math.max(
						0,
						Math.round(Number(slackCloseDelayMinutes) * 60),
					),
				},
			});
			navigate(`/fleet/groups/${group.id}`);
		} catch {
			/* surfaced via update.error */
		}
	};

	const onArchive = async () => {
		if (memberCount > 0) {
			alert(
				`This group still has ${memberCount} server(s). Move them out before archiving.`,
			);
			return;
		}
		if (
			!confirm(
				`Archive group "${group.name}"? It's hidden from listings but can be restored from the Archived tab.`,
			)
		)
			return;
		try {
			await remove.call({ server_group_id: group.id });
			navigate("/fleet");
		} catch {
			/* surfaced via remove.error */
		}
	};

	const pending = update.pending || remove.pending;

	return (
		<Paper variant="outlined" sx={{ p: 3 }} component="form" onSubmit={onSubmit}>
			<Stack spacing={2}>
				<Typography variant="h5" component="h1">
					Edit group
				</Typography>

				<TextField
					label="Name"
					value={name}
					onChange={(e) => setName(e.target.value)}
					disabled={pending}
					required
				/>

				<TextField
					label="Notes"
					multiline
					minRows={3}
					value={notes}
					onChange={(e) => setNotes(e.target.value)}
					disabled={pending}
					helperText="Plain text shown on the group's detail page."
				/>

				<Stack spacing={1}>
					<Typography variant="subtitle1">Slack cooldown</Typography>
					<Stack
						direction={{ xs: "column", md: "row" }}
						spacing={2}
						sx={{ alignItems: { md: "center" } }}
					>
						<Typography variant="body2">
							Hold incident-open Slack notices for
						</Typography>
						<TextField
							label="minutes"
							type="number"
							value={slackOpenDelayMinutes}
							onChange={(e) => setSlackOpenDelayMinutes(e.target.value)}
							disabled={pending}
							slotProps={{ htmlInput: { min: 0, step: 1 } }}
							sx={{ width: 140 }}
						/>
					</Stack>
					<Typography variant="caption" color="text.secondary">
						If the incident resolves inside the window, no Slack notice is
						sent for either edge — useful for flappy probes. Set to 0 to ship
						opens immediately.
					</Typography>
					<Stack
						direction={{ xs: "column", md: "row" }}
						spacing={2}
						sx={{ alignItems: { md: "center" } }}
					>
						<Typography variant="body2">
							Keep incidents open after recovery for
						</Typography>
						<TextField
							label="minutes"
							type="number"
							value={slackCloseDelayMinutes}
							onChange={(e) => setSlackCloseDelayMinutes(e.target.value)}
							disabled={pending}
							slotProps={{ htmlInput: { min: 0, step: 1 } }}
							sx={{ width: 140 }}
						/>
					</Stack>
					<Typography variant="caption" color="text.secondary">
						A failure coming back inside this window continues the same
						incident instead of opening — and notifying about — a new one.
						The resolved notice waits it out. Set to 0 to close incidents
						the moment their last failure recovers.
					</Typography>
				</Stack>

				<Stack spacing={1}>
					<Typography variant="subtitle1">Tags</Typography>
					<TagsEditor value={tags} onChange={setTags} disabled={pending} />
					<Typography variant="caption" color="text.secondary">
						These tags are inherited by every server in the group. A server's
						own tags override the group's on the public tags endpoint.
					</Typography>
				</Stack>

				{(update.error || remove.error) && (
					<Alert severity="error">
						{(update.error || remove.error)!.message}
					</Alert>
				)}

				<Stack
					direction="row"
					spacing={1}
					sx={{ alignItems: "center", justifyContent: "space-between" }}
				>
					<Stack direction="row" spacing={1}>
						<GradedAction calls="fleet/groups/update">
							<Button
								type="submit"
								variant="contained"
								disabled={pending}
							>
								{update.pending ? "Saving…" : "Save"}
							</Button>
						</GradedAction>
						<Button
							type="button"
							variant="outlined"
							color="error"
							onClick={() => navigate(`/fleet/groups/${group.id}`)}
							disabled={pending}
						>
							Cancel
						</Button>
					</Stack>
					<GradedAction calls="fleet/groups/delete">
						<Button
							type="button"
							variant="outlined"
							color="error"
							startIcon={<ArchiveIcon />}
							onClick={onArchive}
							disabled={pending || memberCount > 0}
						>
							Archive group
						</Button>
					</GradedAction>
				</Stack>
			</Stack>
		</Paper>
	);
}
