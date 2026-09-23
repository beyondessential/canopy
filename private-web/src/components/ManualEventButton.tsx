import { Button, Dialog, DialogContent, DialogTitle } from "@mui/material";
import AddAlertIcon from "@mui/icons-material/AddAlert";
import { useState } from "react";
import ActionButton from "./ActionButton";
import { GradedAction } from "./GradedAction";
import ManualEventForm from "./ManualEventForm";

/** Admin-only button on the ServerDetail header: opens a dialog with the
 * manual-event form. Label switches between "New incident" and "Add issue"
 * depending on `hasOpenIncident` — which the parent computes once from a
 * single group-level query and shares with all the buttons on the page
 * (header + child rows), so they all read the same group state. */
export default function ManualEventButton({
	serverId,
	hasOpenIncident,
	onSubmitted,
	size = "medium",
	action = false,
}: {
	serverId: string;
	/** Whether the server group currently has an open incident. The parent
	 * owns this state and threads it down so every button shows the same
	 * answer (a child server's own incidents list is empty even when the
	 * group's root has an open one). */
	hasOpenIncident: boolean;
	/** Called after a successful submission so the parent can refresh sibling
	 * panels (issues, incidents) that are now stale. */
	onSubmitted?: () => void;
	size?: "small" | "medium" | "large";
	/** Render as an expanding icon button for the server-detail action row.
	 * Default is the full-width labelled button used in the siblings table. */
	action?: boolean;
}) {
	const [open, setOpen] = useState(false);
	const label = hasOpenIncident ? "Add issue" : "New incident";

	return (
		<>
			{action ? (
				<GradedAction calls="issues/submit_manual_event">
					<ActionButton
						icon={<AddAlertIcon />}
						label={label}
						onClick={() => setOpen(true)}
					/>
				</GradedAction>
			) : (
				<GradedAction calls="issues/submit_manual_event">
					<Button
						variant="outlined"
						size={size}
						startIcon={<AddAlertIcon />}
						onClick={() => setOpen(true)}
					>
						{label}
					</Button>
				</GradedAction>
			)}
			<Dialog open={open} onClose={() => setOpen(false)} fullWidth maxWidth="sm">
				<DialogTitle>{label}</DialogTitle>
				<DialogContent>
					<ManualEventForm
						serverId={serverId}
						onSubmitted={() => {
							setOpen(false);
							onSubmitted?.();
						}}
					/>
				</DialogContent>
			</Dialog>
		</>
	);
}
