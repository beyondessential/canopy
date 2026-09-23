import {
	Alert as MuiAlert,
	Box,
	Button,
	FormControlLabel,
	MenuItem,
	Stack,
	Switch,
	TextField,
} from "@mui/material";
import { useState } from "react";
import { useApiAction } from "../api";
import { GradedAction } from "./GradedAction";

/// Results an operator can raise a manual condition at: a failure can
/// open an incident; a warning joins one for context only.
const MANUAL_RESULTS = ["failed", "warning"] as const;
type ManualResult = (typeof MANUAL_RESULTS)[number];

export default function ManualEventForm({
	serverId,
	onSubmitted,
}: {
	serverId: string;
	onSubmitted?: () => void;
}) {
	const [result, setResult] = useState<ManualResult>("failed");
	const [escalates, setEscalates] = useState(false);
	const [message, setMessage] = useState("");
	const [description, setDescription] = useState("");

	const action = useApiAction("issues", "submit_manual_event");

	const submit = async () => {
		try {
			await action.call({
				applicationId: serverId,
				// Each manual submission is its own issue. Operators that need to
				// add context to an existing issue use the notes panel instead.
				ref: crypto.randomUUID(),
				result,
				escalates,
				description: description.trim() === "" ? null : description.trim(),
				message,
			});
			setMessage("");
			setDescription("");
			onSubmitted?.();
		} catch {
			/* surfaced via action.error */
		}
	};

	const valid = message.trim() !== "";

	return (
		<Stack spacing={2} sx={{ pt: 1 }}>
			<Stack direction="row" spacing={2} sx={{ alignItems: "center" }}>
				<TextField
					select
					label="Result"
					size="small"
					value={result}
					onChange={(e) => setResult(e.target.value as ManualResult)}
					sx={{ minWidth: 140 }}
				>
					{MANUAL_RESULTS.map((r) => (
						<MenuItem key={r} value={r}>
							{r}
						</MenuItem>
					))}
				</TextField>
				{result === "failed" && (
					<FormControlLabel
						control={
							<Switch
								checked={escalates}
								onChange={(e) => setEscalates(e.target.checked)}
								size="small"
							/>
						}
						label="Notify immediately"
					/>
				)}
			</Stack>
			<TextField
				label="Description (short)"
				size="small"
				value={description}
				onChange={(e) => setDescription(e.target.value)}
			/>
			<TextField
				label="Message"
				size="small"
				value={message}
				onChange={(e) => setMessage(e.target.value)}
				required
				multiline
				minRows={2}
			/>
			{action.error && (
				<MuiAlert severity="error">{action.error.message}</MuiAlert>
			)}
			<Box>
				<GradedAction calls="issues/submit_manual_event">
					<Button
						variant="contained"
						onClick={submit}
						disabled={!valid || action.pending}
					>
						{action.pending ? "Submitting…" : "Submit"}
					</Button>
				</GradedAction>
			</Box>
		</Stack>
	);
}
