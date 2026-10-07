import { Chip } from "@mui/material";

/// What kind of target something is, styled to match [`ServerRankChip`], so a
/// row carrying both reads as one set rather than two.
export default function KindChip({ kind }: { kind: string }) {
	return (
		<Chip
			size="small"
			variant="outlined"
			label={kind}
			sx={{ textTransform: "capitalize" }}
		/>
	);
}
