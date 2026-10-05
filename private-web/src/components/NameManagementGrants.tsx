import {
	Alert,
	Checkbox,
	FormControlLabel,
	Stack,
	Typography,
} from "@mui/material";
import { useApi } from "../api";

/// The two name-management grants, shown only when they could mean something.
///
/// A grant is exercised over names beneath a domain the server's *group*
/// controls, so it is worth nothing on its own. Where the Canopy instance has no zones
/// and no group anywhere controls a domain, the feature is not in use at all and
/// these controls stay out of the way entirely — a checkbox that cannot affect
/// anything is worse than no checkbox. Where the feature is in use but this
/// group controls no domain, they show disabled with the reason, because that is
/// a gap an operator can close by claiming a domain.
///
/// Keyed on the *selected* group rather than the saved one, so moving the server
/// into a group that controls a domain makes the grants available before saving.
// spec: DOM#permission-for-an-application-to-manage-its-own-dns-names
export default function NameManagementGrants({
	groupId,
	mayManageDns,
	mayManageTls,
	setMayManageDns,
	setMayManageTls,
	disabled,
}: {
	groupId: string | null;
	mayManageDns: boolean;
	mayManageTls: boolean;
	setMayManageDns: (v: boolean) => void;
	setMayManageTls: (v: boolean) => void;
	disabled: boolean;
}) {
	const availability = useApi(
		"domains",
		"grant_availability",
		{ server_group_id: groupId },
		[groupId],
	);

	// Wait for the answer rather than guessing: rendering enabled controls and
	// then disabling them reads as the form fighting the operator.
	if (availability.status !== "ok") return null;
	const { state, group_domains } = availability.data;

	const held = mayManageDns || mayManageTls;

	// Not in use in this Canopy instance — unless this server somehow holds a grant
	// already, in which case hiding the control would strand it with no way to
	// withdraw it.
	if (state === "unconfigured" && !held) return null;

	const unavailable = state !== "available";

	return (
		<Stack spacing={1}>
			<Typography variant="subtitle1">Name management</Typography>
			<FormControlLabel
				control={
					<Checkbox
						checked={mayManageDns}
						onChange={(e) => setMayManageDns(e.target.checked)}
						disabled={disabled || (unavailable && !mayManageDns)}
					/>
				}
				label="May manage its own DNS records"
			/>
			<FormControlLabel
				control={
					<Checkbox
						checked={mayManageTls}
						onChange={(e) => setMayManageTls(e.target.checked)}
						disabled={disabled || (unavailable && !mayManageTls)}
					/>
				}
				label="May obtain its own TLS certificates"
			/>
			{state === "available" ? (
				<Typography variant="caption" color="text.secondary">
					Both apply only to names under {group_domains.join(", ")}, the
					domain{group_domains.length === 1 ? "" : "s"} this server's group
					controls, and are off until granted: a server without the grant it
					needs is refused. Revoking stops further changes and leaves records
					and certificates already in place.
				</Typography>
			) : state === "no_group_domain" ? (
				<Alert severity="info">
					{groupId
						? "This server's group controls no domain, so neither grant would authorise it over any name. Claim a domain on the group's page first."
						: "This server has no group. A domain is controlled by a group, so a server outside one can hold no useful grant."}
				</Alert>
			) : (
				<Alert severity="warning">
					Canopy has no managed DNS zones configured and no group controls a
					domain, so name management is not in use here — but this server still
					holds a grant. Clear it, or have the infrastructure provide a zone.
				</Alert>
			)}
		</Stack>
	);
}
