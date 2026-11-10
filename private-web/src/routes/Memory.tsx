import {
	Alert as MuiAlert,
	Box,
	Button,
	Chip,
	Dialog,
	DialogActions,
	DialogContent,
	DialogTitle,
	FormControlLabel,
	LinearProgress,
	MenuItem,
	Paper,
	Stack,
	Switch,
	TextField,
	Typography,
} from "@mui/material";
import { useEffect, useState } from "react";
import { Link as RouterLink, useSearchParams } from "react-router-dom";
import { callApi, useApi } from "../api";
import { GradedAction } from "../components/GradedAction";
import TimeAgo from "../components/TimeAgo";
import { useDebounced } from "../hooks/useDebounced";
import { useIsAdmin } from "../hooks/useIsAdmin";
import { usePageTitle } from "../hooks/usePageTitle";
import {
	MEMORY_BODY_LIMIT,
	MEMORY_TITLE_LIMIT,
	type MemoryEntryData,
	type MemoryLink,
} from "../types";

const LINK_KINDS = ["fleet", "group", "check", "incident"] as const;
type LinkKind = (typeof LINK_KINDS)[number];

const KIND_LABEL: Record<LinkKind, string> = {
	fleet: "Fleet-wide",
	group: "Group",
	check: "Check",
	incident: "Incident",
};

function formatError(error: unknown): string {
	return error instanceof Error ? error.message : String(error);
}

/** Where a link leads, so a resolved link is also a way to the thing. */
function linkTarget(link: MemoryLink): string {
	switch (link.kind) {
		case "group":
			return `/fleet/groups/${link.id}`;
		case "incident":
			return `/incidents/${link.id}`;
		case "check":
			return `/healthchecks/${encodeURIComponent(link.source)}/${encodeURIComponent(link.name)}`;
	}
}

export default function Memory() {
	usePageTitle("Incident memory");
	const admin = useIsAdmin() === true;
	const [params, setParams] = useSearchParams();
	const [tick, setTick] = useState(0);
	const reload = () => setTick((t) => t + 1);
	const [recording, setRecording] = useState(false);

	// Everything that narrows the list lives in the address, so a filtered
	// list can be linked to.
	const text = params.get("q") ?? "";
	const kind = (params.get("kind") ?? "") as LinkKind | "";
	const group = params.get("group") ?? "";
	const checkSource = params.get("checkSource") ?? "";
	const checkName = params.get("checkName") ?? "";
	const incident = params.get("incident") ?? "";
	const relevant = params.get("relevant") ?? "";
	const retired = params.get("retired") === "1";
	const set = (key: string, value: string | null) =>
		setParams(
			(prev) => {
				const next = new URLSearchParams(prev);
				if (value === null || value === "") next.delete(key);
				else next.set(key, value);
				return next;
			},
			{ replace: true },
		);

	// The search box types ahead of the address, which follows once typing
	// pauses, so the list is queried once per settled term.
	const [typed, setTyped] = useState(text);
	const settled = useDebounced(typed, 250);
	useEffect(() => {
		if (settled.trim() !== text) set("q", settled.trim());
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [settled]);
	// A term that arrives from the address, rather than from typing, is shown.
	useEffect(() => {
		if (text !== typed.trim() && text !== settled.trim()) setTyped(text);
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [text]);

	const groups = useApi("fleet/groups", "list", {}, []);
	const list = useApi(
		"memory",
		"list",
		{
			text: text || null,
			link_kind: kind || null,
			group_id: group || null,
			check_source: checkSource || null,
			check_name: checkName || null,
			incident_id: incident || null,
			relevant_to_incident: relevant || null,
			include_retired: retired,
		},
		[text, kind, group, checkSource, checkName, incident, relevant, retired, tick],
	);

	return (
		<Stack spacing={3}>
			<Stack direction="row" sx={{ alignItems: "center" }}>
				<Typography variant="h4" component="h1" sx={{ flex: 1 }}>
					Incident memory
				</Typography>
				{admin && (
					<GradedAction calls="memory/record" action="Record a memory entry">
						<Button variant="contained" onClick={() => setRecording(true)}>
							Record entry
						</Button>
					</GradedAction>
				)}
			</Stack>

			<Paper variant="outlined" sx={{ p: 2 }}>
				<Stack
					direction="row"
					spacing={2}
					useFlexGap
					sx={{ alignItems: "center", flexWrap: "wrap" }}
				>
					<TextField
						size="small"
						label="Search"
						value={typed}
						onChange={(e) => setTyped(e.target.value)}
						sx={{ minWidth: 240 }}
					/>
					<TextField
						select
						size="small"
						label="Link"
						value={kind}
						onChange={(e) => set("kind", e.target.value)}
						sx={{ minWidth: 160 }}
					>
						<MenuItem value="">Any</MenuItem>
						{LINK_KINDS.map((k) => (
							<MenuItem key={k} value={k}>
								{KIND_LABEL[k]}
							</MenuItem>
						))}
					</TextField>
					<TextField
						select
						size="small"
						label="Group"
						value={group}
						onChange={(e) => set("group", e.target.value)}
						sx={{ minWidth: 200 }}
					>
						<MenuItem value="">Any</MenuItem>
						{groups.status === "ok" &&
							groups.data.map((g) => (
								<MenuItem key={g.id} value={g.id}>
									{g.name}
								</MenuItem>
							))}
					</TextField>
					<FormControlLabel
						control={
							<Switch
								size="small"
								checked={retired}
								onChange={(e) => set("retired", e.target.checked ? "1" : null)}
							/>
						}
						label="Include retired"
					/>
				</Stack>
				{(checkSource || incident || relevant) && (
					<Stack direction="row" spacing={1} sx={{ mt: 1.5 }}>
						{checkSource && (
							<Chip
								size="small"
								label={`Check ${checkName}`}
								onDelete={() => {
									set("checkSource", null);
									set("checkName", null);
								}}
							/>
						)}
						{incident && (
							<Chip
								size="small"
								label="One incident"
								onDelete={() => set("incident", null)}
							/>
						)}
						{relevant && (
							<Chip
								size="small"
								label="Relevant to an incident"
								onDelete={() => set("relevant", null)}
							/>
						)}
					</Stack>
				)}
			</Paper>

			{list.status === "loading" || list.status === "idle" ? (
				<LinearProgress />
			) : list.status === "error" ? (
				<MuiAlert severity="error">{list.error.message}</MuiAlert>
			) : list.data.entries.length === 0 ? (
				<Typography color="text.secondary">No entries.</Typography>
			) : (
				<Stack spacing={1}>
					{list.data.entries.map((entry) => (
						<EntryCard
							key={entry.id}
							entry={entry}
							admin={admin}
							onChanged={reload}
						/>
					))}
					{list.data.total > list.data.entries.length && (
						<Typography variant="caption" color="text.secondary">
							Showing {list.data.entries.length} of {list.data.total}.
						</Typography>
					)}
				</Stack>
			)}

			{recording && (
				<EntryDialog
					groups={groups.status === "ok" ? groups.data : []}
					onClose={() => setRecording(false)}
					onSaved={() => {
						setRecording(false);
						reload();
					}}
				/>
			)}
		</Stack>
	);
}

function LinkChip({ link }: { link: MemoryEntryData["link"] }) {
	if (!link) return <Chip size="small" variant="outlined" label="Fleet-wide" />;
	return (
		<Chip
			size="small"
			variant="outlined"
			clickable
			component={RouterLink}
			to={linkTarget(link)}
			label={`${KIND_LABEL[link.kind]}: ${link.name}`}
		/>
	);
}

function EntryCard({
	entry,
	admin,
	onChanged,
}: {
	entry: MemoryEntryData;
	admin: boolean;
	onChanged: () => void;
}) {
	const [open, setOpen] = useState(false);
	const [amending, setAmending] = useState(false);
	const [retiring, setRetiring] = useState(false);
	const [reason, setReason] = useState("");
	const [error, setError] = useState<string | null>(null);
	const detail = useApi("memory", "get", { id: entry.id }, [
		entry.id,
		entry.revision,
	], { skip: !open });

	const retire = async () => {
		setError(null);
		try {
			await callApi("memory", "retire", { id: entry.id, reason: reason.trim() });
			setRetiring(false);
			setReason("");
			onChanged();
		} catch (err) {
			setError(formatError(err));
		}
	};
	const restore = async (toRevision: number) => {
		setError(null);
		try {
			await callApi("memory", "restore", {
				id: entry.id,
				revision: entry.revision,
				to_revision: toRevision,
			});
			onChanged();
		} catch (err) {
			setError(formatError(err));
		}
	};

	return (
		<Paper variant="outlined" sx={{ p: 2, opacity: entry.retired_at ? 0.75 : 1 }}>
			<Stack
				direction="row"
				spacing={1}
				sx={{ alignItems: "center", flexWrap: "wrap" }}
				useFlexGap
			>
				<Typography
					variant="subtitle1"
					component="h2"
					sx={{ flex: 1, cursor: "pointer", minWidth: 240 }}
					onClick={() => setOpen((v) => !v)}
				>
					{entry.title}
				</Typography>
				{entry.retired_at && <Chip size="small" label="Retired" />}
				<LinkChip link={entry.link} />
				<Typography variant="caption" color="text.secondary">
					<TimeAgo timestamp={entry.updated_at} />
				</Typography>
			</Stack>
			{open && (
				<Box sx={{ mt: 1.5 }}>
					<Typography variant="body2" sx={{ whiteSpace: "pre-wrap" }}>
						{entry.body}
					</Typography>
					<Typography
						variant="caption"
						color="text.secondary"
						sx={{ display: "block", mt: 1 }}
					>
						Recorded by {entry.created_by}, last changed by {entry.updated_by}
						{entry.retired_at &&
							`; retired by ${entry.retired_by}: ${entry.retired_reason}`}
					</Typography>
					{admin && (
						<Stack direction="row" spacing={1} sx={{ mt: 1.5 }}>
							{!entry.retired_at && (
								<GradedAction calls="memory/amend" action="Amend this entry">
									<Button size="small" onClick={() => setAmending(true)}>
										Amend
									</Button>
								</GradedAction>
							)}
							{!entry.retired_at && (
								<GradedAction calls="memory/retire" action="Retire this entry">
									<Button size="small" onClick={() => setRetiring((v) => !v)}>
										Retire
									</Button>
								</GradedAction>
							)}
						</Stack>
					)}
					{retiring && (
						<Stack direction="row" spacing={1} sx={{ mt: 1 }}>
							<TextField
								size="small"
								fullWidth
								label="Why it no longer holds"
								value={reason}
								onChange={(e) => setReason(e.target.value)}
							/>
							<GradedAction calls="memory/retire" action="Retire this entry">
								<Button
									variant="contained"
									disabled={reason.trim() === ""}
									onClick={retire}
								>
									Retire
								</Button>
							</GradedAction>
						</Stack>
					)}
					{error && (
						<MuiAlert severity="error" sx={{ mt: 1 }}>
							{error}
						</MuiAlert>
					)}
					<Typography variant="subtitle2" sx={{ mt: 2 }}>
						History
					</Typography>
					{detail.status === "ok" ? (
						<Stack spacing={0.5} sx={{ mt: 0.5 }}>
							{detail.data.history.map((version) => (
								<Stack
									key={version.revision}
									direction="row"
									spacing={1}
									sx={{ alignItems: "center" }}
								>
									<Typography variant="caption" color="text.secondary" sx={{ flex: 1 }}>
										{version.kind} by {version.actor} through{" "}
										{version.via === "token" ? "an access token" : "the operator interface"}
										{" · "}
										<TimeAgo timestamp={version.created_at} />
										{version.reason && ` · ${version.reason}`}
										{version.restored_from != null &&
											` · from version ${version.restored_from}`}
									</Typography>
									{admin &&
										(version.revision !== entry.revision || entry.retired_at) && (
											<GradedAction
												calls="memory/restore"
												action="Restore this entry to an earlier version"
											>
												<Button
													size="small"
													onClick={() => restore(version.revision)}
												>
													Restore
												</Button>
											</GradedAction>
										)}
								</Stack>
							))}
						</Stack>
					) : (
						<LinearProgress />
					)}
				</Box>
			)}
			{amending && (
				<EntryDialog
					entry={entry}
					groups={[]}
					onClose={() => setAmending(false)}
					onSaved={() => {
						setAmending(false);
						onChanged();
					}}
				/>
			)}
		</Paper>
	);
}

/** Records a new entry, or amends `entry`: its title and body. A new entry
 * can also be given a link. */
function EntryDialog({
	entry,
	groups,
	onClose,
	onSaved,
}: {
	entry?: MemoryEntryData;
	groups: { id: string; name: string }[];
	onClose: () => void;
	onSaved: () => void;
}) {
	const [title, setTitle] = useState(entry?.title ?? "");
	const [body, setBody] = useState(entry?.body ?? "");
	const [kind, setKind] = useState<LinkKind>("fleet");
	const [group, setGroup] = useState("");
	const [checkSource, setCheckSource] = useState("");
	const [checkName, setCheckName] = useState("");
	const [incident, setIncident] = useState("");
	const [pending, setPending] = useState(false);
	const [error, setError] = useState<string | null>(null);

	const link = (): MemoryLink | null => {
		switch (kind) {
			case "fleet":
				return null;
			case "group":
				return { kind: "group", id: group };
			case "incident":
				return { kind: "incident", id: incident.trim() };
			case "check":
				return {
					kind: "check",
					source: checkSource.trim(),
					name: checkName.trim(),
				};
		}
	};

	const save = async () => {
		setPending(true);
		setError(null);
		try {
			if (entry) {
				await callApi("memory", "amend", {
					id: entry.id,
					revision: entry.revision,
					title: title.trim(),
					body: body.trim(),
				});
			} else {
				await callApi("memory", "record", {
					title: title.trim(),
					body: body.trim(),
					link: link(),
				});
			}
			onSaved();
		} catch (err) {
			setError(formatError(err));
		} finally {
			setPending(false);
		}
	};

	const call = entry ? "memory/amend" : "memory/record";
	return (
		<Dialog open onClose={onClose} fullWidth maxWidth="sm">
			<DialogTitle>{entry ? "Amend entry" : "Record entry"}</DialogTitle>
			<DialogContent>
				<Stack spacing={2} sx={{ mt: 1 }}>
					<TextField
						autoFocus
						label="Title"
						value={title}
						onChange={(e) => setTitle(e.target.value)}
						slotProps={{ htmlInput: { maxLength: MEMORY_TITLE_LIMIT } }}
					/>
					<TextField
						label="Body"
						multiline
						minRows={4}
						value={body}
						onChange={(e) => setBody(e.target.value)}
						slotProps={{ htmlInput: { maxLength: MEMORY_BODY_LIMIT } }}
					/>
					{!entry && (
						<>
							<TextField
								select
								label="Applies to"
								value={kind}
								onChange={(e) => setKind(e.target.value as LinkKind)}
							>
								{LINK_KINDS.map((k) => (
									<MenuItem key={k} value={k}>
										{KIND_LABEL[k]}
									</MenuItem>
								))}
							</TextField>
							{kind === "group" && (
								<TextField
									select
									label="Group"
									value={group}
									onChange={(e) => setGroup(e.target.value)}
								>
									{groups.map((g) => (
										<MenuItem key={g.id} value={g.id}>
											{g.name}
										</MenuItem>
									))}
								</TextField>
							)}
							{kind === "check" && (
								<Stack direction="row" spacing={1}>
									<TextField
										label="Check source"
										value={checkSource}
										onChange={(e) => setCheckSource(e.target.value)}
									/>
									<TextField
										label="Check name"
										value={checkName}
										onChange={(e) => setCheckName(e.target.value)}
									/>
								</Stack>
							)}
							{kind === "incident" && (
								<TextField
									label="Incident id"
									value={incident}
									onChange={(e) => setIncident(e.target.value)}
								/>
							)}
						</>
					)}
					{error && <MuiAlert severity="error">{error}</MuiAlert>}
				</Stack>
			</DialogContent>
			<DialogActions>
				<Button onClick={onClose} disabled={pending}>
					Cancel
				</Button>
				<GradedAction calls={call} action={entry ? "Amend this entry" : "Record a memory entry"}>
					<Button
						variant="contained"
						onClick={save}
						disabled={pending || title.trim() === ""}
					>
						{pending ? "Saving…" : "Save"}
					</Button>
				</GradedAction>
			</DialogActions>
		</Dialog>
	);
}
