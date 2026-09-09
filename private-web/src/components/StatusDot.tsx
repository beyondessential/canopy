import { Box, Tooltip, keyframes, type Theme } from "@mui/material";
import { alpha } from "@mui/material/styles";
import type { HealthState, ShortStatus } from "../types";

// One dot, one subject, one colourway.
//
// The dot used to say two things at once — a fill for reachability and a ring
// for health — because a server was a box and the software on it at once. The
// machine owns reachability now and carries it on the enclosure around the dot
// (see `MachineEnclosure`), so the dot spends its whole colourway on the one
// application, and an operator reads severity from the colour rather than
// decoding a ring.
//
// Red appears on both grains deliberately: red means down, and which element
// carries it says what went down. A red dot in a neutral pill is one
// application failing on a healthy box; a red pill is the box.
// spec: CHK#presentation
const HEALTHY = (theme: Theme) => theme.palette.success.main;
// A near neighbour of the healthy green, on purpose. A warning is "could be
// better" rather than "degraded", so it reads as a shade of fine rather than a
// shade of trouble, and does not compete with a red for attention. Yellow stays
// out of the ramp entirely, being spoken for by the striped check-broken state.
const WARNING = "rgb(132, 223, 91)";
const DOWN = (theme: Theme) => theme.palette.error.main;
const NEVER = (theme: Theme) => theme.palette.text.disabled;

// Diagonal cut for unmonitored targets. A mask punches the band out of the
// dot rather than painting over it, so the surface behind shows through
// whatever it is — these dots sit on plain paper, on operator-tinted group
// cards, and on hover backgrounds.
const UNMONITORED_MASK =
	"linear-gradient(135deg, #000 0 42%, transparent 42% 58%, #000 58% 100%)";

// A window hollows the dot instead of cutting it: the workload is out of play,
// and a ring keeps its health readable where a second cut would only differ
// from the unmonitored one by its angle, which a dot has no room to say.
// Settling puts a faint centre back, the window having ended.
// spec: MNT#presentation
const RING = "2px solid";
const SETTLING_CENTRE = 0.3;

// A dot has no room for a wave to cross, so it pulses instead, on the timing
// the wave runs at. Work under way moves and a target serving out the settle
// period is still, the same rule every grain follows.
// spec: MNT#presentation
const PULSE = keyframes`
	0%, 100% { opacity: 1; }
	50% { opacity: 0.4; }
`;

/// What colour this target's dot takes.
///
/// Never reported wins over everything: there is nothing to say about a target
/// that has never spoken. Otherwise silence and self-reported failure are both
/// red — the application is not serving either way, and which it is reads from
/// the tooltip and its own page rather than from a second colour.
export function dotColor(
	theme: Theme,
	up: ShortStatus,
	health?: HealthState,
): string {
	if (up === "gone") return NEVER(theme);
	if (up === "down" || health === "unhealthy") return DOWN(theme);
	if (health === "warning") return WARNING;
	return HEALTHY(theme);
}

interface StatusDotProps {
	up: ShortStatus;
	/** The target's health from its current check state. Folded into the one
	 * colourway with reachability rather than carried as a separate ring. */
	health?: HealthState;
	title?: string;
	/** Size relative to the surrounding font size. Defaults to "1em". */
	size?: string;
	/** Whether canopy alerts on this target. Unmonitored targets are cut
	 * through with a diagonal so a red dot doesn't read as "someone is
	 * being paged about this" — their checks are determined and shown as
	 * normal but raise nothing. Defaults to monitored. */
	// spec: CHK#monitoring-gate
	monitored?: boolean;
	/** Whether a window declared over this target in particular suspends it.
	 * Hollowed rather than cut: the work is deliberate and temporary, and its
	 * checks are still recorded and shown. One reaching it through the box it
	 * runs on is marked there instead. Defaults to no window. */
	// spec: MNT#presentation
	maintained?: boolean;
	/** Whether that window has ended and the target is serving out the settle
	 * period. */
	// spec: MNT#settling
	settling?: boolean;
	/** Whether something around this dot already names what it stands for. Two
	 * tooltips over the same few pixels open together and overlap, and the
	 * reader loses both, so a dot inside a `MachineEnclosure` carries none of
	 * its own. */
	quiet?: boolean;
}

export default function StatusDot({
	up,
	health,
	title,
	size = "1em",
	monitored = true,
	maintained = false,
	settling = false,
	quiet = false,
}: StatusDotProps) {
	const mask = monitored ? undefined : UNMONITORED_MASK;
	const dot = (
		<Box
			component="span"
			data-testid="status-dot"
			data-maintenance={
				maintained ? (settling ? "settling" : "holding") : undefined
			}
			sx={(theme) => {
				const color = dotColor(theme, up, health);
				return {
					display: "inline-block",
					boxSizing: "border-box",
					width: size,
					height: size,
					borderRadius: "50%",
					border: maintained ? RING : undefined,
					borderColor: maintained ? color : undefined,
					bgcolor: maintained
						? settling
							? alpha(color, SETTLING_CENTRE)
							: "transparent"
						: color,
					marginRight: "0.5em",
					verticalAlign: "middle",
					maskImage: mask,
					WebkitMaskImage: mask,
					...(maintained && !settling
						? {
								animation: `${PULSE} 2s ease-in-out 0.5s infinite`,
								"@media (prefers-reduced-motion: reduce)": {
									animation: "none",
								},
							}
						: {}),
				};
			}}
		/>
	);
	const tooltip = quiet
		? ""
		: [
				title,
				maintained
					? settling
						? "maintenance just ended, watching resumes shortly"
						: "under maintenance"
					: null,
				monitored ? null : "unmonitored",
			]
				.filter(Boolean)
				.join(" · ");
	if (tooltip) {
		return <Tooltip title={tooltip}>{dot}</Tooltip>;
	}
	return dot;
}
