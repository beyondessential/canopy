import { Box, Typography } from "@mui/material";
import { shortInstanceKey } from "../types";

/** One instance of a check, named as an operator reads it: its label, with
 * its key after it in secondary type, or the key alone where it carries no
 * label. A long key is shortened, and given in full on hover.
 * spec: CHK#checks-with-instances */
export default function InstanceName({
	instanceKey,
	label,
	quiet = false,
}: {
	instanceKey: string;
	label: string | null;
	/** Reads the name in secondary type, as a silenced instance's is. */
	quiet?: boolean;
}) {
	const nameColor = quiet ? "text.secondary" : undefined;
	if (!label) {
		return (
			<Typography
				component="span"
				variant="body2"
				color={nameColor}
				title={instanceKey}
				sx={{ fontFamily: "monospace", fontSize: 13 }}
			>
				{shortInstanceKey(instanceKey)}
			</Typography>
		);
	}
	return (
		<Box
			component="span"
			sx={{ display: "inline-flex", gap: 1, alignItems: "baseline", minWidth: 0 }}
		>
			<Typography
				component="span"
				variant="body2"
				color={nameColor}
				sx={{ fontSize: 13 }}
			>
				{label}
			</Typography>
			<Typography
				component="span"
				color="text.secondary"
				title={instanceKey}
				sx={{ fontFamily: "monospace", fontSize: 11 }}
			>
				{shortInstanceKey(instanceKey)}
			</Typography>
		</Box>
	);
}
