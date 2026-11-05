import WarningAmberIcon from "@mui/icons-material/WarningAmber";
import { Stack, Typography } from "@mui/material";
import { useApi } from "../api";
import TimeAgo from "./TimeAgo";

/** When a machine last reported its packages and how many, flagging a report
 * Canopy could not place in an ecosystem, since nothing on the machine is then
 * checked. Nothing for a machine that has not reported.
 * spec: PKG#presentation */
export default function MachinePackagesLine({
	machineId,
	refreshKey,
}: {
	machineId: string;
	refreshKey?: number;
}) {
	const report = useApi(
		"vulnerabilities",
		"machine_packages",
		{ machine_id: machineId },
		[machineId, refreshKey],
	);
	if (report.status !== "ok" || !report.data) return null;
	const { package_count, reported_at, ecosystem, distribution, release } =
		report.data;

	return (
		<Stack
			direction="row"
			spacing={1}
			sx={{ alignItems: "center", mt: 2, flexWrap: "wrap" }}
			useFlexGap
		>
			<Typography variant="body2">
				{package_count} package{package_count === 1 ? "" : "s"} reported{" "}
				<TimeAgo timestamp={reported_at} />
			</Typography>
			{ecosystem == null && (
				<Stack direction="row" spacing={0.5} sx={{ alignItems: "center" }}>
					<WarningAmberIcon fontSize="small" color="warning" />
					<Typography variant="body2" color="warning.main">
						Not checked: {distribution} {release} is not a recognised release
					</Typography>
				</Stack>
			)}
		</Stack>
	);
}
