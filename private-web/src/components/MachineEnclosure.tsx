import { Box, Tooltip, type Theme } from "@mui/material";
import { alpha } from "@mui/material/styles";
import type { ReactNode } from "react";
import type { HealthState, ShortStatus } from "../types";

// The pill is the machine, so it carries the machine's state while the dots
// inside carry each application's. Every machine is enclosed, whether it runs
// one application or several: an enclosure never means anything on its own,
// only its contents do. A box hosting two workloads is one pill with two dots,
// which is the whole point of the grain.
//
// The pill is drawn as an outline with a wash rather than a solid fill, so the
// dots inside stay the loudest thing in it. A machine's state is context for
// its applications', not a competitor to it.
//
// Orange is the enclosure's alone, so each hue means one thing: light green a
// degraded application, orange a degraded machine, red down. A red pill is the
// box, and everything on it is unreachable with it.
// spec: CHK#presentation
const STATES = {
	fine: { border: "divider", fill: "transparent" },
	degraded: { border: "warning.main", fill: "rgba(237, 108, 2, 0.10)" },
	down: { border: "error.main", fill: "rgba(211, 47, 47, 0.12)" },
	// A box that has never reported: outlined like any other, washed out rather
	// than coloured, since there is nothing yet to say about it.
	never: { border: "action.disabled", fill: "action.disabledBackground" },
} as const;

// A window hatches the pill's wash rather than cutting through it the way a
// dot's does. A mask on the enclosure would clip the dots inside it as well,
// which would say something about the applications, and a window is the box's.
// The hatch runs the same way as the dot's maintenance cut, so the two read as
// one idea at either grain.
//
// A window washes the pill in the info hue, which is what marks maintenance
// everywhere else in the interface and is a colour no machine state uses: the
// ring's grey, orange and red are spoken for, so a blue wash cannot be read as
// a health verdict. The border still says how the box is, so a degraded box
// under a window shows both.
//
// A wash rather than a hatch because the ring is a few pixels wide: a pattern
// needs room to resolve into stripes, while a fill reads at any size. The
// settling state is the same hue washed back, and two strengths of a solid
// colour are separable where two strengths of a hatch are not.
// spec: MNT#presentation
// One wash for both states, since both are the same idea, and light enough
// that the dot inside stays the loudest thing in the pill: blue and green are
// adjacent hues, so anything deep enough to separate by hue swallows the dot
// it is meant to be context for.
function maintenanceWash(theme: Theme): string {
	return alpha(theme.palette.info.light, 0.45);
}

// Settling is that wash struck through in white, so the pair reads as one
// state at two stages rather than as two colours. White because it is the one
// ink that lifts off the wash without borrowing a hue that means something
// else, and the stripes carry their own phase so no background offset exposes
// the gradient's tile as a seam.
const SETTLING_STRIPES =
	"repeating-linear-gradient(45deg, rgba(255, 255, 255, 0.9) 0 1px, transparent 1px 3px, rgba(255, 255, 255, 0.9) 3px 4px)";

function enclosureState(up: ShortStatus, health: HealthState) {
	if (up === "gone") return STATES.never;
	if (up === "down") return STATES.down;
	if (health === "unhealthy" || health === "warning") return STATES.degraded;
	return STATES.fine;
}

function enclosureTitle(up: ShortStatus, health: HealthState): string {
	if (up === "gone") return "Machine has never reported";
	if (up === "down") return "Machine unreachable";
	if (health === "unhealthy") return "Machine's own checks failing";
	if (health === "warning") return "Machine's own checks warning";
	return "Machine healthy";
}

export default function MachineEnclosure({
	up,
	health,
	name,
	maintained = false,
	settling = false,
	describes,
	children,
}: {
	up: ShortStatus;
	health: HealthState;
	/** The box's name, for the tooltip. */
	name?: string | null;
	/** Whether a maintenance window suspends this box, its own or its group's.
	 * A window is declared over a machine and never over an application, so it
	 * is the enclosure that carries it — the applications inside are suspended
	 * by their box rather than each saying so. */
	// spec: MNT#presentation
	maintained?: boolean;
	/** Whether every window over the box has ended and it is serving out the
	 * settle period, still suspended but no longer being worked on. */
	// spec: MNT#settling
	settling?: boolean;
	/** What the dots inside stand for, one line each. The enclosure names them
	 * so the dots need no tooltip of their own: two tooltips over the same few
	 * pixels open together and overlap, and the reader loses both. */
	describes?: string[];
	/** The dots for the applications on this machine. */
	children: ReactNode;
}) {
	const state = enclosureState(up, health);
	const box = [
		name,
		enclosureTitle(up, health),
		maintained
			? settling
				? "maintenance just ended, watching resumes shortly"
				: "under maintenance"
			: null,
	]
		.filter(Boolean)
		.join(" · ");
	const title = [box, ...(describes ?? [])].join("\n");
	return (
		<Tooltip
			title={title}
			slotProps={{ tooltip: { sx: { whiteSpace: "pre-line" } } }}
		>
			<Box
				component="span"
				data-maintenance={
					maintained ? (settling ? "settling" : "holding") : undefined
				}
				sx={{
					display: "inline-flex",
					alignItems: "center",
					// Everything inside is sized in em, so without a scale of its
					// own a pill takes the font-size of whatever surrounds it and
					// comes out a different size on each surface. One rem here is
					// what makes a pill on a card, in a tree and in the legend the
					// same pill.
					fontSize: "1rem",
					lineHeight: 1,
					gap: "0.35em",
					border: 1,
					borderColor: state.border,
					bgcolor: (theme) =>
						maintained ? maintenanceWash(theme) : state.fill,
					backgroundImage:
						maintained && settling ? SETTLING_STRIPES : undefined,

					borderRadius: "999px",
					// The band keeps its old proportion to the dot inside it: both
					// grew together, so the ring reads as the same ring.
					px: "0.2em",
					py: "0.2em",
					// The dots carry their own right margin, which the pill's own
					// gap replaces.
					"& span": { marginRight: 0 },
				}}
			>
				{children}
			</Box>
		</Tooltip>
	);
}
