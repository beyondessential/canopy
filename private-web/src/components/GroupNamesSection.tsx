import { Box, Chip, Paper, Stack, Tooltip, Typography } from "@mui/material";
import { useApi } from "../api";
import { type DnsNameKind, KIND_HEADINGS, KIND_MODULES } from "../dnsNames";
import type { DomainNameView } from "../types";
import TimeLeft from "./TimeLeft";

/// The DNS names of one kind in use beneath each domain a group controls, so
/// whether the group's DNS names or certificates are healthy is answerable here
/// rather than application by application. One component for both kinds, fed
/// from the endpoint of the kind it is given; a DNS name declared for both kinds
/// appears under both sections.
///
/// Absent while the group has no DNS name of the kind beneath any of its
/// domains.
// spec: DNS#on-a-group
export default function GroupNamesSection({
	kind,
	groupId,
	refreshKey,
}: {
	kind: DnsNameKind;
	groupId: string;
	refreshKey?: unknown;
}) {
	const health = useApi(
		KIND_MODULES[kind],
		"for_group",
		{ server_group_id: groupId },
		[groupId, kind, refreshKey],
	);
	if (health.status !== "ok") return null;

	const domains = health.data.filter((domain) => domain.names.length > 0);
	if (domains.length === 0) return null;

	return (
		<Paper
			variant="outlined"
			sx={{ p: 2 }}
			data-testid={`group-names-${kind}`}
		>
			<Typography variant="h6" component="h2" gutterBottom>
				{KIND_HEADINGS[kind]}
			</Typography>
			<Stack spacing={1.5}>
				{domains.map((domain) => (
					<Box key={domain.domain}>
						<Typography variant="body2" sx={{ fontFamily: "monospace" }}>
							{domain.domain}
						</Typography>
						<Stack spacing={0.25} sx={{ mt: 0.5, ml: 2 }}>
							{domain.names.map((row) => (
								<NameRow key={row.name} kind={kind} row={row} />
							))}
						</Stack>
					</Box>
				))}
			</Stack>
		</Paper>
	);
}

function NameRow({ kind, row }: { kind: DnsNameKind; row: DomainNameView }) {
	return (
		<Stack
			direction="row"
			spacing={1}
			sx={{ alignItems: "center", flexWrap: "wrap", rowGap: 0.25 }}
		>
			<Typography
				variant="caption"
				sx={{ fontFamily: "monospace" }}
				color="text.secondary"
			>
				{row.name}
			</Typography>
			{kind === "addresses" ? <RecordsChip row={row} /> : <CertificateChip row={row} />}
			<Box sx={{ flex: 1 }} />
			{row.server_name && (
				<Typography variant="caption" color="text.secondary">
					{row.server_name}
				</Typography>
			)}
		</Stack>
	);
}

function RecordsChip({ row }: { row: DomainNameView }) {
	if (row.published === null) return <Chip size="small" label="declared" />;
	return row.published ? (
		<Chip size="small" variant="outlined" color="success" label="published" />
	) : (
		<Tooltip title="Canopy has not yet written this name's address records into the zone.">
			<Chip
				size="small"
				variant="outlined"
				color="warning"
				label="records pending"
			/>
		</Tooltip>
	);
}

function CertificateChip({ row }: { row: DomainNameView }) {
	const certificate = row.certificate;
	if (!certificate?.current) {
		return (
			<Tooltip title="Canopy holds no valid certificate for this name. It may never have been requested, or an order may be failing; the application's page has the reason.">
				<Chip size="small" variant="outlined" label="no certificate" />
			</Tooltip>
		);
	}
	return (
		<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
			{certificate.risk === "critical" ? (
				<Chip size="small" color="error" label="expiring" />
			) : certificate.risk === "at_risk" ? (
				<Chip size="small" color="warning" label="due for renewal" />
			) : (
				<Chip size="small" color="success" variant="outlined" label="certified" />
			)}
			{certificate.not_after && (
				<TimeLeft notAfter={certificate.not_after} risk={certificate.risk} />
			)}
		</Stack>
	);
}
