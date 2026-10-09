import { Chip } from "@mui/material";

/// Whether an operator has allowed an application a kind of request.
export default function GrantChip({
	label,
	granted,
}: {
	label: string;
	granted: boolean;
}) {
	return (
		<Chip
			size="small"
			variant="outlined"
			color={granted ? "success" : "default"}
			label={granted ? `may manage ${label}` : `may not manage ${label}`}
		/>
	);
}
