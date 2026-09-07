import {
	Alert,
	Box,
	Button,
	Chip,
	Dialog,
	DialogActions,
	DialogContent,
	DialogTitle,
	IconButton,
	LinearProgress,
	MenuItem,
	Paper,
	Select,
	Stack,
	Table,
	TableBody,
	TableCell,
	TableContainer,
	TableHead,
	TableRow,
	TextField,
	Typography,
} from "@mui/material";
import CheckCircleIcon from "@mui/icons-material/CheckCircle";
import DeleteIcon from "@mui/icons-material/Delete";
import EditIcon from "@mui/icons-material/Edit";
import ErrorOutlineIcon from "@mui/icons-material/ErrorOutlined";
import LockIcon from "@mui/icons-material/Lock";
import LockOpenIcon from "@mui/icons-material/LockOpen";
import { useState } from "react";
import { useParams } from "react-router-dom";
import Markdown from "../components/Markdown";
import TimeAgo from "../components/TimeAgo";
import VersionStatusChip from "../components/VersionStatusChip";
import { useApi, useApiAction } from "../api";
import { useIsAdmin } from "../hooks/useIsAdmin";
import { usePageTitle } from "../hooks/usePageTitle";
import { prettifyVersionRange } from "../lib/versionRange";
import type {
	ArtifactData,
	KnownIssueData,
	RelatedVersionData,
	VersionDetail as VersionDetailData,
	VersionStatus,
} from "../types";

export default function VersionDetail() {
	const { version = "" } = useParams<{ version: string }>();
	usePageTitle(version || "Version");
	const detail = useApi(
		"versions",
		"get_version_detail",
		{ version },
		[version],
	);
	const admin = useIsAdmin() === true;

	if (detail.status === "loading" || detail.status === "idle") {
		return <LinearProgress />;
	}
	if (detail.status === "error") {
		return <Alert severity="error">{detail.error.message}</Alert>;
	}

	const v = detail.data;
	const versionStr = `${v.major}.${v.minor}.${v.patch}`;

	return (
		<Stack spacing={3}>
			<Stack
				direction="row"
				spacing={2}
				sx={{ alignItems: "center", justifyContent: "space-between" }}
			>
				<Stack direction="row" spacing={2} sx={{ alignItems: "center" }}>
					<Typography variant="h4" component="h1" sx={{ fontFamily: "monospace" }}>
						{versionStr}
					</Typography>
					<ReadyChip ready={v.ready} />
				</Stack>
				<StatusControl
					// Remount per version: `selected` is seeded from
					// `detail.status` once, so a reused instance would carry
					// the previous version's status into the new one — and
					// submitting would write it there.
					key={versionStr}
					detail={v}
					versionStr={versionStr}
					isAdmin={admin}
					onChanged={() => detail.reload()}
				/>
			</Stack>

			<VersionInfo detail={v} />

			<ArtifactsSection
				version={versionStr}
				versionId={v.id}
				isAdmin={admin}
			/>

			<ChangelogSection
				detail={v}
				versionStr={versionStr}
				isAdmin={admin}
				onChanged={() => detail.reload()}
			/>

			<KnownIssuesSection
				versionId={v.id}
				currentVersion={{ major: v.major, minor: v.minor, patch: v.patch }}
				issues={v.known_issues}
				isAdmin={admin}
				onChanged={() => detail.reload()}
			/>

			{v.related_versions.length > 0 && (
				<RelatedVersionsSection related={v.related_versions} />
			)}
		</Stack>
	);
}

function StatusControl({
	detail,
	versionStr,
	isAdmin,
	onChanged,
}: {
	detail: VersionDetailData;
	versionStr: string;
	isAdmin: boolean;
	onChanged: () => void;
}) {
	const [selected, setSelected] = useState<VersionStatus>(detail.status);
	const action = useApiAction("versions", "update_version_status");

	if (!isAdmin) {
		return <VersionStatusChip status={detail.status} />;
	}

	const canSwitchToDraft =
		detail.status !== "published" || detail.is_latest_in_minor;
	const dirty = selected !== detail.status;

	return (
		<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
			<Select
				size="small"
				value={selected}
				onChange={(e) => setSelected(e.target.value as VersionStatus)}
				disabled={action.pending}
			>
				<MenuItem value="draft" disabled={!canSwitchToDraft}>
					Draft
				</MenuItem>
				<MenuItem value="published">Published</MenuItem>
				<MenuItem value="yanked">Yanked</MenuItem>
			</Select>
			<Button
				variant="contained"
				disabled={!dirty || action.pending}
				onClick={async () => {
					try {
						await action.call({ version: versionStr, status: selected });
						onChanged();
					} catch {
						/* surfaced via action.error */
					}
				}}
			>
				{action.pending ? "Changing…" : "Change"}
			</Button>
			{action.error && (
				<Typography variant="caption" color="error">
					{action.error.message}
				</Typography>
			)}
		</Stack>
	);
}

function ChangelogSection({
	detail,
	versionStr,
	isAdmin,
	onChanged,
}: {
	detail: VersionDetailData;
	versionStr: string;
	isAdmin: boolean;
	onChanged: () => void;
}) {
	const [editing, setEditing] = useState(false);
	const [draft, setDraft] = useState(detail.changelog);
	const action = useApiAction("versions", "update_version_changelog");

	const start = () => {
		setDraft(detail.changelog);
		setEditing(true);
	};
	const cancel = () => {
		setDraft(detail.changelog);
		setEditing(false);
	};
	const save = async () => {
		try {
			await action.call({ version: versionStr, changelog: draft });
			setEditing(false);
			onChanged();
		} catch {
			/* surfaced via action.error */
		}
	};

	return (
		<Box>
			<Stack
				direction="row"
				spacing={1}
				sx={{ mb: 1, alignItems: "center", justifyContent: "space-between" }}
			>
				<Typography variant="h5" component="h2">
					Changelog
				</Typography>
				{isAdmin &&
					(editing ? (
						<Stack direction="row" spacing={1}>
							<Button
								variant="contained"
								color="success"
								onClick={save}
								disabled={action.pending}
							>
								{action.pending ? "Saving…" : "Save"}
							</Button>
							<Button
								variant="outlined"
								color="error"
								onClick={cancel}
								disabled={action.pending}
							>
								Cancel
							</Button>
						</Stack>
					) : (
						<Button
							variant="outlined"
							startIcon={<EditIcon />}
							onClick={start}
						>
							Edit
						</Button>
					))}
			</Stack>
			<Paper variant="outlined" sx={{ p: 2 }}>
				{editing ? (
					<TextField
						multiline
						fullWidth
						minRows={20}
						value={draft}
						onChange={(e) => setDraft(e.target.value)}
						slotProps={{
							input: { sx: { fontFamily: "monospace" } },
						}}
					/>
				) : detail.changelog ? (
					<Markdown>{detail.changelog}</Markdown>
				) : (
					<Typography variant="body2" color="text.secondary">
						No changelog
					</Typography>
				)}
			</Paper>
			{action.error && (
				<Alert severity="error" sx={{ mt: 1 }}>
					{action.error.message}
				</Alert>
			)}
		</Box>
	);
}

function VersionInfo({ detail }: { detail: VersionDetailData }) {
	const items = [
		{ label: "Created", value: formatDate(detail.created_at) },
		{ label: "Last updated", value: formatDate(detail.updated_at) },
		detail.min_chrome_version != null && {
			label: "Chrome support",
			value: `${detail.min_chrome_version} or later`,
		},
	].filter(Boolean) as Array<{ label: string; value: string }>;

	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<Stack
				direction="row"
				spacing={4}
				useFlexGap
				sx={{ flexWrap: "wrap" }}
			>
				{items.map(({ label, value }) => (
					<Stack key={label} spacing={0.25}>
						<Typography variant="caption" color="text.secondary">
							{label}
						</Typography>
						<Typography variant="body2">{value}</Typography>
					</Stack>
				))}
			</Stack>
		</Paper>
	);
}

function ArtifactsSection({
	version,
	versionId,
	isAdmin,
}: {
	version: string;
	versionId: string;
	isAdmin: boolean;
}) {
	const result = useApi(
		"versions",
		"get_version_artifacts",
		{ version },
		[version],
	);
	const [unlocked, setUnlocked] = useState(false);
	const [showCreate, setShowCreate] = useState(false);

	const reload = () => result.reload();

	return (
		<Box>
			<Stack
				direction="row"
				spacing={1}
				sx={{ mb: 1, alignItems: "center", justifyContent: "space-between" }}
			>
				<Typography variant="h5" component="h2">
					Artifacts
				</Typography>
				{isAdmin && (
					<Stack direction="row" spacing={1}>
						{unlocked && (
							<Button
								variant={showCreate ? "outlined" : "contained"}
								color={showCreate ? "warning" : "primary"}
								onClick={() => setShowCreate((s) => !s)}
							>
								{showCreate ? "Cancel create" : "Create"}
							</Button>
						)}
						<Button
							variant="outlined"
							startIcon={unlocked ? <LockOpenIcon /> : <LockIcon />}
							onClick={() => setUnlocked((u) => !u)}
						>
							{unlocked ? "Lock" : "Unlock"}
						</Button>
					</Stack>
				)}
			</Stack>
			{showCreate && (
				<Box sx={{ mb: 2 }}>
					<CreateArtifactForm
						versionId={versionId}
						onCreated={() => {
							setShowCreate(false);
							reload();
						}}
					/>
				</Box>
			)}
			{result.status === "loading" || result.status === "idle" ? (
				<LinearProgress />
			) : result.status === "error" ? (
				<Alert severity="error">{result.error.message}</Alert>
			) : result.data.length === 0 ? (
				<Alert severity="info">No artifacts for this version.</Alert>
			) : (
				<TableContainer component={Paper} variant="outlined">
					<Table size="small">
						<TableHead>
							<TableRow>
								<TableCell>Type</TableCell>
								<TableCell>Platform</TableCell>
								<TableCell>Download URL</TableCell>
								{unlocked && <TableCell />}
							</TableRow>
						</TableHead>
						<TableBody>
							{result.data.map((a) => (
								<ArtifactRow
									key={a.id}
									artifact={a}
									unlocked={unlocked}
									onChanged={reload}
								/>
							))}
						</TableBody>
					</Table>
				</TableContainer>
			)}
		</Box>
	);
}

function ArtifactRow({
	artifact,
	unlocked,
	onChanged,
}: {
	artifact: ArtifactData;
	unlocked: boolean;
	onChanged: () => void;
}) {
	const [editing, setEditing] = useState(false);
	const [confirmDelete, setConfirmDelete] = useState(false);
	const deleteAction = useApiAction("versions", "delete_artifact");

	if (editing) {
		return (
			<EditArtifactRow
				artifact={artifact}
				onClose={(changed) => {
					setEditing(false);
					if (changed) onChanged();
				}}
			/>
		);
	}

	const onDelete = async () => {
		try {
			await deleteAction.call({ artifact_id: artifact.id });
			onChanged();
		} catch {
			/* surfaced via deleteAction.error */
		}
	};

	return (
		<TableRow>
			<TableCell sx={{ fontFamily: "monospace" }}>
				{artifact.artifact_type}
				{artifact.has_range_override && (
					<Typography
						variant="caption"
						color="warning.main"
						sx={{ display: "block" }}
					>
						Overrides other artifact
					</Typography>
				)}
				{artifact.version_range_pattern && (
					<Typography
						variant="caption"
						color="text.secondary"
						sx={{ display: "block" }}
					>
						{!artifact.is_used_in_public_api && (
							<Box component="span" sx={{ color: "error.main", mr: 0.5 }}>
								[Hidden]
							</Box>
						)}
						Applies to: {prettifyVersionRange(artifact.version_range_pattern)}
					</Typography>
				)}
			</TableCell>
			<TableCell sx={{ fontFamily: "monospace" }}>
				{artifact.platform}
			</TableCell>
			<TableCell sx={{ wordBreak: "break-all" }}>
				{artifact.canopy_holds_bytes ? (
					<Stack spacing={0.25}>
						<Typography variant="body2">
							Held by Canopy for {artifact.group_name ?? "a group"}
						</Typography>
						{artifact.digest && (
							<Typography
								variant="caption"
								color="text.secondary"
								sx={{ fontFamily: "monospace" }}
							>
								{artifact.digest}
							</Typography>
						)}
					</Stack>
				) : artifact.download_url?.startsWith("https://") ? (
					<a
						href={artifact.download_url}
						target="_blank"
						rel="noopener noreferrer"
					>
						{artifact.download_url}
					</a>
				) : (
					artifact.download_url
				)}
			</TableCell>
			{unlocked && (
				<TableCell align="right">
					{confirmDelete ? (
						<Stack direction="row" spacing={0.5} sx={{ justifyContent: "flex-end" }}>
							<Button
								size="small"
								variant="contained"
								color="error"
								onClick={onDelete}
								disabled={deleteAction.pending}
							>
								Really delete
							</Button>
							<Button
								size="small"
								variant="outlined"
								onClick={() => setConfirmDelete(false)}
								disabled={deleteAction.pending}
							>
								Cancel
							</Button>
						</Stack>
					) : (
						<Stack direction="row" spacing={0.5} sx={{ justifyContent: "flex-end" }}>
							<IconButton
								aria-label={`edit ${artifact.artifact_type}`}
								size="small"
								onClick={() => setEditing(true)}
							>
								<EditIcon fontSize="small" />
							</IconButton>
							<IconButton
								aria-label={`delete ${artifact.artifact_type}`}
								size="small"
								color="error"
								onClick={() => setConfirmDelete(true)}
							>
								<DeleteIcon fontSize="small" />
							</IconButton>
						</Stack>
					)}
				</TableCell>
			)}
		</TableRow>
	);
}

function EditArtifactRow({
	artifact,
	onClose,
}: {
	artifact: ArtifactData;
	onClose: (changed: boolean) => void;
}) {
	const [type, setType] = useState(artifact.artifact_type);
	const [platform, setPlatform] = useState(artifact.platform);
	const [url, setUrl] = useState(artifact.download_url);
	const action = useApiAction("versions", "update_artifact");

	const save = async () => {
		try {
			await action.call({
				artifact_id: artifact.id,
				artifact_type: type,
				platform,
				download_url: artifact.canopy_holds_bytes ? null : url,
			});
			onClose(true);
		} catch {
			/* surfaced via action.error */
		}
	};

	return (
		<TableRow>
			<TableCell>
				<TextField
					size="small"
					value={type}
					onChange={(e) => setType(e.target.value)}
					disabled={action.pending}
					required
				/>
			</TableCell>
			<TableCell>
				<TextField
					size="small"
					value={platform}
					onChange={(e) => setPlatform(e.target.value)}
					disabled={action.pending}
					required
				/>
			</TableCell>
			<TableCell>
				{artifact.canopy_holds_bytes ? (
					<Typography variant="body2" color="text.secondary">
						Held by Canopy for {artifact.group_name ?? "a group"}. Register
						it again to replace the bytes.
					</Typography>
				) : (
					<TextField
						size="small"
						fullWidth
						value={url ?? ""}
						onChange={(e) => setUrl(e.target.value)}
						disabled={action.pending}
						required
					/>
				)}
			</TableCell>
			<TableCell align="right">
				<Stack direction="row" spacing={0.5} sx={{ justifyContent: "flex-end" }}>
					<Button
						size="small"
						variant="contained"
						onClick={save}
						disabled={action.pending}
					>
						{action.pending ? "Saving…" : "Save"}
					</Button>
					<Button
						size="small"
						variant="outlined"
						onClick={() => onClose(false)}
						disabled={action.pending}
					>
						Cancel
					</Button>
				</Stack>
			</TableCell>
		</TableRow>
	);
}

/// Canopy holds a group-scoped artifact's bytes, and the API takes them in the
/// JSON body, so the file is read here rather than posted as a multipart form.
async function encodeFile(file: File): Promise<string> {
	const buffer = new Uint8Array(await file.arrayBuffer());
	let binary = "";
	for (const byte of buffer) binary += String.fromCharCode(byte);
	return btoa(binary);
}

function CreateArtifactForm({
	versionId,
	onCreated,
}: {
	versionId: string;
	onCreated: () => void;
}) {
	const [type, setType] = useState("");
	const [platform, setPlatform] = useState("");
	const [url, setUrl] = useState("");
	const [groupId, setGroupId] = useState("");
	const [file, setFile] = useState<File | null>(null);
	const action = useApiAction("versions", "create_artifact");
	const groups = useApi("fleet/groups", "list", {}, []);

	const scoped = groupId !== "";

	const submit = async (e: React.FormEvent) => {
		e.preventDefault();
		try {
			await action.call({
				version_id: versionId,
				artifact_type: type,
				platform,
				download_url: scoped ? null : url,
				group_id: scoped ? groupId : null,
				content_base64: file ? await encodeFile(file) : null,
				content_type: file ? file.type || null : null,
			});
			setType("");
			setPlatform("");
			setUrl("");
			setGroupId("");
			setFile(null);
			onCreated();
		} catch {
			/* surfaced via action.error */
		}
	};

	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<Box component="form" onSubmit={submit}>
				<Stack
					direction="row"
					spacing={1}
					sx={{ alignItems: "flex-start" }}
				>
					<TextField
						size="small"
						label="Type"
						value={type}
						onChange={(e) => setType(e.target.value)}
						disabled={action.pending}
						required
					/>
					<TextField
						size="small"
						label="Platform"
						value={platform}
						onChange={(e) => setPlatform(e.target.value)}
						disabled={action.pending}
						required
					/>
					<TextField
						size="small"
						select
						label="Group"
						value={groupId}
						onChange={(e) => setGroupId(e.target.value)}
						disabled={action.pending}
						sx={{ minWidth: 160 }}
					>
						<MenuItem value="">Every group</MenuItem>
						{(groups.status === "ok" ? groups.data : []).map((g) => (
							<MenuItem key={g.id} value={g.id}>
								{g.name}
							</MenuItem>
						))}
					</TextField>
					{scoped ? (
						<Button
							component="label"
							variant="outlined"
							disabled={action.pending}
							sx={{ flexGrow: 1, justifyContent: "flex-start" }}
						>
							{file ? file.name : "Choose file…"}
							<input
								type="file"
								hidden
								onChange={(e) => setFile(e.target.files?.[0] ?? null)}
							/>
						</Button>
					) : (
						<TextField
							size="small"
							label="Download URL"
							value={url}
							onChange={(e) => setUrl(e.target.value)}
							disabled={action.pending}
							fullWidth
							required
						/>
					)}
					<Button
						type="submit"
						variant="contained"
						disabled={action.pending || (scoped && !file)}
					>
						{action.pending ? "Creating…" : "Create"}
					</Button>
				</Stack>
				{action.error && (
					<Alert severity="error" sx={{ mt: 1 }}>
						{action.error.message}
					</Alert>
				)}
			</Box>
		</Paper>
	);
}

function RelatedVersionsSection({
	related,
}: {
	related: RelatedVersionData[];
}) {
	return (
		<Box>
			<Typography variant="h5" component="h2" gutterBottom>
				Related versions
			</Typography>
			<Stack spacing={2}>
				{related.map((r) => (
					<Box key={`${r.major}.${r.minor}.${r.patch}`}>
						<Typography
							variant="h6"
							component="h3"
							sx={{ fontFamily: "monospace" }}
						>
							{r.major}.{r.minor}.{r.patch}
						</Typography>
						<Paper variant="outlined" sx={{ p: 2 }}>
							{r.changelog ? (
								<Markdown>{r.changelog}</Markdown>
							) : (
								<Typography variant="body2" color="text.secondary">
									No changelog
								</Typography>
							)}
						</Paper>
					</Box>
				))}
			</Stack>
		</Box>
	);
}

function formatDate(iso: string): string {
	return iso.slice(0, 10);
}

function ReadyChip({ ready }: { ready: boolean }) {
	if (ready) {
		return (
			<Chip
				size="small"
				color="success"
				variant="outlined"
				icon={<CheckCircleIcon />}
				label="Ready"
			/>
		);
	}
	return (
		<Chip
			size="small"
			color="warning"
			variant="outlined"
			icon={<ErrorOutlineIcon />}
			label="Known issues"
		/>
	);
}

function KnownIssuesSection({
	versionId,
	currentVersion,
	issues,
	isAdmin,
	onChanged,
}: {
	versionId: string;
	currentVersion: { major: number; minor: number; patch: number };
	issues: KnownIssueData[];
	isAdmin: boolean;
	onChanged: () => void;
}) {
	const [addOpen, setAddOpen] = useState(false);

	const open = issues.filter((i) => i.resolved_at == null);
	const resolved = issues.filter((i) => i.resolved_at != null);

	return (
		<Box>
			<Stack
				direction="row"
				spacing={1}
				sx={{ mb: 1, alignItems: "center", justifyContent: "space-between" }}
			>
				<Typography variant="h5" component="h2">
					Known issues
				</Typography>
				{isAdmin && (
					<Button
						variant="outlined"
						size="small"
						onClick={() => setAddOpen(true)}
					>
						Add known issue
					</Button>
				)}
			</Stack>

			{issues.length === 0 ? (
				<Typography variant="body2" color="text.secondary">
					No known issues for this minor.
				</Typography>
			) : (
				<Stack spacing={1}>
					{open.map((k) => (
						<KnownIssueRow
							key={k.id}
							issue={k}
							currentVersion={currentVersion}
							isAdmin={isAdmin}
							onChanged={onChanged}
						/>
					))}
					{resolved.length > 0 && open.length > 0 && (
						<Typography
							variant="caption"
							color="text.secondary"
							sx={{ mt: 1 }}
						>
							Resolved
						</Typography>
					)}
					{resolved.map((k) => (
						<KnownIssueRow
							key={k.id}
							issue={k}
							currentVersion={currentVersion}
							isAdmin={isAdmin}
							onChanged={onChanged}
						/>
					))}
				</Stack>
			)}

			<AddKnownIssueDialog
				open={addOpen}
				onClose={() => setAddOpen(false)}
				versionId={versionId}
				onAdded={() => {
					setAddOpen(false);
					onChanged();
				}}
			/>
		</Box>
	);
}

function formatIssueRange(issue: KnownIssueData): string {
	const min = `${issue.min_major}.${issue.min_minor}.${issue.min_patch}`;
	if (
		issue.max_major == null ||
		issue.max_minor == null ||
		issue.max_patch == null
	) {
		return `Affects ${min} and later in ${issue.min_major}.${issue.min_minor}.x`;
	}
	const fix = `${issue.max_major}.${issue.max_minor}.${issue.max_patch}`;
	const lastAffected = `${issue.max_major}.${issue.max_minor}.${issue.max_patch - 1}`;
	if (issue.max_patch - 1 === issue.min_patch) {
		return `Affects ${min} (fixed in ${fix})`;
	}
	return `Affects ${min} – ${lastAffected} (fixed in ${fix})`;
}

function KnownIssueRow({
	issue,
	currentVersion,
	isAdmin,
	onChanged,
}: {
	issue: KnownIssueData;
	currentVersion: { major: number; minor: number; patch: number };
	isAdmin: boolean;
	onChanged: () => void;
}) {
	const [resolveOpen, setResolveOpen] = useState(false);
	const open = issue.resolved_at == null;

	return (
		<Paper
			variant="outlined"
			sx={{
				p: 1.5,
				borderLeft: 4,
				borderLeftColor: open ? "warning.main" : "success.main",
			}}
		>
			<Stack
				direction="row"
				spacing={1}
				sx={{ alignItems: "center", justifyContent: "space-between" }}
			>
				<Typography variant="caption" color="text.secondary">
					{issue.author} • <TimeAgo timestamp={issue.created_at} />
				</Typography>
				{open && isAdmin && (
					<Button size="small" onClick={() => setResolveOpen(true)}>
						Resolve
					</Button>
				)}
			</Stack>
			<Typography
				variant="caption"
				color="text.secondary"
				sx={{ display: "block", fontFamily: "monospace" }}
			>
				{formatIssueRange(issue)}
			</Typography>
			<Typography
				variant="body2"
				component="pre"
				sx={{ mt: 0.5, mb: 0, whiteSpace: "pre-wrap", fontFamily: "inherit" }}
			>
				{issue.description}
			</Typography>
			{!open && issue.resolution_message && (
				<Box
					sx={{
						mt: 1,
						pt: 1,
						borderTop: 1,
						borderTopColor: "divider",
					}}
				>
					<Typography variant="caption" color="text.secondary">
						Resolved by {issue.resolved_by ?? "?"}
						{issue.resolved_at && (
							<>
								{" "}
								(<TimeAgo timestamp={issue.resolved_at} />)
							</>
						)}
					</Typography>
					<Typography
						variant="body2"
						component="pre"
						sx={{ mt: 0.5, whiteSpace: "pre-wrap", fontFamily: "inherit" }}
					>
						{issue.resolution_message}
					</Typography>
				</Box>
			)}
			<ResolveKnownIssueDialog
				open={resolveOpen}
				onClose={() => setResolveOpen(false)}
				issue={issue}
				currentVersion={currentVersion}
				onResolved={() => {
					setResolveOpen(false);
					onChanged();
				}}
			/>
		</Paper>
	);
}

function AddKnownIssueDialog({
	open,
	onClose,
	versionId,
	onAdded,
}: {
	open: boolean;
	onClose: () => void;
	versionId: string;
	onAdded: () => void;
}) {
	const [draft, setDraft] = useState("");
	const action = useApiAction("versions", "add_known_issue");
	const submit = async () => {
		const description = draft.trim();
		if (description === "") return;
		try {
			await action.call({ version_id: versionId, description });
			setDraft("");
			onAdded();
		} catch {
			/* surfaced via action.error */
		}
	};
	const cancel = () => {
		setDraft("");
		onClose();
	};
	return (
		<Dialog open={open} onClose={cancel} fullWidth maxWidth="sm">
			<DialogTitle>Add known issue</DialogTitle>
			<DialogContent>
				<TextField
					autoFocus
					fullWidth
					multiline
					minRows={3}
					value={draft}
					onChange={(e) => setDraft(e.target.value)}
					disabled={action.pending}
					placeholder="Describe the issue, including any user-facing impact"
					sx={{ mt: 1 }}
				/>
				{action.error && (
					<Alert severity="error" sx={{ mt: 1 }}>
						{action.error.message}
					</Alert>
				)}
			</DialogContent>
			<DialogActions>
				<Button onClick={cancel} disabled={action.pending}>
					Cancel
				</Button>
				<Button
					variant="contained"
					onClick={submit}
					disabled={action.pending || draft.trim() === ""}
				>
					{action.pending ? "Adding…" : "Add"}
				</Button>
			</DialogActions>
		</Dialog>
	);
}

function ResolveKnownIssueDialog({
	open,
	onClose,
	issue,
	currentVersion,
	onResolved,
}: {
	open: boolean;
	onClose: () => void;
	issue: KnownIssueData;
	currentVersion: { major: number; minor: number; patch: number };
	onResolved: () => void;
}) {
	const minPatch = issue.min_patch;
	// Default to the page's version if it's a plausible fix (same minor,
	// strictly above the issue's min) — operators often resolve from the
	// detail page of the version that carries the fix. Otherwise fall
	// back to the smallest narrowing (min_patch + 1).
	const currentIsValidFix =
		currentVersion.major === issue.min_major &&
		currentVersion.minor === issue.min_minor &&
		currentVersion.patch > minPatch;
	const defaultFix = currentIsValidFix
		? `${currentVersion.major}.${currentVersion.minor}.${currentVersion.patch}`
		: `${issue.min_major}.${issue.min_minor}.${minPatch + 1}`;
	const [fixVersion, setFixVersion] = useState(defaultFix);
	const [draft, setDraft] = useState("");
	const action = useApiAction("versions", "resolve_known_issue");

	const parsed = parseSemver(fixVersion);
	const sameMinor =
		parsed != null &&
		parsed.major === issue.min_major &&
		parsed.minor === issue.min_minor;
	const aboveMin = parsed != null && parsed.patch > minPatch;
	const fixValid = parsed != null && sameMinor && aboveMin;

	const submit = async () => {
		const resolution_message = draft.trim();
		if (resolution_message === "" || !fixValid) return;
		try {
			await action.call({
				known_issue_id: issue.id,
				fix_version: fixVersion.trim(),
				resolution_message,
			});
			setDraft("");
			setFixVersion(defaultFix);
			onResolved();
		} catch {
			/* surfaced via action.error */
		}
	};
	const cancel = () => {
		setDraft("");
		setFixVersion(defaultFix);
		onClose();
	};
	return (
		<Dialog open={open} onClose={cancel} fullWidth maxWidth="sm">
			<DialogTitle>Resolve known issue</DialogTitle>
			<DialogContent>
				<TextField
					fullWidth
					label="Fix version"
					value={fixVersion}
					onChange={(e) => setFixVersion(e.target.value)}
					disabled={action.pending}
					error={fixVersion !== "" && !fixValid}
					helperText={
						parsed == null
							? `Semver of the first unaffected patch (e.g. ${defaultFix}).`
							: !sameMinor
								? `Must be in the same minor as the issue (${issue.min_major}.${issue.min_minor}.x).`
								: !aboveMin
									? `Must be above the affected patch ${issue.min_major}.${issue.min_minor}.${minPatch}.`
									: `Issue will cover ${issue.min_major}.${issue.min_minor}.${minPatch}–${parsed.major}.${parsed.minor}.${parsed.patch - 1}.`
					}
					slotProps={{ input: { sx: { fontFamily: "monospace" } } }}
					sx={{ mt: 1 }}
				/>
				<TextField
					fullWidth
					multiline
					minRows={3}
					value={draft}
					onChange={(e) => setDraft(e.target.value)}
					disabled={action.pending}
					placeholder="How was this resolved? (e.g. fixed in 1.0.1, workaround documented)"
					sx={{ mt: 2 }}
				/>
				{action.error && (
					<Alert severity="error" sx={{ mt: 1 }}>
						{action.error.message}
					</Alert>
				)}
			</DialogContent>
			<DialogActions>
				<Button onClick={cancel} disabled={action.pending}>
					Cancel
				</Button>
				<Button
					variant="contained"
					onClick={submit}
					disabled={action.pending || draft.trim() === "" || !fixValid}
				>
					{action.pending ? "Resolving…" : "Resolve"}
				</Button>
			</DialogActions>
		</Dialog>
	);
}

function parseSemver(
	s: string,
): { major: number; minor: number; patch: number } | null {
	const m = s.trim().match(/^(\d+)\.(\d+)\.(\d+)$/);
	if (!m) return null;
	return {
		major: Number(m[1]),
		minor: Number(m[2]),
		patch: Number(m[3]),
	};
}
