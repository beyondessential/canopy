import { Button } from "@mui/material";
import BuildOutlinedIcon from "@mui/icons-material/BuildOutlined";
import { useState } from "react";
import ActionButton from "./ActionButton";
import DeclareMaintenanceDialog from "./DeclareMaintenanceDialog";
import { GradedAction } from "./GradedAction";
import { maintenanceTarget } from "../types";
import type { MaintenanceScope } from "../types";

/** Maintenance at the head of a target's page, beside its other actions, so it
 * is at hand without finding the page's maintenance section. It reads the same
 * whatever the target's state: the dialog amends the target's own window where
 * it has one, and offers to lift it from there. */
// spec: MNT#declaring
export default function MaintenanceHeaderButton({
	scope,
	id,
	targetLabel,
	compact,
	onDone,
}: {
	scope: MaintenanceScope;
	id: string;
	/** What the target is called, so a blocked button says what it would act on. */
	targetLabel: string;
	/** Drawn as the page's compact action buttons rather than a full one. */
	compact?: boolean;
	onDone: () => void;
}) {
	const [open, setOpen] = useState(false);
	return (
		<>
			<GradedAction
				opens={["maintenance/declare", "maintenance/amend"]}
				action={`Declare maintenance on ${targetLabel}`}
			>
				{compact ? (
					<ActionButton
						icon={<BuildOutlinedIcon />}
						label="Maintenance"
						onClick={() => setOpen(true)}
					/>
				) : (
					<Button
						variant="outlined"
						startIcon={<BuildOutlinedIcon />}
						onClick={() => setOpen(true)}
					>
						Maintenance
					</Button>
				)}
			</GradedAction>
			<DeclareMaintenanceDialog
				open={open}
				onClose={() => setOpen(false)}
				start={maintenanceTarget(scope, id)}
				offerLift
				onDone={onDone}
			/>
		</>
	);
}
