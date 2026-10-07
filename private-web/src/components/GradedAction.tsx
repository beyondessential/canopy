import { Box, Tooltip } from "@mui/material";
import { type Theme, alpha } from "@mui/material/styles";
import {
	type ReactElement,
	type ReactNode,
	type SyntheticEvent,
	cloneElement,
} from "react";
import { type RaiseOutcome, useSafetyMode } from "../hooks/useSafetyMode";
import { inRaiseDialog } from "./RaiseDialog";
import {
	LADDER,
	type RaisedMode,
	type SafetyMode,
	modeLabel,
	permits,
} from "../safety";
import {
	DANGER_REASONS,
	type DangerReason,
	type GradedEndpoint,
	SAFETY_MODES,
} from "../safety-modes";

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
export type Grading = (
	| { calls: Calls; opens?: never }
	| { opens: Calls; calls?: never }
) & {
	/**
	 * What the control does and to what, such as "Revoke certificate for
	 * host-3". It titles the raise offered when the control is blocked, so it
	 * names the object where the visible label does not.
	 */
	action: string;
};

/** The mode a control graded by {@link Grading} needs. */
function gradingMode(grading: Grading): SafetyMode {
	return grading.opens !== undefined
		? lowestMode(grading.opens)
		: requiredMode(grading.calls);
}

/** The endpoints a grading names, whichever way it names them. */
function endpointsOf(grading: Grading): readonly GradedEndpoint[] {
	const calls = grading.opens !== undefined ? grading.opens : grading.calls;
	return typeof calls === "string" ? [calls] : calls;
}

/**
 * Why a control needing `required` needs it, when that is danger: the reasons
 * declared by the danger endpoints it calls, each once.
 */
export function dangerReasons(
	grading: Grading,
	required: SafetyMode,
): DangerReason[] {
	if (required !== "danger") return [];
	const declared = DANGER_REASONS as Partial<
		Record<GradedEndpoint, readonly DangerReason[]>
	>;
	const reasons = new Set<DangerReason>();
	for (const endpoint of endpointsOf(grading)) {
		if (SAFETY_MODES[endpoint] !== "danger") continue;
		for (const reason of declared[endpoint] ?? []) reasons.add(reason);
	}
	return [...reasons];
}

/**
 * A control graded by `grading`, for one that cannot take the
 * {@link GradedAction} wrapper: whether it is blocked, and `activate`, which
 * runs what the control does at once when it is usable and otherwise asks the
 * operator to raise first, running it only if they do. A control that stands
 * for several things, such as one per row, gives `activate` the `action` of the
 * one activated.
 */
export function useGradedActivation(grading: Grading) {
	const { requestRaise } = useSafetyMode();
	const { required, blocked } = useModeGrade(gradingMode(grading));

	const activate = async (
		run: () => void,
		action = grading.action,
	): Promise<RaiseOutcome> => {
		if (!blocked) {
			run();
			return "permits";
		}
		const outcome = await requestRaise({
			mode: required as RaisedMode,
			action,
			reasons: dangerReasons(grading, required),
		});
		if (outcome === "permits") run();
		return outcome;
	};
	return { required, blocked, activate };
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
		cursor: "pointer",
		...mutedStripe(theme, required as RaisedMode),
	};
}

/**
 * The blocked treatment, for a control that cannot take the
 * {@link GradedAction} wrapper: the grade's stripe, muted at rest and coming to
 * full colour under the pointer. The control itself asks for the raise when
 * activated (see {@link useGradedActivation}).
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

/**
 * The control a graded child stands for: the child itself, or the one inside
 * the tooltip that is the child.
 */
function controlOf(
	child: ReactElement<GradedChildProps>,
): ReactElement<GradedChildProps> {
	return child.type === Tooltip
		? controlOf(child.props.children as ReactElement<GradedChildProps>)
		: child;
}

/** Whether the control is disabled for a reason of its own. */
function disabledItself(child: ReactElement<GradedChildProps>): boolean {
	return !!controlOf(child).props.disabled;
}

/** What an operator can activate inside a graded control. */
const ACTIVATABLE = [
	"button",
	"a[href]",
	"input",
	"select",
	"textarea",
	"label",
	"summary",
	...[
		"button",
		"link",
		"checkbox",
		"switch",
		"radio",
		"tab",
		"option",
		"menuitem",
		"combobox",
	].map((role) => `[role=${role}]`),
].join(", ");

/** The keys that open a combobox. */
const OPENING_KEYS = [" ", "Enter", "ArrowUp", "ArrowDown"];

/**
 * The element inside `wrapper` that an event activated: the nearest activatable
 * one to where it landed, such as the one button of a group that was pressed.
 * None where it landed on something that does nothing, such as the space
 * between a group's buttons, which is no reason to ask for a raise.
 */
function activatedWithin(
	wrapper: HTMLElement,
	target: EventTarget | null,
): HTMLElement | null {
	const hit =
		target instanceof Element ? target.closest<HTMLElement>(ACTIVATABLE) : null;
	return hit && wrapper.contains(hit) ? hit : null;
}

/** Whether `target` is in a combobox, which opens on press rather than click. */
function inCombobox(target: EventTarget | null): boolean {
	return target instanceof Element && !!target.closest("[role=combobox]");
}

/**
 * Carry out on `control` the activation that asked for a raise.
 *
 * The control is the element the operator activated, still in place: the
 * wrapper is kept whether the control is blocked or not, so a raise redraws the
 * control rather than replacing it. A combobox opens on press, so it is pressed;
 * anything else is clicked. Focus is returned to the control unless the
 * activation moved it somewhere of its own, such as a dialog it opened.
 */
function replay(control: HTMLElement): void {
	if (!control.isConnected) return;
	if (control.getAttribute("role") === "combobox") {
		control.dispatchEvent(
			new MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0 }),
		);
	} else {
		control.click();
	}
	returnFocus(control);
}

/**
 * Return focus to `control`, unless something else has taken it since, such as
 * a dialog the activation opened.
 */
function returnFocus(control: HTMLElement): void {
	requestAnimationFrame(() => {
		const active = document.activeElement;
		const lost =
			!active ||
			active === document.body ||
			inRaiseDialog(active);
		if (lost && control.isConnected) control.focus();
	});
}

type GradedActionProps = Grading & {
	/**
	 * The control itself. When the operator's mode reaches it, it is given its
	 * grade's `color` (see {@link inGradeColour}). It must accept that, or be a
	 * tooltip around one that does.
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
 * means it carries the grade's stripe, says which mode it wants, and when
 * activated asks the operator to raise to that mode and then carries the
 * activation out as it would have been carried out unblocked. A raise is only
 * ever made by the operator choosing to continue, never by the activation alone.
 *
 * The activation is intercepted at the wrapper rather than inside the control,
 * so a click, Enter or Space on the control, Enter in a field of its form, and
 * the press that opens a combobox, are all caught whatever the control is and
 * without touching its handlers. The wrapper stays in place once the control is
 * usable, so the raise redraws the control rather than replacing it, and the
 * activation is carried out on the very element the operator activated.
 *
 * When the mode reaches the control it is rendered in its grade's colour and
 * otherwise as given. A control disabled for a reason of its own (a request in
 * flight, an incomplete form) is rendered that way in every mode: it carries no
 * stripe and offers no raise, so the treatment never misreports why a control
 * is unavailable. A tooltip child passes this through to the control inside.
 *
 * Nothing here decides anything: the server refuses the request regardless.
 * This is what stops an operator finding that out by being refused.
 */
export function GradedAction({
	children,
	fullWidth,
	title,
	...grading
}: GradedActionProps) {
	const { required, blocked, activate } = useGradedActivation(grading);
	const usable = !blocked || disabledItself(children);

	const intercept = (event: SyntheticEvent<HTMLElement>) => {
		event.preventDefault();
		event.stopPropagation();
		const control = activatedWithin(event.currentTarget, event.target);
		if (!control) return;
		activate(() => replay(control)).then((outcome) => {
			if (outcome === "declined") returnFocus(control);
		});
	};

	return (
		<Tooltip title={usable ? null : (title ?? blockedTitle(required))}>
			{/* The wrapper carries the cursor, the tooltip, and the interception.
			    The control inside is left as it is, so it can be focused and
			    activated like any other; capturing the activation here stops its
			    own handler, or its form's submission, until the raise is made. */}
			<Box
				component="span"
				onClickCapture={
					usable
						? undefined
						: (event) => {
								// A combobox was already intercepted on the press that opens it.
								if (inCombobox(event.target)) {
									event.preventDefault();
									event.stopPropagation();
								} else {
									intercept(event);
								}
							}
				}
				// A middle click opens a link in a new tab, as a click would have
				// followed it here, so it asks for the raise the same way.
				onAuxClickCapture={
					usable
						? undefined
						: (event) => {
								if (event.button === 1) intercept(event);
							}
				}
				onMouseDownCapture={
					usable
						? undefined
						: (event) => {
								if (event.button === 0 && inCombobox(event.target)) intercept(event);
							}
				}
				onKeyDownCapture={
					usable
						? undefined
						: (event) => {
								if (OPENING_KEYS.includes(event.key) && inCombobox(event.target)) {
									intercept(event);
								}
							}
				}
				sx={(theme) => ({
					display: fullWidth ? "flex" : "inline-flex",
					width: fullWidth ? "100%" : undefined,
					...(!usable && {
						cursor: "pointer",
						// The same treatment a control that cannot take the wrapper
						// applies to itself, so the convention is written once.
						"& > *": blockedStyles(theme, required),
						"&:hover > *": { filter: "grayscale(0)" },
					}),
				})}
			>
				{usable ? inGradeColour(children, required) : children}
			</Box>
		</Tooltip>
	);
}
