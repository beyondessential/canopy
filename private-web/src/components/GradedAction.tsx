import { Box, MenuItem, type MenuItemProps, Tooltip } from "@mui/material";
import { type Theme, alpha } from "@mui/material/styles";
import { type ReactElement, type ReactNode, cloneElement } from "react";
import { useSafetyMode } from "../hooks/useSafetyMode";
import { LADDER, type SafetyMode, modeLabel, permits } from "../safety";
import { type GradedEndpoint, SAFETY_MODES } from "../safety-modes";

/** A mode above read-only: one that has a stripe. */
export type RaisedMode = Exclude<SafetyMode, "read-only">;

/** The palette a raised mode is drawn in, wherever it appears. */
export function modePalette(mode: RaisedMode): "warning" | "error" {
	return mode === "write" ? "warning" : "error";
}

/**
 * The diagonal stripe a mode is drawn with, in its palette colour. Worn by a
 * blocked control, by the mode control while raised, and by each raised mode
 * in its menu, so the operator learns one treatment everywhere. The angle
 * differs too, so the two read apart from the pattern alone.
 */
export function modeStripe(
	theme: Theme,
	mode: RaisedMode,
	opts?: { stripeAlpha?: number; gapAlpha?: number },
): string {
	const colour = theme.palette[modePalette(mode)].light;
	const angle = mode === "write" ? "135deg" : "45deg";
	const stripe = alpha(colour, opts?.stripeAlpha ?? 0.24);
	const gap = alpha(colour, opts?.gapAlpha ?? 0.07);
	return `repeating-linear-gradient(${angle}, ${stripe}, ${stripe} 6px, ${gap} 6px, ${gap} 12px)`;
}

/**
 * The endpoint, or endpoints, a control calls.
 *
 * An empty list is a control that calls nothing as it stands, which is how a
 * toggle that becomes a plain "Cancel" says it needs no mode for that. It reads
 * as read-only, so such a control is never blocked.
 */
export type Calls = GradedEndpoint | readonly GradedEndpoint[];

/**
 * The mode a control needs: the highest grade among the endpoints it calls. A
 * form whose save calls a write endpoint and, depending on what was filled in,
 * a danger one needs danger whenever it would make the danger call, so pass
 * only the calls this submission would make.
 */
export function requiredMode(calls: Calls): SafetyMode {
	return modesOf(calls).reduce(
		(highest, mode) =>
			LADDER.indexOf(mode) > LADDER.indexOf(highest) ? mode : highest,
		"read-only" as SafetyMode,
	);
}

/**
 * The lowest grade among the endpoints, for a control that only leads to them:
 * a popover whose rows are each graded on their own is reachable as soon as any
 * one of them is.
 */
export function lowestMode(calls: Calls): SafetyMode {
	const modes = modesOf(calls);
	return modes.reduce(
		(lowest, mode) =>
			LADDER.indexOf(mode) < LADDER.indexOf(lowest) ? mode : lowest,
		modes[0] ?? ("read-only" as SafetyMode),
	);
}

/** The grade each of these endpoints requires. */
function modesOf(calls: Calls): SafetyMode[] {
	const list: readonly GradedEndpoint[] =
		typeof calls === "string" ? [calls] : calls;
	return list.map((call) => SAFETY_MODES[call]);
}

/** What the current mode means for a control calling these endpoints. */
export function useGrade(calls: Calls): {
	required: SafetyMode;
	blocked: boolean;
} {
	const { mode } = useSafetyMode();
	const required = requiredMode(calls);
	return { required, blocked: !permits(mode, required) };
}

/** What a blocked control says about itself: the mode it needs. */
export function blockedTitle(required: SafetyMode): string {
	return `Requires ${modeLabel(required).toLowerCase()} mode`;
}

/**
 * A mode's stripe, muted at rest and coming to full colour under the pointer
 * or while `&.Mui-selected`. The blocked treatment, and the mode menu's.
 */
export function mutedStripe(theme: Theme, mode: RaisedMode) {
	const stripe = modeStripe(theme, mode);
	return {
		backgroundImage: stripe,
		filter: "grayscale(0.8)",
		transition: theme.transitions.create("filter", {
			duration: theme.transitions.duration.shortest,
		}),
		"&:hover, &.Mui-selected, &.Mui-selected:hover": {
			filter: "grayscale(0)",
			backgroundImage: stripe,
		},
	};
}

/** The blocked treatment's styles, for nesting inside another `sx`. */
function blockedStyles(theme: Theme, required: SafetyMode) {
	return {
		cursor: "not-allowed",
		...mutedStripe(theme, required as RaisedMode),
	};
}

/**
 * The blocked treatment, for a control that cannot take the
 * {@link GradedAction} wrapper: the grade's stripe, muted at rest and coming to
 * full colour under the pointer. The control itself has to ignore activation.
 */
export function blockedSx(required: SafetyMode) {
	return (theme: Theme) => blockedStyles(theme, required);
}

interface GradedActionProps {
	/** The endpoint, or endpoints, this control calls. */
	calls: Calls;
	/**
	 * The control itself. Rendered untouched when the operator's mode reaches
	 * it; while blocked it is given `disabled`, so it must accept that prop.
	 */
	children: ReactElement<{ disabled?: boolean }>;
	/** Stretch to the width available, for a control that is itself full width. */
	fullWidth?: boolean;
	/** Shown instead of the default tooltip while blocked. */
	title?: ReactNode;
}

/**
 * Wraps a control in the safety mode its endpoints require.
 *
 * A control the operator could use in a higher mode stays where it is and is
 * blocked, so the surface has the same shape whatever mode they are in. Blocked
 * means it does not act and says which mode it wants; raising is done from the
 * mode control, never as a by-product of reaching for a blocked control.
 *
 * When the mode reaches the control it is rendered exactly as given, so a
 * control disabled for its own reasons (a request in flight, an incomplete
 * form) carries no stripe.
 *
 * A blocked control is disabled as well as inert to the pointer, so it cannot
 * be reached from the keyboard. A child that is a tooltip passes that on to the
 * control inside, except where that control names `disabled` itself: a tooltip
 * keeps its child's own props, so such a control has to name the blocked state
 * as well (see `MachineSetupInstructions`).
 *
 * Nothing here decides anything — the server refuses the request regardless.
 * This is what stops an operator finding that out by being refused.
 */
export function GradedAction({
	calls,
	children,
	fullWidth,
	title,
}: GradedActionProps) {
	const { required, blocked } = useGrade(calls);
	if (!blocked) return children;

	return (
		<Tooltip title={title ?? blockedTitle(required)}>
			{/* The wrapper carries the cursor and the tooltip. The control inside
			    takes no pointer events, so a click never reaches its handler, and
			    is disabled, so it cannot be reached from the keyboard either and
			    does not submit its form when Enter is pressed in a field. */}
			<Box
				component="span"
				aria-disabled
				sx={(theme) => ({
					display: fullWidth ? "flex" : "inline-flex",
					width: fullWidth ? "100%" : undefined,
					cursor: "not-allowed",
					// The same treatment a control that cannot take the wrapper
					// applies to itself, so the convention is written once.
					"& > *": { pointerEvents: "none", ...blockedStyles(theme, required) },
					"&:hover > *": { filter: "grayscale(0)" },
				})}
			>
				{cloneElement(children, { disabled: true })}
			</Box>
		</Tooltip>
	);
}

interface GradedMenuItemProps extends MenuItemProps {
	/** The endpoint, or endpoints, choosing this item calls. */
	calls: Calls;
}

/**
 * A menu item graded like {@link GradedAction}.
 *
 * Kept a direct child of its menu, which is what the menu's keyboard handling
 * expects, so it cannot be wrapped: blocked, it keeps its place, carries the
 * stripe, and ignores being chosen.
 */
export function GradedMenuItem({
	calls,
	onClick,
	sx,
	...props
}: GradedMenuItemProps) {
	const { required, blocked } = useGrade(calls);
	if (!blocked) return <MenuItem onClick={onClick} sx={sx} {...props} />;

	return (
		<Tooltip title={blockedTitle(required)} placement="left">
			<MenuItem
				{...props}
				aria-disabled
				onClick={(event) => event.preventDefault()}
				sx={[blockedSx(required), ...(Array.isArray(sx) ? sx : sx ? [sx] : [])]}
			/>
		</Tooltip>
	);
}
