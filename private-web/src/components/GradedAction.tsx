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
 * The palette a usable control needing `required` is drawn in, or nothing for
 * one that needs no mode and keeps its own (see {@link inGradeColour}).
 */
export function gradeColour(
	required: SafetyMode,
): "warning" | "error" | undefined {
	return required === "read-only" ? undefined : modePalette(required);
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
 * a form, dialog, or popover is worth opening as soon as any one of the changes
 * it can make is within reach.
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

/** What the current mode means for a control needing `required`. */
function useModeGrade(required: SafetyMode): {
	required: SafetyMode;
	blocked: boolean;
} {
	const { mode } = useSafetyMode();
	return { required, blocked: !permits(mode, required) };
}

/** What the current mode means for a control calling these endpoints. */
export function useGrade(calls: Calls) {
	return useModeGrade(requiredMode(calls));
}

/**
 * The endpoints a control calls, or those the form it opens can call.
 *
 * A control that makes the calls itself needs the highest of their grades. One
 * that opens a form, dialog, or popover for making them needs the lowest, so it
 * is blocked exactly when nothing inside could be submitted either.
 */
export type Grading = { calls: Calls; opens?: never } | { opens: Calls; calls?: never };

/** The mode a control graded by {@link Grading} needs. */
function gradingMode(grading: Grading): SafetyMode {
	return grading.opens !== undefined
		? lowestMode(grading.opens)
		: requiredMode(grading.calls);
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

/** The props a graded control is handed. */
interface GradedChildProps {
	disabled?: boolean;
	color?: string;
	children?: ReactNode;
}

/**
 * A usable control in its grade's colour, so what it takes to use a control is
 * readable from the control itself, in the same colour as its stripe when it
 * is blocked and as the mode that permits it. The grade's colour wins over any
 * the control names for itself: red means danger wherever it appears. A
 * control that needs no mode, such as a toggle standing as its "Cancel", keeps
 * its own.
 */
function inGradeColour(
	control: ReactElement<GradedChildProps>,
	required: SafetyMode,
): ReactElement<GradedChildProps> {
	const colour = gradeColour(required);
	if (!colour) return control;
	if (control.type === Tooltip) {
		return cloneElement(control, {
			children: inGradeColour(
				control.props.children as ReactElement<GradedChildProps>,
				required,
			),
		});
	}
	return cloneElement(control, { color: colour });
}

type GradedActionProps = Grading & {
	/**
	 * The control itself. When the operator's mode reaches it, it is given its
	 * grade's `color` (see {@link inGradeColour}); while blocked it is given
	 * `disabled`. It must accept both, or be a tooltip around one that does.
	 */
	children: ReactElement<GradedChildProps>;
	/** Stretch to the width available, for a control that is itself full width. */
	fullWidth?: boolean;
	/** Shown instead of the default tooltip while blocked. */
	title?: ReactNode;
};

/**
 * Wraps a control in the safety mode its endpoints require: `calls` for a
 * control that makes the change, `opens` for one that opens the form for it.
 *
 * A control the operator could use in a higher mode stays where it is and is
 * blocked, so the surface has the same shape whatever mode they are in. Blocked
 * means it does not act and says which mode it wants; raising is done from the
 * mode control, never as a by-product of reaching for a blocked control.
 *
 * When the mode reaches the control it is rendered in its grade's colour and
 * otherwise as given, so a control disabled for its own reasons (a request in
 * flight, an incomplete form) carries no stripe.
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
	children,
	fullWidth,
	title,
	...grading
}: GradedActionProps) {
	const { required, blocked } = useModeGrade(gradingMode(grading));
	if (!blocked) return inGradeColour(children, required);

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

type GradedMenuItemProps = MenuItemProps & Grading;

/**
 * A menu item graded like {@link GradedAction}.
 *
 * Kept a direct child of its menu, which is what the menu's keyboard handling
 * expects, so it cannot be wrapped: blocked, it keeps its place, carries the
 * stripe, and ignores being chosen.
 */
export function GradedMenuItem(item: GradedMenuItemProps) {
	const { calls: _calls, opens: _opens, onClick, sx, ...props } = item;
	const { required, blocked } = useModeGrade(gradingMode(item));
	if (!blocked) {
		return (
			<MenuItem
				onClick={onClick}
				sx={[
					!!gradeColour(required) && {
						color: `${gradeColour(required)}.main`,
					},
					...(Array.isArray(sx) ? sx : sx ? [sx] : []),
				]}
				{...props}
			/>
		);
	}

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
