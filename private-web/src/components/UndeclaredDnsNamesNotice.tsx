import { Alert, AlertTitle, Link } from "@mui/material";
import { Fragment } from "react";
import { Link as RouterLink } from "react-router-dom";
import { useApi } from "../api";
import type { UndeclaredNoticeView } from "../types";

type Place = { key: string; label: string; to: string | null; count: number };

/// A standing notice while machines have requests waiting on a declaration.
///
/// A notice rather than a check: it opens no incident and reaches no channel,
/// since what it asks for is an operator's decision rather than a response to
/// something down. On a group's page it names that group's machines; on the
/// Status page it names the groups.
// spec: NAM#notices
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
		"certificates",
		"undeclared_notices",
		groupId ? { server_group_id: groupId } : {},
		[groupId, refreshKey],
	);
	if (notices.status !== "ok" || notices.data.length === 0) return null;

	const rows: UndeclaredNoticeView[] = notices.data;
	const total = rows.reduce((sum, row) => sum + row.count, 0);
	const places = groupId ? byMachine(rows) : byGroup(rows);

	return (
		<Alert severity="info" data-testid="undeclared-dns-names-notice">
			<AlertTitle>
				{total} DNS {total === 1 ? "name" : "names"} waiting on a declaration
			</AlertTitle>
			{groupId ? "On " : "In "}
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
					{places.length > 1 && ` (${place.count})`}
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
		count: row.count,
	}));
}

function byGroup(rows: UndeclaredNoticeView[]): Place[] {
	const places: Place[] = [];
	for (const row of rows) {
		const key = row.group_id ?? "none";
		const place = places.find((p) => p.key === key);
		if (place) {
			place.count += row.count;
		} else {
			places.push({
				key,
				label: row.group_name ?? "no group",
				to: row.group_id ? `/fleet/groups/${row.group_id}` : null,
				count: row.count,
			});
		}
	}
	return places;
}
