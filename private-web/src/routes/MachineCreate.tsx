import {
	Alert,
	Button,
	Checkbox,
	FormControlLabel,
	MenuItem,
	Paper,
	Stack,
	TextField,
	Typography,
} from "@mui/material";
import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { callApi, useApiAction } from "../api";
import { GradedAction } from "../components/GradedAction";
import GroupControl from "../components/GroupControl";
import TagsEditor from "../components/TagsEditor";
import { usePageTitle } from "../hooks/usePageTitle";
import { REACHABILITY_CHECK } from "../types";
import type { TagMap, TailnetLiveInfo } from "../types";

/// Operator-first machine creation, reachable at `/fleet/groups/:id/machines/new`
/// with the group preselected. A group is required — machines are always
/// grouped.
///
/// A machine is what an operator adds; the applications on it arrive by report
/// and take its group. So there is nothing here about an application's type,
/// rank, URL or public name.
/// spec: APP#where-a-type-comes-from
export default function MachineCreate() {
	usePageTitle("Add machine");
	const navigate = useNavigate();
	// When mounted under `/fleet/groups/:id/machines/new`, the route param is the
	// group to default-select.
	const { id: presetGroupId } = useParams<{ id?: string }>();
	const action = useApiAction("fleet/machines", "create");
	const silence = useApiAction("silenced_refs", "silence_machine");

	const [name, setName] = useState("");
	const [isMonitored, setIsMonitored] = useState(true);
	// Off means "alert on everything else, just not this box going away".
	// Written as the machine-scoped silence on canopy's reachability check —
	// the same one the check itself offers, so either place reflects the other.
	// spec: CHK#silences-follow-the-event
	const [alertWhenUnreachable, setAlertWhenUnreachable] = useState(true);
	const [alertWhenDownMinutes, setAlertWhenDownMinutes] = useState("10");
	const [groupId, setGroupId] = useState<string | null>(presetGroupId ?? null);
	const [tailscaleIdentifier, setTailscaleIdentifier] = useState("");
	const [cloud, setCloud] = useState<"" | "true" | "false">("");
	const [lat, setLat] = useState("");
	const [lon, setLon] = useState("");
	const [notes, setNotes] = useState("");
	const [tags, setTags] = useState<TagMap>({});

	const pending = action.pending || silence.pending;
	const error = action.error ?? silence.error;

	const onSubmit = async (e: React.FormEvent) => {
		e.preventDefault();
		if (!groupId || !name.trim()) return; // name and group are required
		const data: Record<string, unknown> = {
			name: name.trim(),
			group_id: groupId,
			tailscale_identifier:
				tailscaleIdentifier.trim() === "" ? null : tailscaleIdentifier.trim(),
			cloud: cloud === "" ? null : cloud === "true",
			geolocation:
				lat && lon ? { lat: Number(lat), lon: Number(lon) } : null,
			is_monitored: isMonitored,
			alert_when_down_for: Math.max(
				60,
				Math.round(Number(alertWhenDownMinutes) * 60),
			),
			notes,
			tags,
		};
		try {
			const machineId = await action.call(data);
			if (!alertWhenUnreachable) {
				await silence.call({
					machine_id: machineId,
					source: REACHABILITY_CHECK.source,
					ref: REACHABILITY_CHECK.ref,
				});
			}
			navigate(`/fleet/machines/${machineId}`);
		} catch {
			/* surfaced via the actions' errors */
		}
	};

	return (
		<Paper variant="outlined" sx={{ p: 3 }} component="form" onSubmit={onSubmit}>
			<Stack spacing={2}>
				<Typography variant="h5" component="h1">
					Add machine
				</Typography>

				<TextField
					label="Name"
					value={name}
					onChange={(e) => setName(e.target.value)}
					disabled={pending}
					required
				/>
				<TailscaleIdentityField
					value={tailscaleIdentifier}
					onChange={setTailscaleIdentifier}
					disabled={pending}
				/>

				<GroupControl
					currentGroupId={groupId}
					onChange={setGroupId}
					disabled={pending}
					required
				/>

				<Stack direction={{ xs: "column", md: "row" }} spacing={2}>
					<TextField
						select
						label="Location"
						value={cloud}
						onChange={(e) => setCloud(e.target.value as "" | "true" | "false")}
						disabled={pending}
						sx={{ minWidth: 180 }}
					>
						<MenuItem value="">unknown</MenuItem>
						<MenuItem value="true">cloud</MenuItem>
						<MenuItem value="false">on premise</MenuItem>
					</TextField>
					<TextField
						label="Latitude"
						value={lat}
						onChange={(e) => setLat(e.target.value)}
						disabled={pending}
						sx={{ flex: 1 }}
					/>
					<TextField
						label="Longitude"
						value={lon}
						onChange={(e) => setLon(e.target.value)}
						disabled={pending}
						sx={{ flex: 1 }}
					/>
				</Stack>

				<FormControlLabel
					control={
						<Checkbox
							checked={isMonitored}
							onChange={(e) => setIsMonitored(e.target.checked)}
							disabled={pending}
						/>
					}
					label="Monitor this machine"
				/>
				<Typography variant="caption" color="text.secondary">
					When off, no check on this machine alerts: its checks are still
					determined and shown, and its health and reachability are marked
					as unmonitored wherever they appear, but nothing triggers or joins
					an incident. The applications on it are unaffected. Use this for
					test environments and ad-hoc demos that are expected to be down.
				</Typography>

				<FormControlLabel
					control={
						<Checkbox
							checked={alertWhenUnreachable}
							onChange={(e) => setAlertWhenUnreachable(e.target.checked)}
							disabled={pending}
						/>
					}
					label="Alert when this machine is unreachable"
				/>
				<Typography variant="caption" color="text.secondary">
					When off, every other check on this box alerts as normal and only the
					box going away is quiet. This is the same as silencing the
					reachability check on this machine, and either place reflects the
					other.
				</Typography>

				<Stack
					direction={{ xs: "column", md: "row" }}
					spacing={2}
					sx={{ alignItems: { md: "center" } }}
				>
					<Typography variant="body2">
						File an issue when this machine is unreachable for
					</Typography>
					<TextField
						label="minutes"
						type="number"
						value={alertWhenDownMinutes}
						onChange={(e) => setAlertWhenDownMinutes(e.target.value)}
						disabled={pending || !isMonitored || !alertWhenUnreachable}
						slotProps={{ htmlInput: { min: 1, step: 1 } }}
						sx={{ width: 140 }}
					/>
				</Stack>

				<TextField
					label="Notes"
					multiline
					minRows={3}
					value={notes}
					onChange={(e) => setNotes(e.target.value)}
					disabled={pending}
					helperText="Operator notes shown on the machine's page. Plain text."
				/>

				<Stack spacing={1}>
					<Typography variant="subtitle1">Tags</Typography>
					<TagsEditor value={tags} onChange={setTags} disabled={pending} />
				</Stack>

				{error && <Alert severity="error">{error.message}</Alert>}

				<Stack direction="row" spacing={1}>
					<GradedAction
						calls={
							alertWhenUnreachable
								? "fleet/machines/create"
								: ["fleet/machines/create", "silenced_refs/silence_machine"]
						}
					>
						<Button
							type="submit"
							variant="contained"
							disabled={pending || !groupId || !name.trim()}
						>
							{pending ? "Creating…" : "Create machine"}
						</Button>
					</GradedAction>
					<Button
						type="button"
						variant="outlined"
						color="error"
						onClick={() => navigate(-1)}
						disabled={pending}
					>
						Cancel
					</Button>
				</Stack>
			</Stack>
		</Paper>
	);
}

/// Optional Tailscale identity field with a debounced live preview from
/// `devices.resolve_tailnet_identifier`. The raw value is passed to
/// `machines.create` as `tailscale_identifier`.
function TailscaleIdentityField({
	value,
	onChange,
	disabled,
}: {
	value: string;
	onChange: (value: string) => void;
	disabled: boolean;
}) {
	const [preview, setPreview] = useState<TailnetLiveInfo | null>(null);
	const [previewError, setPreviewError] = useState<string | null>(null);
	const [previewLoading, setPreviewLoading] = useState(false);

	useEffect(() => {
		const trimmed = value.trim();
		if (trimmed === "") {
			setPreview(null);
			setPreviewError(null);
			return;
		}
		let cancelled = false;
		setPreviewLoading(true);
		const handle = setTimeout(async () => {
			try {
				const r = await callApi("devices", "resolve_tailnet_identifier", {
					identifier: trimmed,
				});
				if (cancelled) return;
				setPreview(r.matched);
				setPreviewError(
					r.matched ? null : "No tailnet node matches that identifier.",
				);
			} catch (err) {
				if (cancelled) return;
				setPreview(null);
				setPreviewError(err instanceof Error ? err.message : String(err));
			} finally {
				if (!cancelled) setPreviewLoading(false);
			}
		}, 250);
		return () => {
			cancelled = true;
			clearTimeout(handle);
		};
	}, [value]);

	return (
		<Stack spacing={1}>
			<TextField
				label="Tailscale identity"
				placeholder="100.64.0.42 / nodekey:… / device.example.ts.net"
				value={value}
				onChange={(e) => onChange(e.target.value)}
				disabled={disabled}
				helperText="Bind this server to a tailnet node up front. Leave empty to enroll it later."
			/>
			{previewLoading && (
				<Typography variant="caption" color="text.secondary">
					Resolving…
				</Typography>
			)}
			{preview && (
				<Paper variant="outlined" sx={{ p: 1.5 }}>
					<Stack spacing={0.5}>
						<Typography variant="caption" color="text.secondary">
							Resolves to
						</Typography>
						<Typography variant="body2">{preview.display_name}</Typography>
						<Typography
							variant="body2"
							color="text.secondary"
							sx={{ fontFamily: "monospace" }}
						>
							{preview.node_id}
						</Typography>
						<Typography variant="body2" color="text.secondary">
							{preview.addresses.join(", ")}
						</Typography>
					</Stack>
				</Paper>
			)}
			{previewError && value.trim() !== "" && (
				<Alert severity="info">{previewError}</Alert>
			)}
		</Stack>
	);
}

