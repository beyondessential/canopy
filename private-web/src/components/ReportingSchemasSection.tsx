import {
	Alert,
	Box,
	Button,
	Chip,
	LinearProgress,
	Paper,
	Table,
	TableBody,
	TableCell,
	TableHead,
	TableRow,
	Tooltip,
	Typography,
} from "@mui/material";
import { useApi, useApiAction } from "../api";

type PairState = "awaiting" | "built" | "failed";

/// Which of the group's versions have a reporting schema, which failed, and
/// which are still to be built, so whether the group's applications can be
/// offered the schema for the version they run or are moving to is answered in
/// one place.
// spec: RPT#alerting
export default function ReportingSchemasSection({
	groupId,
}: {
	groupId: string;
}) {
	const pairs = useApi(
		"reporting_schemas",
		"for_group",
		{ group_id: groupId },
		[groupId],
	);
	const build = useApiAction("reporting_schemas", "build");

	if (pairs.status === "loading" || pairs.status === "idle") {
		return (
			<Paper variant="outlined" sx={{ p: 2 }}>
				<SectionHeading />
				<LinearProgress />
			</Paper>
		);
	}
	if (pairs.status === "error") {
		return (
			<Paper variant="outlined" sx={{ p: 2 }}>
				<SectionHeading />
				<Alert severity="error">{pairs.error.message}</Alert>
			</Paper>
		);
	}

	if (pairs.data.length === 0) {
		return (
			<Paper variant="outlined" sx={{ p: 2 }} data-testid="reporting-schemas">
				<SectionHeading />
				<Typography variant="body2" color="text.secondary">
					Nothing to build for this group: either no builder is declared for it
					under Backups, or no Tamanu application in it reports a published
					version.
				</Typography>
			</Paper>
		);
	}

	const ask = async (versionId: string) => {
		try {
			await build.call({ group_id: groupId, version_id: versionId });
			pairs.reload();
		} catch {
			/* surfaced via build.error */
		}
	};

	return (
		<Paper variant="outlined" sx={{ p: 2 }} data-testid="reporting-schemas">
			<SectionHeading />
			{build.error && (
				<Alert severity="error" sx={{ mb: 1 }}>
					{build.error.message}
				</Alert>
			)}
			<Table size="small">
				<TableHead>
					<TableRow>
						<TableCell>Version</TableCell>
						<TableCell>Schema</TableCell>
						<TableCell />
					</TableRow>
				</TableHead>
				<TableBody>
					{pairs.data.map((pair) => (
						<TableRow key={pair.version_id} data-testid="reporting-schema-row">
							<TableCell sx={{ fontFamily: "monospace" }}>
								{pair.version}
							</TableCell>
							<TableCell>
								<StateChip state={pair.state as PairState} error={pair.error} />
							</TableCell>
							<TableCell align="right">
								{pair.requested ? (
									<Typography variant="caption" color="text.secondary">
										Build asked for
									</Typography>
								) : (
									<Button
										size="small"
										onClick={() => ask(pair.version_id)}
										disabled={build.pending}
									>
										{pair.state === "awaiting" ? "Build sooner" : "Build again"}
									</Button>
								)}
							</TableCell>
						</TableRow>
					))}
				</TableBody>
			</Table>
		</Paper>
	);
}

function StateChip({
	state,
	error,
}: {
	state: PairState;
	error?: string | null;
}) {
	if (state === "built") {
		return <Chip size="small" color="success" label="Built" />;
	}
	if (state === "awaiting") {
		return <Chip size="small" variant="outlined" label="Awaiting build" />;
	}
	return (
		<Tooltip title={error ?? "the build failed"}>
			<Chip size="small" color="warning" label="Failed" />
		</Tooltip>
	);
}

function SectionHeading() {
	return (
		<Box sx={{ mb: 1 }}>
			<Typography variant="h6">Reporting schemas</Typography>
			<Typography variant="body2" color="text.secondary">
				One per version this group runs or is moving to, built from a replica
				of the group's own data.
			</Typography>
		</Box>
	);
}
