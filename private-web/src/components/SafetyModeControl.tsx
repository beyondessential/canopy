import {
	Box,
	ButtonBase,
	ClickAwayListener,
	Collapse,
	ListItemText,
	Menu,
	MenuItem,
	MenuList,
} from "@mui/material";
import ExpandLessIcon from "@mui/icons-material/ExpandLess";
import ExpandMoreIcon from "@mui/icons-material/ExpandMore";
import { useState } from "react";
import type { Theme } from "@mui/material/styles";
import { useRemainingMs, useSafetyMode } from "../hooks/useSafetyMode";
import { LADDER, type SafetyMode, modeLabel } from "../safety";
import { modePalette, modeStripe, mutedStripe } from "./GradedAction";

/**
 * The mode control's face. Read-only is deliberately unremarkable, the chrome
 * it sits in showing through; a raised mode is filled in its colour and wears
 * its stripe, the same stripe its blocked controls carry, drawn strongly enough
 * to read against the fill.
 */
function faceSx(mode: SafetyMode) {
	return (theme: Theme) => {
		if (mode === "read-only") {
			return {
				borderColor: "divider",
				color: "text.secondary",
				backgroundColor: "transparent",
				"&:hover": { backgroundColor: theme.palette.action.hover },
			};
		}
		const palette = theme.palette[modePalette(mode)];
		return {
			borderColor: palette.main,
			color: palette.contrastText,
			backgroundColor: palette.main,
			backgroundImage: modeStripe(theme, mode, { stripeAlpha: 0.5, gapAlpha: 0 }),
			"&:hover": { backgroundColor: palette.dark },
		};
	};
}

/**
 * The control as a square band filling the edge of the chrome it sits on,
 * divided from the rest of that chrome along `edge`.
 */
function bandSx(mode: SafetyMode, edge: "borderTop" | "borderLeft") {
	return (theme: Theme) => ({
		...faceSx(mode)(theme),
		minHeight: 48,
		justifyContent: "flex-start",
		gap: 1,
		border: 0,
		[edge]: 1,
		borderRadius: 0,
		...theme.typography.button,
		textTransform: "none" as const,
	});
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

function ModeDot() {
	return (
		<Box
			sx={{
				flexShrink: 0,
				width: 8,
				height: 8,
				borderRadius: "50%",
				backgroundColor: "currentColor",
			}}
		/>
	);
}

/**
 * The mode's name and the time left on a raise, as the control shows them.
 * Spread, the name takes the room between them.
 */
function ModeFace({ mode, spread }: { mode: SafetyMode; spread?: boolean }) {
	const remainingMs = useRemainingMs();
	return (
		<>
			<ModeDot />
			{spread ? (
				<Box component="span" sx={{ flex: 1, textAlign: "left" }}>
					{modeLabel(mode)}
				</Box>
			) : (
				modeLabel(mode)
			)}
			{remainingMs !== null && (
				<Box
					component="span"
					sx={{ fontVariantNumeric: "tabular-nums", opacity: 0.75 }}
				>
					{remaining(remainingMs)}
				</Box>
			)}
		</>
	);
}

/**
 * One item per mode, for whichever list the control opens. Choosing closes the
 * list either way: left open behind the danger confirmation it keeps the rest
 * of the page from the accessibility tree, and from the pointer.
 *
 * Returned as elements rather than rendered as a component, so the items are
 * the list's own children and it can focus the current one on opening.
 */
function useModeOptions(onClose: () => void, inset?: number) {
	const { mode, raise, lower, requestRaise } = useSafetyMode();
	return LADDER.map((option) => (
		<MenuItem
			key={option}
			selected={option === mode}
			aria-current={option === mode ? "true" : undefined}
			onClick={() => {
				onClose();
				if (option === mode) return;
				if (option === "danger") requestRaise({ mode: "danger" });
				else if (option === "write") raise("write");
				else lower();
			}}
			// Each raised mode wears the stripe its blocked controls
			// carry, so the menu is where the operator learns it.
			sx={(theme) => ({
				gap: 1,
				...(inset !== undefined && { pl: inset }),
				...(option === "read-only"
					? { color: "text.secondary" }
					: {
							color: `${modePalette(option)}.main`,
							...mutedStripe(theme, option),
						}),
			})}
		>
			<ModeDot />
			<ListItemText
				primary={modeLabel(option)}
				secondary={optionHint(option, mode)}
			/>
		</MenuItem>
	));
}

/** How wide the top bar's control is, and the menu it drops. */
const BAR_WIDTH = 240;

/**
 * The operator's mode, and how they change it, as a block at the end of a top
 * bar, dropping its options at its own width.
 *
 * Always visible, so the mode is never something to go and check.
 */
export function SafetyModeControl() {
	const { mode, busy } = useSafetyMode();
	const [anchor, setAnchor] = useState<null | HTMLElement>(null);
	const options = useModeOptions(() => setAnchor(null), 2);

	return (
		<>
			<ButtonBase
				onClick={(event) => setAnchor(event.currentTarget)}
				disabled={busy}
				aria-haspopup="menu"
				aria-expanded={anchor !== null}
				sx={[
					bandSx(mode, "borderLeft"),
					{ alignSelf: "stretch", width: BAR_WIDTH, pl: 2, pr: 1.5 },
				]}
			>
				<ModeFace mode={mode} spread />
				{anchor ? (
					<ExpandLessIcon fontSize="small" />
				) : (
					<ExpandMoreIcon fontSize="small" />
				)}
			</ButtonBase>

			<Menu
				anchorEl={anchor}
				open={anchor !== null}
				onClose={() => setAnchor(null)}
				marginThreshold={0}
				anchorOrigin={{ vertical: "bottom", horizontal: "right" }}
				transformOrigin={{ vertical: "top", horizontal: "right" }}
				slotProps={{
					paper: { square: true, sx: { width: BAR_WIDTH } },
					list: { disablePadding: true },
				}}
			>
				{options}
			</Menu>
		</>
	);
}

/**
 * The operator's mode as a band across the foot of the sidebar. Its options
 * open in place above it rather than over the page, the list above giving up
 * the room.
 */
export function SafetyModeBand() {
	const { mode, busy } = useSafetyMode();
	const [open, setOpen] = useState(false);
	const close = () => setOpen(false);
	const options = useModeOptions(close, 3);

	return (
		<ClickAwayListener onClickAway={close}>
			<Box
				sx={{ flexShrink: 0 }}
				onKeyDown={(event) => {
					if (event.key === "Escape") close();
				}}
			>
				<Collapse in={open} unmountOnExit>
					<MenuList
						autoFocusItem={open}
						disablePadding
						sx={{ borderTop: 1, borderColor: "divider" }}
					>
						{options}
					</MenuList>
				</Collapse>
				<ButtonBase
					onClick={() => setOpen((was) => !was)}
					disabled={busy}
					aria-haspopup="menu"
					aria-expanded={open}
					sx={[
						bandSx(mode, "borderTop"),
						{ width: "100%", pl: 3, pr: 2 },
					]}
				>
					<ModeFace mode={mode} spread />
					{open ? <ExpandMoreIcon fontSize="small" /> : <ExpandLessIcon fontSize="small" />}
				</ButtonBase>
			</Box>
		</ClickAwayListener>
	);
}
