import { Box, Button, Paper, Stack, Typography } from "@mui/material";
import WarningAmberIcon from "@mui/icons-material/WarningAmber";
import { Link as RouterLink } from "react-router-dom";
import TimeAgo from "./TimeAgo";
import { useIsNotificationHeld } from "../hooks/useIsNotificationHeld";
import { isIncidentLingering, type IncidentData } from "../types";

/// The group's open incident, called out above whatever page an operator is
/// on.
///
/// An incident is always the group's, never one application's or one box's, so
/// the same card appears on the group and on both detail pages: an operator who
/// lands on a workload needs to know its group is already on fire, and
/// finding that out by noticing a coloured button is finding it out too late.
/// spec: INC
export default function ActiveIncidentCard({
	incident,
	targetName,
}: {
	incident: IncidentData;
	/// Named when what is on screen is not the incident's target, so the card
	/// says whose incident it is rather than implying it belongs to what is
	/// being read.
	targetName?: string | null;
}) {
	const held = useIsNotificationHeld(incident.notification_held_until);
	const lingering = isIncidentLingering(incident);
	const tone = lingering ? "info" : held ? "warning" : "error";
	return (
		<Paper
			variant="outlined"
			data-testid="active-incident"
			sx={{ p: 2, borderColor: `${tone}.main`, borderWidth: 2 }}
		>
			<Stack
				direction="row"
				spacing={2}
				sx={{ alignItems: "center", justifyContent: "space-between" }}
			>
				<Stack direction="row" spacing={1.5} sx={{ alignItems: "center" }}>
					<WarningAmberIcon color={tone} />
					<Box>
						<Typography variant="h6" component="h2">
							{targetName
								? `Active incident in ${targetName}`
								: "Active incident"}
							<Box
								component="span"
								sx={{
									ml: 1,
									fontFamily: "monospace",
									color: "text.secondary",
									fontWeight: "normal",
									fontSize: "0.85em",
								}}
							>
								{incident.id.slice(0, 8)}
							</Box>
						</Typography>
						<Typography variant="body2" color="text.secondary">
							opened <TimeAgo timestamp={incident.opened_at} />
						</Typography>
						{lingering && incident.lingering_since && (
							<Typography variant="body2" sx={{ color: "info.main" }}>
								Recovering; last failure cleared{" "}
								<TimeAgo timestamp={incident.lingering_since} />
							</Typography>
						)}
						{!lingering && held && incident.notification_held_until && (
							<Typography variant="body2" sx={{ color: "warning.main" }}>
								Holding; posting{" "}
								<TimeAgo timestamp={incident.notification_held_until} />
							</Typography>
						)}
					</Box>
				</Stack>
				<Button
					component={RouterLink}
					to={`/incidents/${incident.id}`}
					variant="outlined"
					color={tone}
				>
					Open
				</Button>
			</Stack>
		</Paper>
	);
}
