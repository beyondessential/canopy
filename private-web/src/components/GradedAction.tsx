import { Box, Tooltip } from "@mui/material";
import type { ReactElement, ReactNode } from "react";
import { useSafetyMode } from "../hooks/useSafetyMode";
import { SAFETY_MODES } from "../safety-modes";
import { type SafetyMode, modeLabel, permits } from "../safety";

/** The diagonal stripes a blocked control carries, one per grade. */
const STRIPES: Record<Exclude<SafetyMode, "read-only">, string> = {
	write:
		"repeating-linear-gradient(135deg, rgba(255,152,0,0.24), rgba(255,152,0,0.24) 6px, rgba(255,152,0,0.07) 6px, rgba(255,152,0,0.07) 12px)",
	danger:
		"repeating-linear-gradient(45deg, rgba(239,83,80,0.24), rgba(239,83,80,0.24) 6px, rgba(239,83,80,0.07) 6px, rgba(239,83,80,0.07) 12px)",
};

/** The mode an endpoint requires, from the generated map. */
export function modeFor(module: string, fn: string): SafetyMode {
	return SAFETY_MODES[`${module}/${fn}`] ?? "read-only";
}

interface GradedActionProps {
	/** The endpoint this control calls, as `callApi` addresses it. */
	module: string;
	fn: string;
	/** The control itself. Rendered as-is when the operator's mode reaches it. */
	children: ReactElement;
	/** Shown instead of the default tooltip while blocked. */
	title?: ReactNode;
}

/**
 * Wraps a control in the safety mode its endpoint requires.
 *
 * A control the operator could use in a higher mode stays where it is and is
 * blocked, so the surface has the same shape whatever mode they are in. Blocked
 * means it does not act and says which mode it wants; raising is done from the
 * mode control, never as a by-product of reaching for a blocked control.
 *
 * Nothing here decides anything — the server refuses the request regardless.
 * This is what stops an operator finding that out by being refused.
 */
export function GradedAction({
	module,
	fn,
	children,
	title,
}: GradedActionProps) {
	const { mode } = useSafetyMode();
	const required = modeFor(module, fn);

	if (permits(mode, required)) return children;
	// Read-only is reachable from every mode, so this is always write or danger.
	const stripe = STRIPES[required as Exclude<SafetyMode, "read-only">];

	return (
		<Tooltip title={title ?? `Requires ${modeLabel(required).toLowerCase()} mode`}>
			{/* The wrapper carries the cursor and the tooltip; the control inside
			    takes no pointer events, so a click never reaches its handler. */}
			<Box
				component="span"
				sx={{
					display: "inline-flex",
					cursor: "not-allowed",
					"& > *": {
						pointerEvents: "none",
						backgroundImage: stripe,
						filter: "grayscale(0.8)",
						transition: "filter 150ms cubic-bezier(.4,0,.2,1)",
					},
					"&:hover > *": { filter: "grayscale(0)" },
				}}
			>
				{children}
			</Box>
		</Tooltip>
	);
}
