import UploadFileIcon from "@mui/icons-material/UploadFile";
import {
	Alert,
	Box,
	Button,
	Checkbox,
	Dialog,
	DialogActions,
	DialogContent,
	DialogTitle,
	LinearProgress,
	MenuItem,
	Table,
	TableBody,
	TableCell,
	TableHead,
	TableRow,
	TextField,
	Typography,
} from "@mui/material";
import { useEffect, useRef, useState } from "react";
import { useApiAction } from "../api";
import type { ImportPreview, ImportVariable, ServerRank } from "../types";
import { GradedAction } from "./GradedAction";

const SKIP = "skip";

type Choices = {
	/** Host key to a machine identifier, or null to skip the host. */
	hosts: Record<string, string | null>;
	/** `host\0name` (host empty for the environment) to whether it is secret. */
	secrets: Record<string, boolean>;
};

const markKey = (host: string | null, name: string) => `${host ?? ""}\0${name}`;

/// Import an Ansible inventory file into an environment that carries no
/// variables: choose the file, review what would be set and where, and apply.
// spec: ANSIMP#preview
export default function AnsibleImportDialog({
	open,
	onClose,
	groupId,
	rank,
	onImported,
}: {
	open: boolean;
	onClose: () => void;
	groupId: string;
	rank: ServerRank;
	onImported: () => void;
}) {
	const [fileName, setFileName] = useState<string | null>(null);
	const [file, setFile] = useState<string | null>(null);
	const [choices, setChoices] = useState<Choices>({ hosts: {}, secrets: {} });
	const [notes, setNotes] = useState<string | null>(null);
	const [preview, setPreview] = useState<ImportPreview | null>(null);
	const picker = useRef<HTMLInputElement>(null);
	const previewing = useApiAction("ansible", "import_preview");
	const applying = useApiAction("ansible", "import_apply");

	// Everything but the notes is worked out by the server from the file and the
	// operator's choices, so a change to either asks again.
	// biome-ignore lint/correctness/useExhaustiveDependencies: `call` is not stable.
	useEffect(() => {
		if (!open || file === null) return;
		let current = true;
		previewing
			.call(request(groupId, rank, file, choices))
			.then((result) => {
				if (current) setPreview(result as ImportPreview);
			})
			.catch(() => {
				if (current) setPreview(null);
			});
		return () => {
			current = false;
		};
	}, [open, file, choices, groupId, rank]);

	const reset = () => {
		setFileName(null);
		setFile(null);
		setChoices({ hosts: {}, secrets: {} });
		setNotes(null);
		setPreview(null);
		previewing.reset();
		applying.reset();
	};
	const close = () => {
		reset();
		onClose();
	};

	const choose = async (chosen: File | undefined) => {
		if (!chosen) return;
		setFileName(chosen.name);
		setChoices({ hosts: {}, secrets: {} });
		setNotes(null);
		setPreview(null);
		setFile(await chosen.text());
	};

	const apply = () => {
		if (file === null || preview === null) return;
		applying
			.call({
				...request(groupId, rank, file, choices),
				members: preview.members,
				...(notes === null ? {} : { notes }),
			})
			.then(() => {
				reset();
				onImported();
				onClose();
			})
			.catch(() => {
				// Surfaced by the alert below.
			});
	};

	const unresolved =
		preview?.hosts.some(
			(host) => host.state === "unmatched" || host.state === "ambiguous",
		) ?? false;
	// Applied only as the preview on screen describes it, so not while the next
	// one is being worked out.
	const blocked =
		preview === null || !preview.can_apply || previewing.pending || applying.pending;

	return (
		<Dialog open={open} onClose={close} fullWidth maxWidth="md">
			<DialogTitle>
				{fileName ? `Import ${fileName} into ${rank}` : `Import into ${rank}`}
			</DialogTitle>
			<DialogContent
				sx={{ display: "flex", flexDirection: "column", gap: 2 }}
				data-testid="import-dialog"
			>
				<input
					ref={picker}
					type="file"
					accept=".yml,.yaml,text/yaml"
					hidden
					data-testid="import-file"
					onChange={(event) => {
						void choose(event.target.files?.[0]);
						event.target.value = "";
					}}
				/>
				{file === null && (
					<Box>
						<Button
							variant="outlined"
							startIcon={<UploadFileIcon />}
							onClick={() => picker.current?.click()}
						>
							Choose an inventory file
						</Button>
					</Box>
				)}
				{previewing.pending && <LinearProgress />}
				{previewing.error && (
					<Alert severity="error" data-testid="import-error">
						{previewing.error.message}
					</Alert>
				)}
				{applying.error && (
					<Alert severity="error">{applying.error.message}</Alert>
				)}
				{preview && (
					<Review
						preview={preview}
						choices={choices}
						onChoices={setChoices}
						notes={notes ?? preview.notes}
						onNotes={setNotes}
					/>
				)}
			</DialogContent>
			<DialogActions>
				{preview && !preview.can_apply && (
					<Typography
						variant="caption"
						color="text.secondary"
						sx={{ mr: "auto", pl: 2 }}
					>
						{unresolved
							? "Assign or skip every host to import"
							: (preview.problems[0] ?? "Fix what is marked to import")}
					</Typography>
				)}
				<Button onClick={close}>Cancel</Button>
				<GradedAction
					calls="ansible/import_apply"
					action="Import Ansible inventory"
				>
					<Button
						variant="contained"
						disabled={blocked}
						onClick={apply}
						data-testid="import-apply"
					>
						Import
					</Button>
				</GradedAction>
			</DialogActions>
		</Dialog>
	);
}

function request(
	groupId: string,
	rank: ServerRank,
	file: string,
	choices: Choices,
) {
	return {
		server_group_id: groupId,
		rank,
		file,
		hosts: Object.entries(choices.hosts).map(([host, machine_id]) => ({
			host,
			machine_id,
		})),
		secrets: Object.entries(choices.secrets).map(([key, secret]) => {
			const [host, name] = key.split("\0");
			return { host: host === "" ? null : host, name, secret };
		}),
	};
}

function Review({
	preview,
	choices,
	onChoices,
	notes,
	onNotes,
}: {
	preview: ImportPreview;
	choices: Choices;
	onChoices: (choices: Choices) => void;
	notes: string;
	onNotes: (notes: string) => void;
}) {
	const chooseHost = (host: string, value: string) =>
		onChoices({
			...choices,
			hosts: { ...choices.hosts, [host]: value === SKIP ? null : value },
		});
	const mark = (host: string | null, name: string, secret: boolean) =>
		onChoices({
			...choices,
			secrets: { ...choices.secrets, [markKey(host, name)]: secret },
		});

	return (
		<>
			{preview.clone_mismatch && (
				<Alert severity="warning" data-testid="clone-mismatch">
					{preview.clone_mismatch.file ? (
						<>
							The file sets <code>tamanu_clone</code>, but {preview.rank} is not
							a clone.
						</>
					) : (
						<>
							The file does not set <code>tamanu_clone</code>, but {preview.rank}{" "}
							is a clone.
						</>
					)}
				</Alert>
			)}
			{preview.left_out.length > 0 && (
				<Typography variant="body2" color="text.secondary">
					Left out, since a run sets{" "}
					{preview.left_out.length === 1 ? "it" : "them"} for itself:{" "}
					<code>{preview.left_out.join(", ")}</code>
				</Typography>
			)}
			{preview.overrides.length > 0 && (
				<Typography variant="body2" color="text.secondary">
					Overrides the group's <code>{preview.overrides.join(", ")}</code>
				</Typography>
			)}
			{!preview.catalogue && (
				<Typography variant="body2" color="text.secondary">
					No catalogue has been read, so nothing here is checked against one.
				</Typography>
			)}

			<Box>
				<Typography variant="subtitle2" gutterBottom>
					Hosts
				</Typography>
				<Table size="small" data-testid="import-hosts">
					<TableHead>
						<TableRow>
							<TableCell>In the file</TableCell>
							<TableCell>Machine</TableCell>
						</TableRow>
					</TableHead>
					<TableBody>
						{preview.hosts.map((host) => {
							const open =
								host.state === "unmatched" || host.state === "ambiguous";
							return (
								<TableRow
									key={host.key}
									data-testid={`import-host-${host.key}`}
									sx={host.state === "skipped" ? { opacity: 0.6 } : undefined}
								>
									<TableCell sx={{ fontFamily: "monospace" }}>
										{host.key}
									</TableCell>
									<TableCell>
										<TextField
											select
											size="small"
											label="Machine"
											error={open}
											value={
												host.state === "skipped"
													? SKIP
													: (host.machine_id ?? "")
											}
											onChange={(event) =>
												chooseHost(host.key, event.target.value)
											}
											slotProps={{
												select: {
													displayEmpty: true,
													renderValue: (value) =>
														value === ""
															? "Assign or skip"
															: value === SKIP
																? "Skip this host"
																: preview.machines.find(
																		(machine) => machine.id === value,
																	)?.name,
												},
												inputLabel: { shrink: true },
											}}
											sx={{ minWidth: 280 }}
										>
											<MenuItem value={SKIP}>Skip this host</MenuItem>
											{preview.machines.map((machine) => (
												<MenuItem key={machine.id} value={machine.id}>
													{machine.name}
												</MenuItem>
											))}
										</TextField>
									</TableCell>
								</TableRow>
							);
						})}
					</TableBody>
				</Table>
			</Box>

			<Box>
				<Typography variant="subtitle2" gutterBottom>
					Variables
				</Typography>
				<Table size="small" data-testid="import-variables">
					<TableHead>
						<TableRow>
							<TableCell>Name</TableCell>
							<TableCell>Value</TableCell>
							<TableCell align="center">Secret</TableCell>
						</TableRow>
					</TableHead>
					<TableBody>
						<VariableGroup
							title="This environment"
							host={null}
							variables={preview.environment}
							onMark={mark}
						/>
						{preview.hosts
							.filter((host) => host.state !== "skipped")
							.map((host) => (
								<VariableGroup
									key={host.key}
									title={
										host.machine_id
											? (preview.machines.find(
													(machine) => machine.id === host.machine_id,
												)?.name ?? host.key)
											: `${host.key}, not yet assigned`
									}
									host={host.key}
									variables={host.variables}
									onMark={mark}
								/>
							))}
					</TableBody>
				</Table>
			</Box>

			<TextField
				label="Added to the group's notes"
				multiline
				minRows={3}
				value={notes}
				onChange={(event) => onNotes(event.target.value)}
				slotProps={{ htmlInput: { "data-testid": "import-notes" } }}
			/>
		</>
	);
}

function VariableGroup({
	title,
	host,
	variables,
	onMark,
}: {
	title: string;
	host: string | null;
	variables: ReadonlyArray<ImportVariable>;
	onMark: (host: string | null, name: string, secret: boolean) => void;
}) {
	if (variables.length === 0) return null;
	return (
		<>
			<TableRow>
				<TableCell
					colSpan={3}
					sx={{ fontWeight: 500, bgcolor: "action.hover" }}
				>
					{title}
				</TableCell>
			</TableRow>
			{variables.map((variable) => (
				<TableRow key={variable.name} data-testid={`import-var-${variable.name}`}>
					<TableCell sx={{ fontFamily: "monospace", verticalAlign: "top" }}>
						{variable.name}
					</TableCell>
					<TableCell sx={{ fontFamily: "monospace" }}>
						{variable.secret ? "••••••••" : JSON.stringify(variable.value)}
						{variable.origin && (
							<Typography
								component="span"
								variant="caption"
								color="text.secondary"
								sx={{ ml: 1, fontFamily: "inherit" }}
							>
								{variable.origin === "added" ? "added" : "from host key"}
							</Typography>
						)}
						{variable.problems.map((problem) => (
							<Typography
								key={problem}
								variant="caption"
								color="error"
								sx={{ display: "block" }}
							>
								{problem}
							</Typography>
						))}
					</TableCell>
					<TableCell align="center" sx={{ verticalAlign: "top" }}>
						<Checkbox
							size="small"
							checked={variable.secret}
							disabled={variable.locked}
							onChange={(event) =>
								onMark(host, variable.name, event.target.checked)
							}
							slotProps={{
								input: { "aria-label": `${variable.name} is secret` },
							}}
						/>
					</TableCell>
				</TableRow>
			))}
		</>
	);
}
