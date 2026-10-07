import {
	Button,
	Dialog,
	DialogActions,
	DialogContent,
	DialogTitle,
	Typography,
} from "@mui/material";

/** Says a raise or lower did not take, so the operator knows their mode stands. */
export function ModeUnchangedDialog({
	open,
	onClose,
}: {
	open: boolean;
	onClose: () => void;
}) {
	return (
		<Dialog open={open} onClose={onClose}>
			<DialogTitle>Mode unchanged</DialogTitle>
			<DialogContent>
				<Typography color="text.secondary">Could not change mode.</Typography>
			</DialogContent>
			<DialogActions>
				<Button onClick={onClose}>Close</Button>
			</DialogActions>
		</Dialog>
	);
}
