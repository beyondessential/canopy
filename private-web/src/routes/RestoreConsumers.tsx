import {
	Alert,
	Chip,
	LinearProgress,
	Paper,
	Stack,
	Tooltip,
	Typography,
} from "@mui/material";
import { useApi } from "../api";
import { deviceLabel } from "../components/DeviceShorty";
import { usePageTitle } from "../hooks/usePageTitle";

/// Fleet-wide restore consumers (`backup-restore` devices) and the intents each
/// currently supports. Per-group replica declarations live on each group's
/// backup page; this is the consumer roster the declaration forms draw from.
export default function RestoreConsumers() {
	usePageTitle("Restore consumers");
	const consumers = useApi("restore_replicas", "consumers");

	return (
		<Stack spacing={2}>
			<Typography variant="body2" color="text.secondary">
				A restore consumer is a device with the <code>backup-restore</code>{" "}
				role. It registers the intents it can satisfy, and Canopy dispatches only
				matching replica declarations. Declare replicas on each group's backup
				page.
			</Typography>

			{consumers.status === "loading" || consumers.status === "idle" ? (
				<LinearProgress />
			) : consumers.status === "error" ? (
				<Alert severity="error">{consumers.error.message}</Alert>
			) : consumers.data.length === 0 ? (
				<Alert severity="info">
					No restore consumers. Promote a device to the{" "}
					<code>backup-restore</code> role on its device page.
				</Alert>
			) : (
				<Stack spacing={1}>
					{consumers.data.map((c) => (
						<Paper key={c.device_id} variant="outlined" sx={{ p: 1.5 }}>
							<Typography variant="subtitle2">
								{deviceLabel(c.name)}
							</Typography>
							<Stack
							direction="row"
							spacing={0.5}
							useFlexGap
							sx={{ mt: 0.5, flexWrap: "wrap" }}
						>
								{c.intents.length === 0 ? (
									<Typography variant="body2" color="text.secondary">
										No capabilities registered yet.
									</Typography>
								) : (
									c.intents.map((i) => {
										const tip = [
											i.description,
											i.semantics && i.semantics.length > 0
												? `semantics: ${i.semantics.join(", ")}`
												: null,
										]
											.filter(Boolean)
											.join(" — ");
										const chip = (
											<Chip key={i.intent} label={i.intent} size="small" />
										);
										return tip ? (
											<Tooltip key={i.intent} title={tip}>
												{chip}
											</Tooltip>
										) : (
											chip
										);
									})
								)}
							</Stack>
						</Paper>
					))}
				</Stack>
			)}
		</Stack>
	);
}
