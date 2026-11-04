import {
	Alert,
	Box,
	IconButton,
	LinearProgress,
	Stack,
	Tooltip,
	Typography,
} from "@mui/material";
import BuildCircleIcon from "@mui/icons-material/BuildCircle";
import CancelIcon from "@mui/icons-material/Cancel";
import CheckCircleIcon from "@mui/icons-material/CheckCircle";
import CloseIcon from "@mui/icons-material/Close";
import NotificationsOffIcon from "@mui/icons-material/NotificationsOff";
import PreviewIcon from "@mui/icons-material/Preview";
import RemoveCircleOutlinedIcon from "@mui/icons-material/RemoveCircleOutlined";
import WarningAmberIcon from "@mui/icons-material/WarningAmber";
import { useApi, type ApiState } from "../api";
import CheckExtrasList, { checkEntryExtras } from "./CheckExtras";
import ExternalUsersDetails, {
	parseExternalUserSessions,
} from "./ExternalUsersDetails";
import HealthChip from "./HealthChip";
import TimeAgo from "./TimeAgo";
import TimezoneTooltip from "./TimezoneTooltip";
import VersionIndicator from "./VersionIndicator";
import {
	useApplicationTypeCaps,
	useApplicationTypeLabel,
} from "../hooks/useApplicationTypes";
import {
	CHECK_RESULT_INTENT,
	type CheckResult,
	type ConsolidatedChecks,
	type StatusSnapshotData,
} from "../types";

/** Inline panel rendering the server's status at a given point in time.
 * Hits `/api/statuses/snapshot` lazily on mount; each (serverId, at)
 * gets its own component instance via the caller's keying so there is no
 * stale-data flash when the user toggles between snapshots. Two or more
 * panels can be open at once for side-by-side comparison. */
export default function StatusSnapshotPanel({
	serverId,
	at,
	onClose,
}: {
	serverId: string;
	/** Timestamp to look up "as of". When null, the endpoint returns
	 * the latest status. */
	at: string | null;
	onClose: () => void;
}) {
	const result = useApi(
		"statuses",
		"snapshot",
		{ server_id: serverId, at },
		[serverId, at],
	);
	return (
		<Box
			sx={{
				mt: 1,
				p: 1.5,
				border: 1,
				borderColor: "divider",
				borderRadius: 1,
			}}
		>
			<Stack
				direction="row"
				spacing={1.5}
				sx={{ alignItems: "center", flexWrap: "wrap", mb: 1 }}
			>
				<Typography variant="overline" color="text.secondary">
					Status snapshot
				</Typography>
				{result.status === "ok" && result.data && (
					<>
						<HealthChip health={result.data.checks.health_state} />
						<Typography variant="body2" color="text.secondary">
							<TimeAgo timestamp={result.data.created_at} />
						</Typography>
					</>
				)}
				<Box sx={{ ml: "auto" }}>
					<IconButton aria-label="Close" size="small" onClick={onClose}>
						<CloseIcon fontSize="small" />
					</IconButton>
				</Box>
			</Stack>
			<PanelBody result={result} />
		</Box>
	);
}

function PanelBody({
	result,
}: {
	result: ApiState<StatusSnapshotData | null>;
}) {
	if (result.status === "loading" || result.status === "idle") {
		return <LinearProgress />;
	}
	if (result.status === "error") {
		return <Alert severity="error">{result.error.message}</Alert>;
	}
	if (result.data === null) {
		return (
			<Alert severity="info">
				No status snapshot is available for this point in time — the server
				hadn't reported yet.
			</Alert>
		);
	}
	const snap = result.data;
	return (
		<Stack spacing={2}>
			<CuratedFields snap={snap} />
			<ChecksBlock checks={snap.checks} operators={snap.operators} />
			<ExtrasBlock extra={snap.extra} />
		</Stack>
	);
}

function CuratedFields({ snap }: { snap: StatusSnapshotData }) {
	const caps = useApplicationTypeCaps(snap.type);
	const label = useApplicationTypeLabel(snap.type);
	const tracking = caps?.version_tracking;
	return (
		<Stack direction="row" spacing={3} sx={{ flexWrap: "wrap" }} useFlexGap>
			{/* A type with no application version shows no version field at
			    all — label included, since an empty "Version" reads as a
			    reporting failure rather than an absence.
			    spec: APP#versions */}
			{tracking !== undefined && tracking !== "absent" && (
				<Field label={label}>
					<VersionIndicator
						version={snap.version}
						tracking={tracking}
						distance={snap.version_distance}
					/>
				</Field>
			)}
			{snap.platform && <Field label="Platform" value={snap.platform} />}
			{snap.timezone && (
				<Field label="Timezone">
					<Typography variant="body2">
						<TimezoneTooltip tz={snap.timezone} />
					</Typography>
				</Field>
			)}
			{snap.postgres && <Field label="PostgreSQL" value={snap.postgres} mono />}
			{snap.reporting_schema && (
				<Field label="Reporting schema" value={snap.reporting_schema} mono />
			)}
			{snap.active_mobile_devices != null && (
				<Field
					label="Mobile devices (30d)"
					value={String(snap.active_mobile_devices)}
				/>
			)}
			{snap.nodejs && <Field label="Node.js" value={snap.nodejs} mono />}
			{snap.bestool && <Field label="bestool" value={snap.bestool} mono />}
			{snap.min_chrome_version != null && (
				<Field
					label="Chrome"
					value={`${snap.min_chrome_version} or later`}
					mono
				/>
			)}
		</Stack>
	);
}

function Field({
	label,
	value,
	mono = false,
	children,
}: {
	label: string;
	value?: string;
	mono?: boolean;
	children?: React.ReactNode;
}) {
	return (
		<Stack spacing={0.25}>
			<Typography variant="caption" color="text.secondary">
				{label}
			</Typography>
			{children ?? (
				<Typography
					variant="body2"
					sx={mono ? { fontFamily: "monospace" } : undefined}
				>
					{value}
				</Typography>
			)}
		</Stack>
	);
}

function ChecksBlock({
	checks,
	operators,
}: {
	checks: ConsolidatedChecks;
	operators: StatusSnapshotData["operators"];
}) {
	const entries = checks.checks;
	if (entries.length === 0) return null;
	return (
		<Box>
			<Typography variant="overline" color="text.secondary">
				Health checks ({entries.length})
			</Typography>
			<Stack spacing={1} sx={{ mt: 0.5 }}>
				{entries.map((entry) => {
					// external_users gets formatted session rows; otherwise a
					// generic dl of the check's detail. No "right now" claim
					// here — a snapshot is "as of" its push.
					const extras = checkEntryExtras(
						(entry.detail ?? {}) as Record<string, unknown>,
					);
					const sessions =
						entry.check === "external_users"
							? parseExternalUserSessions(extras)
							: null;
					const shownExtras =
						sessions === null
							? extras
							: extras.filter(([k]) => k !== "users" && k !== "count");
					const quiet =
						entry.silenced ||
						entry.effective === "passed" ||
						entry.effective === "skipped";
					return (
						<Stack
							key={`${entry.source}:${entry.check}`}
							direction="row"
							spacing={1.5}
							sx={{
								p: 1,
								border: 1,
								borderColor: "divider",
								borderRadius: 1,
								alignItems: "flex-start",
								bgcolor: quiet ? undefined : "action.hover",
							}}
						>
							<CheckIcon
								observed={entry.observed as CheckResult | null}
								effective={entry.effective as CheckResult}
								silenced={entry.silenced}
							/>
							<Box sx={{ flex: 1, minWidth: 0 }}>
								<Stack
									direction="row"
									spacing={1}
									sx={{ alignItems: "baseline" }}
								>
									<Typography variant="body2" sx={{ fontFamily: "monospace" }}>
										{entry.check}
									</Typography>
									<Typography variant="caption" color="text.secondary">
										{entry.source}
									</Typography>
								</Stack>
								{sessions !== null && (
									<ExternalUsersDetails
										sessions={sessions}
										operators={operators}
									/>
								)}
								<CheckExtrasList extras={shownExtras} />
							</Box>
						</Stack>
					);
				})}
			</Stack>
		</Box>
	);
}

function ExtrasBlock({ extra }: { extra: StatusSnapshotData["extra"] }) {
	const obj = (extra ?? {}) as Record<string, unknown>;
	if (Object.keys(obj).length === 0) return null;
	return (
		<Box>
			<details>
				<summary>Raw payload by source</summary>
				<Box
					component="pre"
					sx={{
						mt: 1,
						p: 1.5,
						borderRadius: 1,
						bgcolor: "action.hover",
						overflow: "auto",
						fontSize: "0.85em",
					}}
				>
					{JSON.stringify(extra, null, 2)}
				</Box>
			</details>
		</Box>
	);
}

/// Per-check status indicator, coloured by the check's *effective* result
/// (what policy grades it to): passed → green tick, failed → red ⊘,
/// warning → amber triangle, broken → orange wrench (the check itself
/// errored, not the system), skipped → grey dash. Silenced checks get a
/// neutral grey icon whatever they reported. When the observed result
/// differs from the effective one, the tooltip notes the grading.
function CheckIcon({
	observed,
	effective,
	silenced = false,
}: {
	observed: CheckResult | null;
	effective: CheckResult;
	silenced?: boolean;
}) {
	if (silenced) {
		return (
			<Tooltip
				title={`Silenced — reported ${observed ?? "?"}, not counted toward server health`}
				arrow
			>
				<NotificationsOffIcon fontSize="small" color="disabled" />
			</Tooltip>
		);
	}
	const tooltip =
		observed && observed !== effective
			? `${observed}, graded ${effective} — ${CHECK_RESULT_INTENT[effective]}`
			: `${effective} — ${CHECK_RESULT_INTENT[effective]}`;
	switch (effective) {
		case "passed":
			return (
				<Tooltip title={tooltip} arrow>
					<CheckCircleIcon fontSize="small" color="success" />
				</Tooltip>
			);
		case "failed":
			return (
				<Tooltip title={tooltip} arrow>
					<CancelIcon fontSize="small" color="error" />
				</Tooltip>
			);
		case "warning":
			return (
				<Tooltip title={tooltip} arrow>
					<WarningAmberIcon fontSize="small" color="warning" />
				</Tooltip>
			);
		case "broken":
			return (
				<Tooltip title={tooltip} arrow>
					<BuildCircleIcon fontSize="small" color="warning" />
				</Tooltip>
			);
		case "skipped":
			return (
				<Tooltip title={tooltip} arrow>
					<RemoveCircleOutlinedIcon fontSize="small" color="disabled" />
				</Tooltip>
			);
	}
}

/** Toggle button that opens an inline snapshot panel. Caller owns the
 * `open` state and decides where to render the panel — the button just
 * reflects open/closed visually. */
export function StatusSnapshotButton({
	open,
	onClick,
	tooltip = "View status snapshot",
}: {
	open: boolean;
	onClick: () => void;
	tooltip?: string;
}) {
	return (
		<Tooltip title={open ? "Close status snapshot" : tooltip}>
			<IconButton
				aria-label={tooltip}
				size="small"
				color={open ? "primary" : "default"}
				onClick={(e) => {
					e.stopPropagation();
					onClick();
				}}
			>
				<PreviewIcon fontSize="small" />
			</IconButton>
		</Tooltip>
	);
}
