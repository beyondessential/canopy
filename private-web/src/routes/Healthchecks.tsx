import {
	Alert,
	Box,
	Button,
	Chip,
	FormControlLabel,
	LinearProgress,
	MenuItem,
	Paper,
	Select,
	Stack,
	Switch,
	Table,
	TableBody,
	TableCell,
	TableContainer,
	TableHead,
	TableRow,
	Typography,
} from "@mui/material";
import { useMemo, useState } from "react";
import { Link as RouterLink } from "react-router-dom";
import { ApiError, useApi, useApiAction } from "../api";
import CheckResultChip from "../components/CheckResultChip";
import { GradedAction } from "../components/GradedAction";
import TimeAgo from "../components/TimeAgo";
import { useIsAdmin } from "../hooks/useIsAdmin";
import { usePageTitle } from "../hooks/usePageTitle";
import {
	CEILINGS,
	HEALTHCHECK_SOURCES_PATH,
	healthcheckSettingsPath,
	namespaceSegment,
	type Ceiling,
	type CheckPolicyData,
} from "../types";

/** A catalogued check unreported anywhere for this long is a
 * decommissioning candidate — mirrors the backend's 7-day window. */
const GONE_QUIET_MS = 7 * 24 * 60 * 60 * 1000;

function isGoneQuiet(row: CheckPolicyData): boolean {
	if (row.decommissioned_at || !row.last_seen) return false;
	return Date.now() - new Date(row.last_seen).getTime() > GONE_QUIET_MS;
}

export default function Healthchecks() {
	usePageTitle("Healthchecks");
	const isAdmin = useIsAdmin() === true;
	const list = useApi("healthchecks", "list");
	const [onlyPending, setOnlyPending] = useState(false);
	const [onlyGoneQuiet, setOnlyGoneQuiet] = useState(false);

	const rows: CheckPolicyData[] = list.status === "ok" ? list.data : [];
	const pendingCount = useMemo(
		() => rows.filter((r) => r.pending_review).length,
		[rows],
	);
	const goneQuietCount = useMemo(() => rows.filter(isGoneQuiet).length, [rows]);
	const visible = useMemo(
		() =>
			rows.filter(
				(r) =>
					(!onlyPending || r.pending_review) &&
					(!onlyGoneQuiet || isGoneQuiet(r)),
			),
		[rows, onlyPending, onlyGoneQuiet],
	);

	return (
		<Stack spacing={2}>
			<Box>
				<Typography variant="h6" component="h2" gutterBottom>
					Healthchecks
				</Typography>
				<Typography variant="body2" color="text.secondary">
					Catalog of healthchecks, one entry per reporting source and check
					name, with the ceiling each check's results are graded at. New
					checks land here at the default <strong>warning</strong> ceiling,
					marked pending review.
				</Typography>
			</Box>

			<SourcesLink />

			<Stack direction="row" spacing={2} sx={{ alignItems: "center" }}>
				{pendingCount > 0 ? (
					<Chip
						label={`${pendingCount} pending review`}
						color="warning"
						size="small"
					/>
				) : (
					<Chip label="all reviewed" color="success" size="small" variant="outlined" />
				)}
				{goneQuietCount > 0 && (
					<Chip
						label={`${goneQuietCount} gone quiet`}
						color="warning"
						size="small"
						variant="outlined"
						title="Checks not reported anywhere in the fleet for 7+ days — candidates for decommissioning"
					/>
				)}
				<FormControlLabel
					control={
						<Switch
							size="small"
							checked={onlyPending}
							onChange={(e) => setOnlyPending(e.target.checked)}
						/>
					}
					label="Show only pending review"
				/>
				<FormControlLabel
					control={
						<Switch
							size="small"
							checked={onlyGoneQuiet}
							onChange={(e) => setOnlyGoneQuiet(e.target.checked)}
						/>
					}
					label="Show only gone quiet"
				/>
			</Stack>

			{list.status === "loading" || list.status === "idle" ? (
				<LinearProgress />
			) : list.status === "error" ? (
				<Alert severity="error">{list.error.message}</Alert>
			) : visible.length === 0 ? (
				<Alert severity="info">
					{onlyGoneQuiet
						? "No checks have gone quiet."
						: onlyPending
							? "No checks pending review."
							: "No healthchecks reported yet."}
				</Alert>
			) : (
				<Paper variant="outlined">
					<TableContainer>
						<Table size="small">
							<TableHead>
								<TableRow>
									<TableCell>Source</TableCell>
									<TableCell>Check name</TableCell>
									<TableCell>Ceiling</TableCell>
									<TableCell>First seen</TableCell>
									<TableCell>Last seen</TableCell>
									<TableCell>Reviewed</TableCell>
								</TableRow>
							</TableHead>
							<TableBody>
								{visible.map((row) => (
									<HealthcheckRow
										key={`${row.source}:${namespaceSegment(row.namespace)}:${row.check_name}`}
										row={row}
										canEdit={isAdmin}
										onChanged={() => list.reload()}
									/>
								))}
							</TableBody>
						</Table>
					</TableContainer>
				</Paper>
			)}
		</Stack>
	);
}

function SourcesLink() {
	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<Stack
				direction="row"
				spacing={2}
				sx={{ alignItems: "center", justifyContent: "space-between" }}
			>
				<Box>
					<Typography variant="subtitle2" component="h3">
						Sources
					</Typography>
					<Typography variant="body2" color="text.secondary">
						Per-source reachability and ingest policy — how canopy treats each
						reporter going quiet, and whether the device API accepts its
						reports. High-danger, rarely changed; each change is confirmed.
					</Typography>
				</Box>
				<Button
					component={RouterLink}
					to={HEALTHCHECK_SOURCES_PATH}
					variant="outlined"
					size="small"
					sx={{ flexShrink: 0 }}
				>
					Manage sources
				</Button>
			</Stack>
		</Paper>
	);
}

function HealthcheckRow({
	row,
	canEdit,
	onChanged,
}: {
	row: CheckPolicyData;
	canEdit: boolean;
	onChanged: () => void;
}) {
	const update = useApiAction("healthchecks", "update");
	const decommission = useApiAction("healthchecks", "decommission");
	const [localCeiling, setLocalCeiling] = useState<Ceiling>(row.ceiling as Ceiling);
	const [localEscalates, setLocalEscalates] = useState(row.escalates);

	const goneQuiet = isGoneQuiet(row);
	const decommissioned = row.decommissioned_at != null;

	const doDecommission = async () => {
		if (
			!window.confirm(
				`Decommission ${row.source}/${row.qualified_name}? Its states across all ` +
					`servers will be resolved and it will stop counting toward health ` +
					`and staleness. It returns pending review if reported again.`,
			)
		)
			return;
		try {
			await decommission.call({
				source: row.source,
				namespace: row.namespace,
				check_name: row.check_name,
			});
			onChanged();
		} catch {
			// Error is surfaced in the row below.
		}
	};

	const save = async () => {
		try {
			await update.call({
				source: row.source,
				namespace: row.namespace,
				check_name: row.check_name,
				ceiling: localCeiling,
				escalates: localEscalates,
				notes: row.notes,
			});
			onChanged();
		} catch {
			// Revert the editor selection on failure so the row's
			// rendered state matches the server's.
			setLocalCeiling(row.ceiling as Ceiling);
			setLocalEscalates(row.escalates);
		}
	};

	const hasRules = row.rule_count > 0;

	return (
		<TableRow hover>
			<TableCell sx={{ fontFamily: "monospace" }}>{row.source}</TableCell>
			<TableCell sx={{ fontFamily: "monospace" }}>
				<RouterLink to={healthcheckSettingsPath(row.source, row.namespace, row.check_name)}>
					{row.qualified_name}
				</RouterLink>
			</TableCell>
			<TableCell>
				{hasRules ? (
					<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
						<Typography variant="body2">
							<RouterLink to={healthcheckSettingsPath(row.source, row.namespace, row.check_name)}>
								Custom rules ({row.rule_count})
							</RouterLink>
						</Typography>
						<Typography variant="caption" color="text.secondary">
							· ceiling
						</Typography>
						<CheckResultChip result={row.ceiling as Ceiling} />
						{row.escalates && <EscalatesChip />}
					</Stack>
				) : (
					<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
						{canEdit ? (
							<>
								<Select
									size="small"
									value={localCeiling}
									onChange={(e) => setLocalCeiling(e.target.value as Ceiling)}
									disabled={update.pending}
									sx={{ minWidth: 120 }}
								>
									{CEILINGS.map((c) => (
										<MenuItem key={c} value={c}>
											<CheckResultChip result={c} />
										</MenuItem>
									))}
								</Select>
								<FormControlLabel
									control={
										<Switch
											size="small"
											checked={localEscalates}
											onChange={(e) => setLocalEscalates(e.target.checked)}
											disabled={update.pending}
										/>
									}
									label="escalates"
									slotProps={{ typography: { variant: "caption" } }}
								/>
							</>
						) : (
							<>
								<CheckResultChip result={row.ceiling as Ceiling} />
								{row.escalates && <EscalatesChip />}
							</>
						)}
						{canEdit && (
							<GradedAction calls="healthchecks/update">
								<Button
									size="small"
									variant="outlined"
									onClick={save}
									disabled={update.pending}
								>
									Save
								</Button>
							</GradedAction>
						)}
						<Typography variant="caption" color="text.secondary" sx={{ ml: 1 }}>
							·{" "}
							<RouterLink to={healthcheckSettingsPath(row.source, row.namespace, row.check_name)}>
								Add custom rules
							</RouterLink>
						</Typography>
					</Stack>
				)}
				{update.error && (
					<Typography variant="caption" color="error" sx={{ display: "block" }}>
						{formatError(update.error)}
					</Typography>
				)}
			</TableCell>
			<TableCell>
				<TimeAgo timestamp={row.first_seen} />
			</TableCell>
			<TableCell>
				{decommissioned ? (
					<Chip
						label="decommissioned"
						size="small"
						variant="outlined"
						title={
							row.decommissioned_at
								? `Decommissioned ${new Date(row.decommissioned_at).toLocaleString()}`
								: undefined
						}
					/>
				) : (
					<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
						{row.last_seen ? (
							<TimeAgo timestamp={row.last_seen} />
						) : (
							<Typography variant="caption" color="text.secondary">
								never
							</Typography>
						)}
						{goneQuiet && (
							<Chip label="gone quiet" color="warning" size="small" />
						)}
						{canEdit && goneQuiet && (
							<GradedAction calls="healthchecks/decommission">
								<Button
									size="small"
									color="warning"
									variant="outlined"
									onClick={doDecommission}
									disabled={decommission.pending}
								>
									Decommission
								</Button>
							</GradedAction>
						)}
					</Stack>
				)}
				{decommission.error && (
					<Typography variant="caption" color="error" sx={{ display: "block" }}>
						{formatError(decommission.error)}
					</Typography>
				)}
			</TableCell>
			<TableCell>
				{row.pending_review ? (
					<Chip label="pending review" color="warning" size="small" />
				) : (
					<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
						{row.reviewed_at && <TimeAgo timestamp={row.reviewed_at} />}
						{row.reviewed_by && (
							<Typography variant="caption" color="text.secondary">
								by {row.reviewed_by}
							</Typography>
						)}
					</Stack>
				)}
			</TableCell>
		</TableRow>
	);
}

function EscalatesChip() {
	return (
		<Chip
			label="escalates"
			color="error"
			size="small"
			variant="outlined"
			title="An effective failure notifies immediately, bypassing the incident grace period"
		/>
	);
}

function formatError(err: unknown): string {
	if (err instanceof ApiError) {
		const detail = err.detail as { title?: string } | null;
		if (detail?.title) return detail.title;
		return err.message;
	}
	if (err instanceof Error) return err.message;
	return String(err);
}
