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
import type { Theme } from "@mui/material/styles";
import { useRemainingMs, useSafetyMode } from "../hooks/useSafetyMode";
import { LADDER, type SafetyMode, modeLabel } from "../safety";
import { modePalette, modeStripe, mutedStripe } from "./GradedAction";

/**
 * The mode control's face. Read-only is deliberately unremarkable; a raised
 * mode is filled in its colour and wears its stripe, the same stripe its
 * blocked controls carry, drawn strongly enough to read against the fill.
 */
function faceSx(mode: SafetyMode) {
	return (theme: Theme) => {
		if (mode === "read-only") {
			return {
				border: 1,
				borderColor: "divider",
				color: "text.secondary",
				backgroundColor: "background.paper",
			};
		}
		const palette = theme.palette[modePalette(mode)];
		return {
			border: 1,
			borderColor: palette.main,
			color: palette.contrastText,
			backgroundColor: palette.main,
			backgroundImage: modeStripe(theme, mode, { stripeAlpha: 0.5, gapAlpha: 0 }),
			"&:hover": { backgroundColor: palette.dark },
		};
	};
}

/** What each option in the menu says beneath its name. */
function optionHint(option: SafetyMode, current: SafetyMode): string {
	if (option === current) return "Current";
	switch (option) {
		case "read-only":
			return "Back to reading";
		case "write":
			return "Ten minutes";
		case "danger":
			return "Ten minutes, confirms first";
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
 * Always visible, so the mode is never something to go and check.
 */
export function SafetyModeControl() {
	const { mode, raise, lower, busy } = useSafetyMode();
	const remainingMs = useRemainingMs();
	const [anchor, setAnchor] = useState<null | HTMLElement>(null);
	const [confirming, setConfirming] = useState(false);
	const [failed, setFailed] = useState(false);

	async function to(next: SafetyMode) {
		setAnchor(null);
		setFailed(false);
		try {
			if (next === "read-only") await lower();
			else await raise(next);
		} catch {
			setFailed(true);
		}
	}

	return (
		<>
			<Button
				size="small"
				onClick={(event) => setAnchor(event.currentTarget)}
				disabled={busy}
				sx={[
					faceSx(mode),
					{
						borderRadius: 4,
						textTransform: "none",
						fontWeight: 500,
						gap: 1,
						px: 1.25,
					},
				]}
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
				{LADDER.map((option) => (
					<MenuItem
						key={option}
						selected={option === mode}
						aria-current={option === mode ? "true" : undefined}
						onClick={() => {
							// The menu closes either way: left open behind the
							// confirmation it keeps the rest of the page from the
							// accessibility tree, and from the pointer.
							setAnchor(null);
							if (option === mode) return;
							if (option === "danger") setConfirming(true);
							else to(option);
						}}
						// Each raised mode wears the stripe its blocked controls
						// carry, so the menu is where the operator learns it.
						sx={(theme) => ({
							gap: 1,
							...(option === "read-only"
								? { color: "text.secondary" }
								: {
										color: `${modePalette(option)}.main`,
										...mutedStripe(theme, option),
									}),
						})}
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
							secondary={optionHint(option, mode)}
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

			<Dialog open={failed} onClose={() => setFailed(false)}>
				<DialogTitle>Mode unchanged</DialogTitle>
				<DialogContent>
					<Typography color="text.secondary">
						Could not change mode.
					</Typography>
				</DialogContent>
				<DialogActions>
					<Button onClick={() => setFailed(false)}>Close</Button>
				</DialogActions>
			</Dialog>
		</>
	);
}
