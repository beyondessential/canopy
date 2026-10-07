import WarningAmberIcon from "@mui/icons-material/WarningAmber";
import {
	Button,
	Dialog,
	DialogActions,
	DialogContent,
	DialogTitle,
	Typography,
} from "@mui/material";
import { useRef } from "react";
import {
	type RaisedMode,
	dangerReasonsSentence,
	modeLabel,
} from "../safety";
import type { DangerReason } from "../safety-modes";

/** A request to raise the session, and what the operator is told about it. */
/** The attribute marking the raise dialog in the page. */
const RAISE_DIALOG = "data-raise-dialog";

/** Whether `element` is inside the raise dialog. */
export function inRaiseDialog(element: Element): boolean {
	return !!element.closest(`[${RAISE_DIALOG}]`);
}

export interface RaiseRequest {
	/** The mode to raise to. */
	mode: RaisedMode;
	/**
	 * The action the operator was reaching for, when the raise is offered by a
	 * blocked control. Absent when it is asked for from the mode control.
	 */
	action?: string;
	/** Why the action needs danger, when it does. */
	reasons?: readonly DangerReason[];
}

/**
 * The confirmation that precedes a raise.
 *
 * From the mode control it is the plain "Enter danger mode?". From a blocked
 * control it is titled with the action and says what the action needs, and its
 * confirming choice says the action continues, because confirming both raises
 * and carries the action out.
 *
 * Nothing is focused on opening, so the keypress that activated the control
 * cannot also confirm the raise.
 */
export function RaiseDialog({
	request,
	onConfirm,
	onCancel,
}: {
	request: RaiseRequest | null;
	onConfirm: () => void;
	onCancel: () => void;
}) {
	// Kept while the dialog closes so it does not change under the exit
	// transition.
	const last = useRef<RaiseRequest | null>(null);
	if (request) last.current = request;
	const shown = request ?? last.current;
	if (!shown) return null;

	const danger = shown.mode === "danger";
	const mode = modeLabel(shown.mode).toLowerCase();
	const fromControl = shown.action !== undefined;

	return (
		<Dialog
			open={request !== null}
			// The control that asked returns focus itself, to the very element the
			// operator activated (see `inRaiseDialog`).
			disableRestoreFocus
			{...{ [RAISE_DIALOG]: "" }}
			// A click that lands on the backdrop is not a decision: the second click
			// of a double click on the control that asked lands there.
			onClose={(_, reason) => {
				if (reason !== "backdropClick") onCancel();
			}}
		>
			<DialogTitle
				sx={{
					display: "flex",
					alignItems: "center",
					gap: 1.25,
					color: danger ? "error.main" : undefined,
				}}
			>
				{danger && <WarningAmberIcon />}
				{fromControl ? shown.action : "Enter danger mode?"}
			</DialogTitle>
			<DialogContent>
				{fromControl ? (
					<Typography color="text.secondary" sx={{ mb: 2 }}>
						This action needs {mode} mode
						{danger && shown.reasons?.length
							? `: ${dangerReasonsSentence(shown.reasons)}`
							: ""}
						.
					</Typography>
				) : (
					<Typography color="text.secondary" sx={{ mb: 2 }}>
						Danger mode unlocks actions that cannot be undone, act directly on
						production servers, or remove a protection.
					</Typography>
				)}
				<Typography color="text.secondary">
					It lasts ten minutes, then drops back to read-only.
				</Typography>
			</DialogContent>
			<DialogActions>
				<Button color="inherit" onClick={onCancel}>
					Cancel
				</Button>
				<Button
					variant="contained"
					color={danger ? "error" : "warning"}
					onClick={onConfirm}
				>
					{fromControl ? `Continue in ${mode} mode` : "Enter danger mode"}
				</Button>
			</DialogActions>
		</Dialog>
	);
}
