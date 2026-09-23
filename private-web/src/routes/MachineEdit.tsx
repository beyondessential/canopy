import {
	Alert,
	Button,
	Checkbox,
	FormControlLabel,
	LinearProgress,
	MenuItem,
	Paper,
	Stack,
	TextField,
	Typography,
} from "@mui/material";
import { useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { callApi, useApi } from "../api";
import ApplicationTypeChip from "../components/ApplicationTypeChip";
import GroupControl from "../components/GroupControl";
import NameManagementGrants from "../components/NameManagementGrants";
import { GradedAction } from "../components/GradedAction";
import TagsEditor from "../components/TagsEditor";
import { useApplicationTypeCaps } from "../hooks/useApplicationTypes";
import { usePageTitle } from "../hooks/usePageTitle";
import type { GradedEndpoint } from "../safety-modes";
import {
	applicationName,
	REACHABILITY_CHECK,
	type ServerInfo,
	type ServerRank,
	type TagMap,
} from "../types";

const RANK_OPTIONS: Array<{ value: ServerRank | ""; label: string }> = [
	{ value: "", label: "(none)" },
	{ value: "production", label: "production" },
	{ value: "clone", label: "clone" },
	{ value: "demo", label: "demo" },
	{ value: "test", label: "test" },
	{ value: "dev", label: "dev" },
];

/// Editing is machine-first: one form per machine, holding the box's own
/// section and one section per application on it.
///
/// Reading is two pages because the grains hold different material, but a
/// machine fact has one place to be edited and a shared box is edited where
/// everything sharing it is visible — so a change to the box is visibly a
/// change to all of them. An application's group is not offered at all: it is
/// the machine's, and the workloads take it.
/// spec: FLT#groups
export default function MachineEdit() {
	const { id = "" } = useParams<{ id: string }>();
	const detail = useApi("fleet/machines", "get_detail", { machine_id: id }, [id]);
	// The box's switch and each workload's are the same kind of thing — a
	// scoped silence on canopy's reachability check — so both are read before
	// the form paints rather than settling in afterwards.
	const machineSilences = useApi(
		"silenced_refs",
		"list_for_machine",
		{ machine_id: id },
		[id],
	);
	const applicationIds =
		detail.status === "ok" ? detail.data.applications.map((a) => a.id) : [];
	const applicationSilences = useApi(
		"silenced_refs",
		"list_for_servers",
		{ application_ids: applicationIds },
		[applicationIds.join(",")],
		{ skip: detail.status !== "ok" },
	);
	usePageTitle(
		detail.status === "ok"
			? `Edit ${detail.data.machine.name ?? "machine"}`
			: "Edit machine",
	);

	if (
		detail.status === "loading" ||
		detail.status === "idle" ||
		machineSilences.status === "loading" ||
		machineSilences.status === "idle" ||
		applicationSilences.status === "loading" ||
		applicationSilences.status === "idle"
	) {
		return <LinearProgress />;
	}
	if (detail.status === "error") {
		return <Alert severity="error">{detail.error.message}</Alert>;
	}
	if (machineSilences.status === "error") {
		return <Alert severity="error">{machineSilences.error.message}</Alert>;
	}
	if (applicationSilences.status === "error") {
		return <Alert severity="error">{applicationSilences.error.message}</Alert>;
	}

	const silenced = (refs: Array<{ source: string; ref: string }>) =>
		refs.some(
			(s) =>
				s.source === REACHABILITY_CHECK.source &&
				s.ref === REACHABILITY_CHECK.ref,
		);

	return (
		<Form
			machine={detail.data.machine}
			applications={detail.data.applications}
			machineReachabilitySilenced={silenced(machineSilences.data)}
			applicationReachabilitySilenced={new Set(
				applicationSilences.data
					.filter(
						(s) =>
							s.source === REACHABILITY_CHECK.source &&
							s.ref === REACHABILITY_CHECK.ref,
					)
					.map((s) => s.application_id),
			)}
		/>
	);
}

/// The box's own fields.
interface MachineForm {
	name: string;
	groupId: string | null;
	cloud: "" | "true" | "false";
	lat: string;
	lon: string;
	isMonitored: boolean;
	alertWhenUnreachable: boolean;
	alertWhenDownMinutes: string;
	notes: string;
	tags: TagMap;
}

/// One application's fields, as its section holds them.
interface ApplicationForm {
	name: string;
	host: string;
	rank: ServerRank | "";
	publicName: string;
	isMonitored: boolean;
	alertWhenUnreachable: boolean;
	alertWhenDownMinutes: string;
	notes: string;
	tags: TagMap;
	mayManageDns: boolean;
	mayManageTls: boolean;
}

function minutesOf(seconds: number): string {
	return Math.max(1, Math.round(seconds / 60)).toString();
}

function Form({
	machine,
	applications,
	machineReachabilitySilenced,
	applicationReachabilitySilenced,
}: {
	machine: {
		id: string;
		name?: string | null;
		group_id?: string | null;
		cloud?: boolean | null;
		geolocation?: { lat: number; lon: number } | null;
		is_monitored: boolean;
		alert_when_down_for: number;
		notes: string;
		tags: TagMap;
	};
	applications: ServerInfo[];
	machineReachabilitySilenced: boolean;
	applicationReachabilitySilenced: Set<string>;
}) {
	const navigate = useNavigate();
	const [box, setBox] = useState<MachineForm>({
		name: machine.name ?? "",
		groupId: machine.group_id ?? null,
		cloud: machine.cloud == null ? "" : machine.cloud ? "true" : "false",
		lat: machine.geolocation?.lat?.toString() ?? "",
		lon: machine.geolocation?.lon?.toString() ?? "",
		isMonitored: machine.is_monitored,
		alertWhenUnreachable: !machineReachabilitySilenced,
		alertWhenDownMinutes: minutesOf(machine.alert_when_down_for),
		notes: machine.notes ?? "",
		tags: machine.tags ?? {},
	});
	const [apps, setApps] = useState<Record<string, ApplicationForm>>(() =>
		Object.fromEntries(
			applications.map((a) => [
				a.id,
				{
					name: a.name ?? "",
					host: a.host ?? "",
					rank: a.rank ?? "",
					publicName: a.public_name ?? "",
					isMonitored: a.is_monitored,
					alertWhenUnreachable: !applicationReachabilitySilenced.has(a.id),
					alertWhenDownMinutes: minutesOf(a.alert_when_down_for),
					notes: a.notes ?? "",
					tags: a.tags ?? {},
					mayManageDns: a.may_manage_dns,
					mayManageTls: a.may_manage_tls,
				},
			]),
		),
	);
	const [pending, setPending] = useState(false);
	const [error, setError] = useState<string | null>(null);

	const patchApp = (id: string, patch: Partial<ApplicationForm>) =>
		setApps((held) => ({ ...held, [id]: { ...held[id]!, ...patch } }));

	// One save, in one order: the box, then each workload, then the silence
	// changes. Sequential rather than concurrent because a group change on the
	// box propagates onto the applications, and an application write racing
	// that propagation would be writing against a group that is still moving.
	// What saving does, in order, as one list: submitting runs exactly these and
	// the save button is graded on exactly these. Written once, so a change to
	// what save does cannot leave the grade behind.
	const plan: { call: GradedEndpoint; run: () => Promise<unknown> }[] = [
		{
			call: "fleet/machines/update",
			// Flat args, unlike the application's update: every field is
			// `Option<Option<_>>` on the wire, so an absent one is left alone
			// and an explicit null clears it. Nesting them under `data` would
			// send an empty changeset and quietly write nothing.
			run: () =>
				callApi("fleet/machines", "update", {
					machine_id: machine.id,
					name: box.name.trim() === "" ? null : box.name.trim(),
					group_id: box.groupId,
					cloud: box.cloud === "" ? null : box.cloud === "true",
					geolocation:
						box.lat && box.lon
							? { lat: Number(box.lat), lon: Number(box.lon) }
							: null,
					is_monitored: box.isMonitored,
					alert_when_down_for: Math.max(
						60,
						Math.round(Number(box.alertWhenDownMinutes) * 60),
					),
					notes: box.notes,
					tags: box.tags,
				}),
		},
		...applications.map((application) => ({
			call: "fleet/applications/update" as GradedEndpoint,
			run: () => {
				const form = apps[application.id]!;
				return callApi("fleet/applications", "update", {
					server_id: application.id,
					data: {
						name: form.name.trim(),
						host: form.host.trim(),
						rank: form.rank === "" ? null : form.rank,
						// Sent whether or not the field is offered: a public name
						// already set survives its type losing eligibility, and
						// takes effect again if it regains it.
						// spec: APP#public-listing
						public_name:
							form.publicName.trim() === "" ? null : form.publicName.trim(),
						is_monitored: form.isMonitored,
						alert_when_down_for: Math.max(
							60,
							Math.round(Number(form.alertWhenDownMinutes) * 60),
						),
						notes: form.notes,
						tags: form.tags,
						may_manage_dns: form.mayManageDns,
						may_manage_tls: form.mayManageTls,
					},
				});
			},
		})),
	];

	if (box.alertWhenUnreachable === machineReachabilitySilenced) {
		const fn = box.alertWhenUnreachable ? "unsilence_machine" : "silence_machine";
		plan.push({
			call: `silenced_refs/${fn}`,
			run: () =>
				callApi("silenced_refs", fn, {
					machine_id: machine.id,
					source: REACHABILITY_CHECK.source,
					ref: REACHABILITY_CHECK.ref,
				}),
		});
	}
	for (const application of applications) {
		const wants = apps[application.id]!.alertWhenUnreachable;
		const was = !applicationReachabilitySilenced.has(application.id);
		if (wants === was) continue;
		const fn = wants ? "unsilence_server" : "silence_server";
		plan.push({
			call: `silenced_refs/${fn}`,
			run: () =>
				callApi("silenced_refs", fn, {
					server_id: application.id,
					source: REACHABILITY_CHECK.source,
					ref: REACHABILITY_CHECK.ref,
				}),
		});
	}

	const saveCalls = plan.map((step) => step.call);

	const onSubmit = async (e: React.FormEvent) => {
		e.preventDefault();
		if (!box.groupId) return;
		setPending(true);
		setError(null);
		try {
			for (const step of plan) await step.run();

			navigate(`/fleet/machines/${machine.id}`);
		} catch (err) {
			setError(err instanceof Error ? err.message : String(err));
		} finally {
			setPending(false);
		}
	};

	return (
		<Stack spacing={3} component="form" onSubmit={onSubmit}>
			<Typography variant="h5" component="h1">
				Edit {machine.name ?? "machine"}
			</Typography>

			<Paper variant="outlined" sx={{ p: 3 }} data-testid="machine-section">
				<Stack spacing={2}>
					<Typography variant="h6" component="h2">
						Machine
					</Typography>

					{/* Not required, unlike at creation: a box that arrived
					    without a name — a migrated server that had none — has
					    to stay editable. */}
					<TextField
						label="Name"
						value={box.name}
						onChange={(e) => setBox({ ...box, name: e.target.value })}
						disabled={pending}
					/>

					<GroupControl
						currentGroupId={box.groupId}
						onChange={(groupId) => setBox({ ...box, groupId })}
						disabled={pending}
						required
					/>
					<Typography variant="caption" color="text.secondary">
						The applications on this machine take its group, so moving the box
						moves them with it.
					</Typography>

					<Stack direction={{ xs: "column", md: "row" }} spacing={2}>
						<TextField
							select
							label="Location"
							value={box.cloud}
							onChange={(e) =>
								setBox({
									...box,
									cloud: e.target.value as "" | "true" | "false",
								})
							}
							disabled={pending}
							sx={{ minWidth: 180 }}
						>
							<MenuItem value="">unknown</MenuItem>
							<MenuItem value="true">cloud</MenuItem>
							<MenuItem value="false">on premise</MenuItem>
						</TextField>
						<TextField
							label="Latitude"
							value={box.lat}
							onChange={(e) => setBox({ ...box, lat: e.target.value })}
							disabled={pending}
							sx={{ flex: 1 }}
						/>
						<TextField
							label="Longitude"
							value={box.lon}
							onChange={(e) => setBox({ ...box, lon: e.target.value })}
							disabled={pending}
							sx={{ flex: 1 }}
						/>
					</Stack>

					<FormControlLabel
						control={
							<Checkbox
								checked={box.isMonitored}
								onChange={(e) =>
									setBox({ ...box, isMonitored: e.target.checked })
								}
								disabled={pending}
							/>
						}
						label="Monitor this machine"
					/>
					<Typography variant="caption" color="text.secondary">
						When off, no check on this machine alerts: its checks are still
						determined and shown, and its health and reachability are marked
						as unmonitored wherever they appear, but nothing triggers or joins
						an incident. The applications on it are unaffected.
					</Typography>

					<FormControlLabel
						control={
							<Checkbox
								checked={box.alertWhenUnreachable}
								onChange={(e) =>
									setBox({ ...box, alertWhenUnreachable: e.target.checked })
								}
								disabled={pending}
							/>
						}
						label="Alert when this machine is unreachable"
					/>
					<Typography variant="caption" color="text.secondary">
						When off, every other check on this box alerts as normal and only
						the box going away is quiet. This quiets it for every application
						on the machine.
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
							value={box.alertWhenDownMinutes}
							onChange={(e) =>
								setBox({ ...box, alertWhenDownMinutes: e.target.value })
							}
							disabled={
								pending || !box.isMonitored || !box.alertWhenUnreachable
							}
							slotProps={{ htmlInput: { min: 1, step: 1 } }}
							sx={{ width: 140 }}
						/>
					</Stack>

					<TextField
						label="Notes"
						multiline
						minRows={3}
						value={box.notes}
						onChange={(e) => setBox({ ...box, notes: e.target.value })}
						disabled={pending}
						helperText="Operator notes shown on the machine's page. Plain text."
					/>

					<Stack spacing={1}>
						<Typography variant="subtitle1">Tags</Typography>
						<TagsEditor
							value={box.tags}
							onChange={(tags) => setBox({ ...box, tags })}
							disabled={pending}
						/>
					</Stack>
				</Stack>
			</Paper>

			{applications.length === 0 ? (
				<Alert severity="info">
					Nothing runs on this machine yet. Applications appear here as it
					reports them.
				</Alert>
			) : (
				applications.map((application) => (
					<ApplicationFields
						key={application.id}
						application={application}
						groupId={box.groupId}
						form={apps[application.id]!}
						onChange={(patch) => patchApp(application.id, patch)}
						disabled={pending}
					/>
				))
			)}

			{error && <Alert severity="error">{error}</Alert>}

			<Stack direction="row" spacing={1}>
				<GradedAction calls={saveCalls}>
					<Button
						type="submit"
						variant="contained"
						disabled={pending || !box.groupId}
					>
						{pending ? "Saving…" : "Save"}
					</Button>
				</GradedAction>
				<Button
					type="button"
					variant="outlined"
					color="error"
					onClick={() => navigate(`/fleet/machines/${machine.id}`)}
					disabled={pending}
				>
					Cancel
				</Button>
			</Stack>
		</Stack>
	);
}

/// One application's section of the machine's form.
///
/// No group and no location: those are the box's, and offering them per
/// workload is what made "where do I edit this" have two answers.
/// spec: FLT#what-each-carries
function ApplicationFields({
	application,
	groupId,
	form,
	onChange,
	disabled,
}: {
	application: ServerInfo;
	groupId: string | null;
	form: ApplicationForm;
	onChange: (patch: Partial<ApplicationForm>) => void;
	disabled: boolean;
}) {
	const caps = useApplicationTypeCaps(application.type);
	const canListPublicly = caps?.public_listing === true;
	return (
		<Paper
			variant="outlined"
			sx={{ p: 3 }}
			data-testid="application-section"
			data-application={application.id}
		>
			<Stack spacing={2}>
				<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
					<Typography variant="h6" component="h2">
						{applicationName(application)}
					</Typography>
					{/* The type is reported rather than entered, so it is shown
					    and not offered. */}
					{/* spec: APP#where-a-type-comes-from */}
					<ApplicationTypeChip type={application.type} />
				</Stack>

				<TextField
					label="Name"
					value={form.name}
					onChange={(e) => onChange({ name: e.target.value })}
					disabled={disabled}
				/>
				<TextField
					label="URL"
					value={form.host}
					onChange={(e) => onChange({ host: e.target.value })}
					disabled={disabled}
					helperText="Where an operator reaches it. Empty falls back to the box's tailnet name."
				/>
				<TextField
					select
					label="Rank"
					value={form.rank}
					onChange={(e) => onChange({ rank: e.target.value as ServerRank | "" })}
					disabled={disabled}
				>
					{RANK_OPTIONS.map((o) => (
						<MenuItem key={o.value} value={o.value}>
							{o.label}
						</MenuItem>
					))}
				</TextField>

				{canListPublicly && (
					<TextField
						label="Name in Tamanu Mobile app"
						value={form.publicName}
						onChange={(e) => onChange({ publicName: e.target.value })}
						disabled={disabled}
						helperText="Leave empty to hide this application from the public mobile-app list."
					/>
				)}

				<FormControlLabel
					control={
						<Checkbox
							checked={form.isMonitored}
							onChange={(e) => onChange({ isMonitored: e.target.checked })}
							disabled={disabled}
						/>
					}
					label="Monitor this application"
				/>
				<FormControlLabel
					control={
						<Checkbox
							checked={form.alertWhenUnreachable}
							onChange={(e) =>
								onChange({ alertWhenUnreachable: e.target.checked })
							}
							disabled={disabled}
						/>
					}
					label="Alert when this application is unreachable"
				/>
				<Stack
					direction={{ xs: "column", md: "row" }}
					spacing={2}
					sx={{ alignItems: { md: "center" } }}
				>
					<Typography variant="body2">
						File an issue when this application is unreachable for
					</Typography>
					<TextField
						label="minutes"
						type="number"
						value={form.alertWhenDownMinutes}
						onChange={(e) => onChange({ alertWhenDownMinutes: e.target.value })}
						disabled={
							disabled || !form.isMonitored || !form.alertWhenUnreachable
						}
						slotProps={{ htmlInput: { min: 1, step: 1 } }}
						sx={{ width: 140 }}
					/>
				</Stack>

				<NameManagementGrants
					groupId={groupId}
					mayManageDns={form.mayManageDns}
					mayManageTls={form.mayManageTls}
					setMayManageDns={(v) => onChange({ mayManageDns: v })}
					setMayManageTls={(v) => onChange({ mayManageTls: v })}
					disabled={disabled}
				/>

				<TextField
					label="Notes"
					multiline
					minRows={3}
					value={form.notes}
					onChange={(e) => onChange({ notes: e.target.value })}
					disabled={disabled}
					helperText="Operator notes shown on the application's page. Plain text."
				/>

				<Stack spacing={1}>
					<Typography variant="subtitle1">Tags</Typography>
					<TagsEditor
						value={form.tags}
						onChange={(tags) => onChange({ tags })}
						disabled={disabled}
					/>
				</Stack>
			</Stack>
		</Paper>
	);
}
