import { Alert, AlertTitle, Link } from "@mui/material";
import { Fragment } from "react";
import { Link as RouterLink } from "react-router-dom";
import { useApi } from "../api";
import type { UndeclaredNoticeView } from "../types";

type Place = {
	key: string;
	label: string;
	to: string | null;
	addresses: number;
	certificates: number;
};

const total = (place: { addresses: number; certificates: number }) =>
	place.addresses + place.certificates;

/// A standing notice while machines have requests waiting on a declaration.
///
/// A notice rather than a check: it opens no incident and reaches no channel,
/// since what it asks for is an operator's decision rather than a response to
/// something down. On a group's page it names that group's machines; on the
/// Status page it names the groups.
// spec: DNS#notices
export default function UndeclaredDnsNamesNotice({
	groupId,
	refreshKey,
}: {
	/// Narrow to one group's machines, naming them; omitted for the fleet,
	/// naming groups.
	groupId?: string;
	refreshKey?: unknown;
}) {
	const notices = useApi(
		"dns_names",
		"undeclared_notices",
		groupId ? { server_group_id: groupId } : {},
		[groupId, refreshKey],
	);
	if (notices.status !== "ok" || notices.data.length === 0) return null;

	const rows: UndeclaredNoticeView[] = notices.data;
	const addresses = rows.reduce((sum, row) => sum + row.addresses, 0);
	const certificates = rows.reduce((sum, row) => sum + row.certificates, 0);
	const count = addresses + certificates;
	const places = groupId ? byMachine(rows) : byGroup(rows);

	return (
		<Alert severity="info" data-testid="undeclared-dns-names-notice">
			<AlertTitle>
				{count} {count === 1 ? "request" : "requests"} waiting on a declaration
			</AlertTitle>
			{kindSummary(addresses, certificates)}
			{groupId ? " on " : " in "}
			{places.map((place, i) => (
				<Fragment key={place.key}>
					{i > 0 && (i === places.length - 1 ? " and " : ", ")}
					{place.to ? (
						<Link component={RouterLink} to={place.to} color="inherit">
							{place.label}
						</Link>
					) : (
						place.label
					)}
					{places.length > 1 && ` (${total(place)})`}
				</Fragment>
			))}
			.
		</Alert>
	);
}

function byMachine(rows: UndeclaredNoticeView[]): Place[] {
	return rows.map((row) => ({
		key: row.machine_id,
		label: row.machine_name,
		to: `/fleet/machines/${row.machine_id}`,
		addresses: row.addresses,
		certificates: row.certificates,
	}));
}

function byGroup(rows: UndeclaredNoticeView[]): Place[] {
	const places: Place[] = [];
	for (const row of rows) {
		const key = row.group_id ?? "none";
		const place = places.find((p) => p.key === key);
		if (place) {
			place.addresses += row.addresses;
			place.certificates += row.certificates;
		} else {
			places.push({
				key,
				label: row.group_name ?? "no group",
				to: row.group_id ? `/fleet/groups/${row.group_id}` : null,
				addresses: row.addresses,
				certificates: row.certificates,
			});
		}
	}
	return places;
}

/// Says how many of the requests are for each kind, naming only the kinds there
/// are.
function kindSummary(addresses: number, certificates: number): string {
	const parts = [];
	if (addresses > 0)
		parts.push(`${addresses} for DNS ${addresses === 1 ? "address" : "addresses"}`);
	if (certificates > 0)
		parts.push(
			`${certificates} for TLS ${certificates === 1 ? "certificate" : "certificates"}`,
		);
	return parts.join(" and ");
}
