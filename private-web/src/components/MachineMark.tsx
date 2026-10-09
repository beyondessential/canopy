import { Box, Tooltip } from "@mui/material";
import { darken } from "@mui/material/styles";
import type { HealthState, ShortStatus } from "../types";
import {
	MUTED,
	machineState,
	machineTitle,
	NEVER_REPORTED,
	ONE_DOT_ENCLOSURE,
	ownWindowStripes,
	pulseWhileHolding,
} from "./MachineEnclosure";

// The box's mark where the applications on it are listed beneath, as in the
// group tree. Enclosing their dots would repeat the rows below, so the box is
// drawn alone: a solid dot in its state's colour, edged a shade darker. With
// nothing inside it there is no wash to stay quieter than, so a fine box is
// green here.
// spec: CHK#presentation
const PALETTE = {
	fine: "success",
	degraded: "warning",
	down: "error",
} as const;

export default function MachineMark({
	up,
	health,
	name,
	maintained = false,
	settling = false,
	ownWindow = false,
	heldBy,
}: {
	up: ShortStatus;
	health: HealthState;
	name?: string | null;
	/** Whether a window suspends this box, whatever grain it was declared over. */
	// spec: MNT#presentation
	maintained?: boolean;
	/** Whether every window over the box has ended and it is serving out the
	 * settle period. */
	// spec: MNT#settling
	settling?: boolean;
	/** Whether the window covering it was declared over this box in
	 * particular; one reaching it through its environment or group is marked at
	 * that grain instead. */
	// spec: MNT#presentation
	ownWindow?: boolean;
	heldBy?: string | null;
}) {
	const state = machineState(up, health);
	const title = machineTitle({
		name,
		up,
		health,
		maintained,
		ownWindow,
		settling,
		heldBy,
	});
	return (
		<Tooltip title={title}>
			<Box
				component="span"
				data-testid="machine-mark"
				data-state={state}
				data-maintenance={
					ownWindow ? (settling ? "settling" : "holding") : undefined
				}
				sx={(theme) => {
					const base = {
						display: "inline-block",
						flex: "none",
						fontSize: "1rem",
						width: ONE_DOT_ENCLOSURE,
						height: ONE_DOT_ENCLOSURE,
						borderRadius: "50%",
						boxSizing: "border-box",
						backgroundClip: "padding-box",
						backgroundImage: ownWindow
							? ownWindowStripes(theme, settling)
							: "none",
						opacity: maintained ? MUTED : 1,
						...pulseWhileHolding(ownWindow && !settling),
					};
					if (state === "never")
						return { ...base, ...NEVER_REPORTED };
					const fill = theme.palette[PALETTE[state]].main;
					return {
						...base,
						bgcolor: fill,
						border: 1,
						borderColor: darken(fill, 0.2),
					};
				}}
			/>
		</Tooltip>
	);
}
