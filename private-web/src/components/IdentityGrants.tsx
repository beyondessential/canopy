import ContentCopyIcon from "@mui/icons-material/ContentCopy";
import {
	Autocomplete,
	Box,
	Chip,
	IconButton,
	Stack,
	TextField,
	Tooltip,
	Typography,
} from "@mui/material";
import type { MachineGrants } from "../types";

/// The relying parties a machine, or one application on it, may obtain
/// identity tokens for, with the subject those tokens carry.
///
/// Offered while a relying party is registered, the Canopy instance issues
/// tokens, and the machine is placed. Otherwise shown only for a subject that
/// already holds a grant, with nothing more to add, so a held grant can always
/// be withdrawn.
// spec: IDP#presentation
export default function IdentityGrants({
	grants,
	held,
	value,
	onChange,
	sub,
	caption,
	disabled,
}: {
	grants: MachineGrants;
	/// What the subject holds as saved.
	held: string[];
	value: string[];
	onChange: (ids: string[]) => void;
	sub: string | null | undefined;
	caption: string;
	disabled: boolean;
}) {
	if (!grants.offered && held.length === 0) return null;

	const options = grants.relying_parties;
	const byId = new Map(options.map((party) => [party.id, party]));
	const selected = value.flatMap((id) => {
		const party = byId.get(id);
		return party ? [party] : [];
	});

	return (
		<Stack spacing={1}>
			<Typography variant="subtitle1">Identity tokens</Typography>
			<Autocomplete
				multiple
				options={options}
				value={selected}
				onChange={(_, parties) => onChange(parties.map((party) => party.id))}
				getOptionLabel={(party) => party.name}
				getOptionDisabled={(party) => !grants.offered && !value.includes(party.id)}
				isOptionEqualToValue={(a, b) => a.id === b.id}
				disabled={disabled}
				renderValue={(parties, getItemProps) =>
					parties.map((party, index) => {
						const { key, ...itemProps } = getItemProps({ index });
						return <Chip key={key} size="small" label={party.name} {...itemProps} />;
					})
				}
				renderInput={(params) => (
					<TextField {...params} label="May obtain tokens for" />
				)}
			/>
			<Typography variant="caption" color="text.secondary">
				{caption}
			</Typography>
			{sub && (
				<Box sx={{ display: "flex", alignItems: "center", gap: 0.5 }}>
					<Typography
						component="code"
						variant="body2"
						sx={{ fontFamily: "monospace", wordBreak: "break-all" }}
					>
						{sub}
					</Typography>
					<Tooltip title="Copy">
						<IconButton
							size="small"
							aria-label="copy subject"
							onClick={() => navigator.clipboard.writeText(sub)}
						>
							<ContentCopyIcon fontSize="small" />
						</IconButton>
					</Tooltip>
				</Box>
			)}
		</Stack>
	);
}
