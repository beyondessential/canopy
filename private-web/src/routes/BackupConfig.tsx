import {
	Alert,
	Button,
	LinearProgress,
	Link as MuiLink,
	Paper,
	Stack,
	Step,
	StepLabel,
	Stepper,
	TextField,
	ToggleButton,
	ToggleButtonGroup,
	Typography,
} from "@mui/material";
import { useState } from "react";
import { Link as RouterLink, useNavigate, useParams } from "react-router-dom";
import { useApi, useApiAction } from "../api";
import { GradedAction } from "../components/GradedAction";
import { usePageTitle } from "../hooks/usePageTitle";
import { type BackupConfigView, type BackupRepoMode } from "../types";

/// Most buckets live here, so default the region to it.
const DEFAULT_REGION = "ap-southeast-2";

/// Probe response shape (wire type is generated; this is the UI-facing subset).
type ProbeResult = {
	state: "empty" | "kopia_repo" | "other_content" | "inaccessible";
	error: string | null;
	object_sample: string[];
	already_configured: string | null;
};

/// Onboarding (create) / edit form for a group's backup config. Create is a
/// probe-driven wizard: enter the bucket + roles, Canopy inspects it, and the
/// repo mode is derived from what's there (empty → from-birth; existing kopia
/// repo → import by passphrase; other content / inaccessible → blocked). Edit is
/// a flat form (structural fields are create-only). Schedule + retention are NOT
/// set here — they're per-`(group, type)`, inherit the canopy-wide type defaults,
/// and are tuned per type on the group's backup page.
export default function BackupConfig() {
	const { id = "" } = useParams<{ id: string }>();
	const existing = useApi("backups", "get", { server_group_id: id }, [id]);
	usePageTitle("Backup config");

	if (existing.status === "loading" || existing.status === "idle") {
		return <LinearProgress />;
	}
	if (existing.status === "error") {
		return <Alert severity="error">{existing.error.message}</Alert>;
	}
	return <ConfigForm groupId={id} existing={existing.data} />;
}

function ConfigForm({
	groupId,
	existing,
}: {
	groupId: string;
	existing: BackupConfigView | null;
}) {
	const navigate = useNavigate();
	const isCreate = existing == null;
	const create = useApiAction("backups", "create");
	const createShared = useApiAction("backups", "create_shared");
	const update = useApiAction("backups", "update");
	const createRepo = useApiAction("backups", "create_repo");
	const probeAction = useApiAction("backups", "probe");

	// Create-mode placement choice. Defaults to shared-account backups (canopy
	// provisions the bucket; no AWS account needed) — the dedicated-account path is
	// most often driven by pulumi's config API, not this UI.
	const [placement, setPlacement] = useState<"external" | "shared">("shared");

	const [bucket, setBucket] = useState(existing?.bucket ?? "");
	const [prefix, setPrefix] = useState(existing?.prefix ?? "");
	const [roleArn, setRoleArn] = useState(existing?.target_role_arn ?? "");
	const [maintenanceRoleArn, setMaintenanceRoleArn] = useState(
		existing?.maintenance_role_arn ?? "",
	);
	const [region, setRegion] = useState(existing?.region ?? DEFAULT_REGION);
	const [passphrase, setPassphrase] = useState("");

	// Wizard state (create only).
	const [step, setStep] = useState(0);
	const [probe, setProbe] = useState<ProbeResult | null>(null);

	// Probe-derived repo mode: an existing kopia repo is imported by passphrase;
	// an empty bucket is created from-birth. (Canopy never lets the operator pick
	// the passphrase for a repo it creates.)
	const mode: BackupRepoMode =
		probe?.state === "kopia_repo" ? "passphrase" : "from_birth";

	const pending = create.pending || update.pending || createRepo.pending;
	const error = create.error || update.error || createRepo.error;

	const runProbe = async () => {
		try {
			const result = (await probeAction.call({
				bucket,
				prefix,
				region: region.trim() === "" ? null : region,
				maintenance_role_arn: maintenanceRoleArn,
				target_role_arn: roleArn,
			})) as ProbeResult;
			setProbe(result);
			setStep(1);
		} catch {
			/* surfaced via probeAction.error */
		}
	};

	const persist = async () => {
		try {
			if (isCreate) {
				await create.call({
					server_group_id: groupId,
					bucket,
					prefix,
					target_role_arn: roleArn,
					maintenance_role_arn: maintenanceRoleArn,
					region: region.trim() === "" ? null : region,
					mode,
					passphrase: mode === "passphrase" ? passphrase : null,
				});
				await createRepo.call({ server_group_id: groupId });
			} else {
				await update.call({
					server_group_id: groupId,
					region: region.trim() === "" ? null : region,
				});
			}
			navigate(`/fleet/groups/${groupId}/backups`);
		} catch {
			/* surfaced via the action errors */
		}
	};

	// Shared-account onboarding: canopy auto-names + creates the bucket and uses
	// the shared roles, so there's no bucket/roles to supply and no probe.
	const persistShared = async () => {
		try {
			await createShared.call({
				server_group_id: groupId,
				region: region.trim() === "" ? null : region,
			});
			navigate(`/fleet/groups/${groupId}/backups`);
		} catch {
			/* surfaced via createShared.error */
		}
	};

	// ── Edit mode: flat form (structural fields are create-only) ───────────────
	if (!isCreate) {
		return (
			<Paper variant="outlined" sx={{ p: 3 }}>
				<Stack spacing={2}>
					<Typography variant="h5" component="h1">
						Edit backup config
					</Typography>
					<TextField label="Bucket" value={bucket} disabled />
					<TextField label="Target role ARN" value={roleArn} disabled />
					<TextField
						label="Maintenance role ARN"
						value={maintenanceRoleArn}
						disabled
					/>
					<TextField
						label="Region"
						value={region}
						onChange={(e) => setRegion(e.target.value)}
						disabled={pending}
						helperText="Changing region typically implies a different bucket — change with care."
					/>
					<Typography variant="caption" color="text.secondary">
						Schedule and retention are managed per backup type on the group's
						backup page.
					</Typography>
					{error && <Alert severity="error">{error.message}</Alert>}
					<Stack direction="row" spacing={1}>
						<GradedAction calls="backups/update">
							<Button variant="contained" onClick={persist} disabled={pending}>
								{pending ? "Saving…" : "Save"}
							</Button>
						</GradedAction>
						<Button
							variant="outlined"
							color="error"
							onClick={() => navigate(`/fleet/groups/${groupId}/backups`)}
							disabled={pending}
						>
							Cancel
						</Button>
					</Stack>
				</Stack>
			</Paper>
		);
	}

	// Placement choice (create mode): BYO AWS account vs shared-account backups.
	const placementToggle = (
		<ToggleButtonGroup
			exclusive
			color="primary"
			size="small"
			value={placement}
			onChange={(_, v) => v && setPlacement(v)}
			sx={{ alignSelf: "flex-start" }}
		>
			<ToggleButton value="shared">Create a bucket</ToggleButton>
			<ToggleButton value="external">Use an existing bucket</ToggleButton>
		</ToggleButtonGroup>
	);

	// Shared-account placement: canopy provisions + manages the bucket, so there's
	// no bucket/roles to supply and no probe — just confirm (optionally set region).
	if (placement === "shared") {
		return (
			<Paper variant="outlined" sx={{ p: 3 }}>
				<Stack spacing={3}>
					<Typography variant="h5" component="h1">
						Set up backups
					</Typography>
					{placementToggle}
					<Alert severity="info">
						No AWS account needed — Canopy creates and manages a bucket for this
						group in the shared backups account and rotates its passphrase.
						Schedule &amp; retention inherit the per-type defaults.
					</Alert>
					<TextField
						label="Region"
						value={region}
						onChange={(e) => setRegion(e.target.value)}
						disabled={createShared.pending}
						helperText="Defaults to ap-southeast-2."
					/>
					{createShared.error && (
						<Alert severity="error">{createShared.error.message}</Alert>
					)}
					<Stack direction="row" spacing={1}>
						<GradedAction calls="backups/create_shared">
							<Button
								variant="contained"
								onClick={persistShared}
								disabled={createShared.pending}
							>
								{createShared.pending ? "Creating…" : "Create & provision"}
							</Button>
						</GradedAction>
						<Button
							variant="outlined"
							color="error"
							onClick={() => navigate(`/fleet/groups/${groupId}/backups`)}
							disabled={createShared.pending}
						>
							Cancel
						</Button>
					</Stack>
				</Stack>
			</Paper>
		);
	}

	// ── Create mode: probe-driven wizard ───────────────────────────────────────
	const canCheck =
		bucket.trim() !== "" &&
		roleArn.trim() !== "" &&
		maintenanceRoleArn.trim() !== "";
	const canProvision =
		probe != null &&
		probe.already_configured == null &&
		(probe.state === "empty" ||
			(probe.state === "kopia_repo" && passphrase.trim() !== ""));

	return (
		<Paper variant="outlined" sx={{ p: 3 }}>
			<Stack spacing={3}>
				<Typography variant="h5" component="h1">
					Set up backups
				</Typography>
				{placementToggle}
				<Stepper activeStep={step}>
					<Step>
						<StepLabel>Target bucket</StepLabel>
					</Step>
					<Step>
						<StepLabel>Review &amp; create</StepLabel>
					</Step>
				</Stepper>

				{step === 0 && (
					<Stack spacing={2}>
						<TextField
							label="Bucket"
							value={bucket}
							onChange={(e) => setBucket(e.target.value)}
							disabled={probeAction.pending}
							required
							helperText="S3 bucket holding this group's kopia repository."
						/>
						<TextField
							label="Prefix"
							value={prefix}
							onChange={(e) => setPrefix(e.target.value)}
							disabled={probeAction.pending}
							helperText="Optional key prefix within the bucket."
						/>
						<TextField
							label="Region"
							value={region}
							onChange={(e) => setRegion(e.target.value)}
							disabled={probeAction.pending}
							helperText="Defaults to ap-southeast-2."
						/>
						<TextField
							label="Target role ARN"
							value={roleArn}
							onChange={(e) => setRoleArn(e.target.value)}
							disabled={probeAction.pending}
							required
							helperText="IAM role Canopy assumes to mint device credentials (no delete)."
						/>
						<TextField
							label="Maintenance role ARN"
							value={maintenanceRoleArn}
							onChange={(e) => setMaintenanceRoleArn(e.target.value)}
							disabled={probeAction.pending}
							required
							helperText="IAM role for maintenance + this check (s3:* + delete + CloudWatch)."
						/>
						{probeAction.error && (
							<Alert severity="error">{probeAction.error.message}</Alert>
						)}
						<Stack direction="row" spacing={1}>
							<Button
								variant="contained"
								onClick={runProbe}
								disabled={!canCheck || probeAction.pending}
							>
								{probeAction.pending ? "Checking…" : "Check bucket"}
							</Button>
							<Button
								variant="outlined"
								color="error"
								onClick={() => navigate(`/fleet/groups/${groupId}/backups`)}
								disabled={probeAction.pending}
							>
								Cancel
							</Button>
						</Stack>
					</Stack>
				)}

				{step === 1 && probe && (
					<Stack spacing={2}>
						<ProbeReview
							probe={probe}
							groupId={groupId}
							passphrase={passphrase}
							setPassphrase={setPassphrase}
						/>
						{canProvision && (
							<Typography variant="caption" color="text.secondary">
								Schedule &amp; retention inherit the canopy-wide per-type
								defaults — tune them per type on the group's backup page after
								setup.
							</Typography>
						)}
						{error && <Alert severity="error">{error.message}</Alert>}
						<Stack direction="row" spacing={1}>
							<Button variant="outlined" onClick={() => setStep(0)}>
								Back
							</Button>
							{(probe.state === "other_content" ||
								probe.state === "inaccessible") && (
								<Button variant="contained" onClick={runProbe}>
									Re-check
								</Button>
							)}
							<GradedAction calls={["backups/create", "backups/create_repo"]}>
								<Button
									variant="contained"
									onClick={persist}
									disabled={!canProvision || pending}
								>
									{pending ? "Creating…" : "Create & provision"}
								</Button>
							</GradedAction>
						</Stack>
					</Stack>
				)}
			</Stack>
		</Paper>
	);
}

/// Review step: explain what the probe found and gate provisioning.
function ProbeReview({
	probe,
	groupId,
	passphrase,
	setPassphrase,
}: {
	probe: ProbeResult;
	groupId: string;
	passphrase: string;
	setPassphrase: (v: string) => void;
}) {
	if (
		probe.already_configured != null &&
		probe.already_configured !== groupId
	) {
		return (
			<Alert severity="error">
				This bucket + prefix is already configured for another group.{" "}
				<MuiLink
					component={RouterLink}
					to={`/fleet/groups/${probe.already_configured}/backups`}
				>
					View that config
				</MuiLink>
				. Use a different prefix or bucket.
			</Alert>
		);
	}
	switch (probe.state) {
		case "inaccessible":
			return (
				<Alert severity="error">
					Couldn't access the bucket: {probe.error ?? "unknown error"}. Check
					the role ARN, bucket, and region, then re-check.
				</Alert>
			);
		case "other_content":
			return (
				<Alert severity="warning">
					The bucket/prefix already holds other (non-kopia) content
					{probe.object_sample.length > 0 && (
						<> — e.g. {probe.object_sample.join(", ")}</>
					)}
					. Canopy won't write over it: clear the prefix, or use a different
					prefix/bucket, then re-check.
				</Alert>
			);
		case "empty":
			return (
				<Alert severity="info">
					Empty bucket. Canopy will create a new repository and generate its
					passphrase (from-birth), then rotate it regularly.
				</Alert>
			);
		case "kopia_repo":
			return (
				<Stack spacing={2}>
					<Alert severity="info">
						An existing kopia repository is here. Supply its passphrase to
						import it; Canopy verifies it at init and then rotates it to a
						Canopy-owned passphrase (a hard break for any existing consumers).
					</Alert>
					<TextField
						label="Existing repository passphrase"
						type="password"
						value={passphrase}
						onChange={(e) => setPassphrase(e.target.value)}
						required
					/>
				</Stack>
			);
	}
}
