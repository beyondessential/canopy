import WarningAmberIcon from "@mui/icons-material/WarningAmber";
import {
	Box,
	Button,
	Dialog,
	DialogActions,
	DialogContent,
	DialogTitle,
	ListItemText,
	Menu,
	MenuItem,
	Typography,
} from "@mui/material";
import { useState } from "react";
import { ApiError } from "../api";
import { useSafetyMode } from "../hooks/useSafetyMode";
import { type SafetyMode, modeLabel, refusalOf } from "../safety";

/** The palette each mode reads in. Read-only is deliberately unremarkable. */
export function modeColour(mode: SafetyMode): {
	main: string;
	border: string;
	background: string;
} {
	switch (mode) {
		case "danger":
			return {
				main: "error.main",
				border: "error.main",
				background: "rgba(239,83,80,0.10)",
			};
		case "write":
			return {
				main: "warning.main",
				border: "warning.main",
				background: "rgba(255,152,0,0.08)",
			};
		case "read-only":
			return {
				main: "text.secondary",
				border: "divider",
				background: "background.paper",
			};
	}
}

/** `m:ss` left on the raise. Counts down from ten minutes. */
function remaining(ms: number): string {
	const total = Math.ceil(ms / 1000);
	const minutes = Math.floor(total / 60);
	const seconds = total % 60;
	return `${minutes}:${seconds.toString().padStart(2, "0")}`;
}

/**
 * The operator's mode, and how they change it.
 *
 * Always visible, so the mode is never something to go and check. Danger is
 * offered to every operator: nothing tells the client in advance whether its
 * operator holds the permission, so an operator who does not is told when they
 * try rather than being quietly shown a shorter menu.
 */
export function SafetyModeControl() {
	const { mode, remainingMs, raise, lower, busy } = useSafetyMode();
	const [anchor, setAnchor] = useState<null | HTMLElement>(null);
	const [confirming, setConfirming] = useState(false);
	const [refusal, setRefusal] = useState<string | null>(null);

	const colour = modeColour(mode);

	async function to(next: SafetyMode) {
		setAnchor(null);
		setRefusal(null);
		try {
			if (next === "read-only") await lower();
			else await raise(next);
		} catch (error) {
			// An operator without the danger permission is told they lack it,
			// rather than that something went wrong.
			const detail = error instanceof ApiError ? error.detail : null;
			setRefusal(
				refusalOf(detail) === "permission"
					? "You do not hold the danger permission."
					: "Could not change mode.",
			);
		}
	}

	return (
		<>
			<Button
				size="small"
				onClick={(event) => setAnchor(event.currentTarget)}
				disabled={busy}
				sx={{
					borderRadius: 4,
					border: 1,
					borderColor: colour.border,
					color: colour.main,
					backgroundColor: colour.background,
					textTransform: "none",
					fontWeight: 500,
					gap: 1,
					px: 1.25,
				}}
			>
				<Box
					sx={{
						width: 8,
						height: 8,
						borderRadius: "50%",
						backgroundColor: "currentColor",
					}}
				/>
				{modeLabel(mode)}
				{remainingMs !== null && (
					<Box
						component="span"
						sx={{ fontVariantNumeric: "tabular-nums", opacity: 0.75 }}
					>
						{remaining(remainingMs)}
					</Box>
				)}
			</Button>

			<Menu
				anchorEl={anchor}
				open={anchor !== null}
				onClose={() => setAnchor(null)}
			>
				{(["read-only", "write", "danger"] as const).map((option) => (
					<MenuItem
						key={option}
						onClick={() => {
							// The menu closes either way: left open behind the
							// confirmation it keeps the rest of the page from the
							// accessibility tree, and from the pointer.
							setAnchor(null);
							if (option === "danger") setConfirming(true);
							else to(option);
						}}
						disabled={option === mode}
						sx={{ color: modeColour(option).main, gap: 1 }}
					>
						<Box
							sx={{
								width: 8,
								height: 8,
								borderRadius: "50%",
								backgroundColor: "currentColor",
							}}
						/>
						<ListItemText
							primary={modeLabel(option)}
							secondary={
								option === mode
									? "Current"
									: option === "read-only"
										? "Back to reading"
										: option === "write"
											? "Ten minutes"
											: "Ten minutes, confirms first"
							}
						/>
					</MenuItem>
				))}
			</Menu>

			<Dialog open={confirming} onClose={() => setConfirming(false)}>
				<DialogTitle
					sx={{
						display: "flex",
						alignItems: "center",
						gap: 1.25,
						color: "error.main",
					}}
				>
					<WarningAmberIcon />
					Enter danger mode?
				</DialogTitle>
				<DialogContent>
					<Typography color="text.secondary" sx={{ mb: 2 }}>
						Danger mode unlocks actions that cannot be undone, act directly on
						production servers, or remove a protection.
					</Typography>
					<Typography color="text.secondary">
						It lasts ten minutes, then drops back to read-only.
					</Typography>
				</DialogContent>
				<DialogActions>
					<Button color="inherit" onClick={() => setConfirming(false)}>
						Cancel
					</Button>
					<Button
						variant="contained"
						color="error"
						onClick={() => {
							setConfirming(false);
							to("danger");
						}}
					>
						Enter danger mode
					</Button>
				</DialogActions>
			</Dialog>

			<Dialog open={refusal !== null} onClose={() => setRefusal(null)}>
				<DialogTitle>Mode unchanged</DialogTitle>
				<DialogContent>
					<Typography color="text.secondary">{refusal}</Typography>
				</DialogContent>
				<DialogActions>
					<Button onClick={() => setRefusal(null)}>Close</Button>
				</DialogActions>
			</Dialog>
		</>
	);
}
