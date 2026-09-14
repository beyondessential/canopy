import CloseIcon from "@mui/icons-material/Close";
import {
	Alert,
	AlertTitle,
	Box,
	Chip,
	Dialog,
	DialogContent,
	DialogTitle,
	IconButton,
	Stack,
	Table,
	TableBody,
	TableCell,
	TableHead,
	TableRow,
	Typography,
	alpha,
} from "@mui/material";
import type { ReactNode } from "react";
import { formatDuration } from "../lib/migrationTests";
import type { ApiResponse, ServerRank } from "../types";
import ServerRankChip from "./ServerRankChip";
import TimeAgo from "./TimeAgo";

type LatestTest = NonNullable<
	ApiResponse<"migration_tests", "for_group">[number]["latest"]
>;

/// Every migration one test ran, in order, with the run's window above it. A
/// run is dozens of migrations of which one or two carry the whole duration,
/// which is what the slowest figure names.
export default function MigrationRunDialog({
	open,
	onClose,
	serverName,
	rank,
	targetVersion,
	latest,
}: {
	open: boolean;
	onClose: () => void;
	serverName: string;
	rank?: ServerRank | null;
	targetVersion: string;
	latest: LatestTest;
}) {
	const timings = [...latest.timings].sort((a, b) => a.ordinal - b.ordinal);
	const longest = timings.reduce((max, t) => Math.max(max, t.elapsed), 0);
	const slowest = longest > 0 ? timings.find((t) => t.elapsed === longest) : undefined;

	return (
		<Dialog
			open={open}
			onClose={onClose}
			fullWidth
			maxWidth="md"
			scroll="paper"
			data-testid="migration-run"
		>
			<DialogTitle component="div" sx={{ pb: 1 }}>
				<Stack
					direction="row"
					spacing={1}
					sx={{ alignItems: "center", justifyContent: "space-between" }}
				>
					<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
						<Typography variant="h6" component="h2">
							{serverName}
						</Typography>
						{rank && <ServerRankChip rank={rank} />}
						<Typography variant="body2" color="text.secondary">
							upgrading to {targetVersion}
						</Typography>
					</Stack>
					<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
						{latest.verdict === "passed" ? (
							<Chip size="small" color="success" label="passed" />
						) : (
							<Chip size="small" color="warning" label="failed" />
						)}
						<IconButton size="small" aria-label="Close" onClick={onClose}>
							<CloseIcon fontSize="small" />
						</IconButton>
					</Stack>
				</Stack>
			</DialogTitle>
			<DialogContent dividers>
				<Box
					sx={{
						display: "grid",
						gap: 2,
						gridTemplateColumns: "repeat(auto-fit, minmax(140px, 1fr))",
						mb: 2,
					}}
				>
					<Stat label="Migrations took" value={formatDuration(latest.total_elapsed)}>
						{timings.length} migration{timings.length === 1 ? "" : "s"}
					</Stat>
					<Stat
						label="Slowest"
						value={slowest ? formatDuration(slowest.elapsed) : "—"}
					>
						{slowest?.name ?? "all under a second"}
					</Stat>
					<Stat label="Tested" value={<TimeAgo timestamp={latest.reported_at} />}>
						<Box component="span" sx={{ fontFamily: "monospace" }}>
							{latest.snapshot_id ?? "no snapshot"}
						</Box>
					</Stat>
				</Box>

				{latest.failed_migration && (
					<Alert severity="warning" sx={{ mb: 2 }}>
						<AlertTitle sx={{ fontFamily: "monospace", wordBreak: "break-all" }}>
							{latest.failed_migration}
						</AlertTitle>
						{latest.error ? (
							<Box
								component="pre"
								sx={{
									m: 0,
									fontFamily: "monospace",
									fontSize: 13,
									whiteSpace: "pre-wrap",
									overflowWrap: "anywhere",
								}}
							>
								{latest.error}
							</Box>
						) : (
							"The runner did not say what went wrong."
						)}
					</Alert>
				)}

				<Table size="small" stickyHeader sx={{ tableLayout: "fixed" }}>
					<TableHead>
						<TableRow>
							<TableCell sx={{ width: 48 }}>#</TableCell>
							<TableCell>Migration</TableCell>
							<TableCell align="right" sx={{ width: 90 }}>
								Took
							</TableCell>
						</TableRow>
					</TableHead>
					<TableBody>
						{timings.map((timing) => {
							const failed = timing.name === latest.failed_migration;
							return (
								<TableRow
									key={timing.ordinal}
									data-testid="migration-timing"
									sx={
										failed
											? (theme) => ({
													bgcolor: alpha(theme.palette.warning.main, 0.12),
												})
											: undefined
									}
								>
									<TableCell sx={{ color: "text.secondary" }}>
										{timing.ordinal + 1}
									</TableCell>
									<TableCell
										sx={{ fontFamily: "monospace", overflowWrap: "anywhere" }}
									>
										<MigrationName name={timing.name} />
									</TableCell>
									<TableCell align="right" sx={{ whiteSpace: "nowrap" }}>
										{formatDuration(timing.elapsed)}
									</TableCell>
								</TableRow>
							);
						})}
					</TableBody>
				</Table>
			</DialogContent>
		</Dialog>
	);
}

/// The runner's names carry a timestamp prefix that is the same width on every
/// row and never what someone is looking for.
function MigrationName({ name }: { name: string }) {
	const split = name.match(/^(\d+-)(.*)$/);
	if (!split) return <>{name}</>;
	return (
		<>
			<Box component="span" sx={{ color: "text.secondary" }}>
				{split[1]}
			</Box>
			{split[2]}
		</>
	);
}

function Stat({
	label,
	value,
	children,
}: {
	label: string;
	value: ReactNode;
	children: ReactNode;
}) {
	return (
		<Box>
			<Typography
				variant="caption"
				color="text.secondary"
				component="div"
			>
				{label}
			</Typography>
			<Typography variant="body1">{value}</Typography>
			<Typography
				variant="caption"
				color="text.secondary"
				component="div"
				sx={{ overflowWrap: "anywhere" }}
			>
				{children}
			</Typography>
		</Box>
	);
}
