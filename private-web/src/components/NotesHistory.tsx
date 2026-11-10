import { Box, Button, Collapse, Stack, Typography } from "@mui/material";
import { useState } from "react";
import { useApi } from "../api";
import TimeAgo from "./TimeAgo";

const HOW: Record<string, string> = {
	operator: "the operator interface",
	ansible_import: "an Ansible import",
	token: "an access token",
};

type Record_ =
	| { module: "fleet/groups"; id: string }
	| { module: "fleet/machines"; id: string }
	| { module: "fleet/applications"; id: string };

/** The revision log of a record's notes, newest first, behind a button.
 * Renders nothing for a record whose notes have never had a revision. */
export default function NotesHistory({
	record,
	refreshKey = 0,
}: {
	record: Record_;
	refreshKey?: number;
}) {
	const [open, setOpen] = useState(false);
	const params =
		record.module === "fleet/groups"
			? { server_group_id: record.id }
			: record.module === "fleet/machines"
				? { machine_id: record.id }
				: { server_id: record.id };
	const history = useApi(record.module, "notes_history", params, [
		record.module,
		record.id,
		refreshKey,
	]);
	if (history.status !== "ok" || history.data.length === 0) return null;

	return (
		<Box>
			<Button size="small" onClick={() => setOpen((v) => !v)}>
				{open ? "Hide history" : `History (${history.data.length})`}
			</Button>
			<Collapse in={open} unmountOnExit>
				<Stack spacing={1} sx={{ mt: 1 }}>
					{history.data.map((revision) => (
						<Box
							key={revision.revision}
							sx={{
								p: 1,
								border: 1,
								borderColor: "divider",
								borderRadius: 1,
							}}
						>
							<Typography variant="caption" color="text.secondary">
								{revision.author
									? `${revision.author} through ${HOW[revision.via ?? ""] ?? "an unknown route"}`
									: "Before history was kept"}
								{" · "}
								<TimeAgo timestamp={revision.created_at} />
							</Typography>
							{revision.notes === "" ? (
								<Typography variant="body2" color="text.secondary">
									Cleared
								</Typography>
							) : (
								<Typography variant="body2" sx={{ whiteSpace: "pre-wrap" }}>
									{revision.notes}
								</Typography>
							)}
						</Box>
					))}
				</Stack>
			</Collapse>
		</Box>
	);
}
