import {
	Alert,
	Box,
	Button,
	Chip,
	IconButton,
	LinearProgress,
	Paper,
	Stack,
	TextField,
	Tooltip,
	Typography,
} from "@mui/material";
import AddIcon from "@mui/icons-material/Add";
import DeleteIcon from "@mui/icons-material/Delete";
import WarningAmberIcon from "@mui/icons-material/WarningAmber";
import { useState } from "react";
import { useApi, useApiAction } from "../api";
import { GradedAction } from "./GradedAction";
import { useIsAdmin } from "../hooks/useIsAdmin";
import TimeAgo from "./TimeAgo";

/// The domains this group controls, on the group detail page. A claim covers
/// every name beneath it and excludes every other group from overlapping it,
/// which is what authorises the group's servers to manage names there.
/// See `database::server_domains` for the matching backend.
// spec: DOM
export default function GroupDomainsSection({ groupId }: { groupId: string }) {
	const isAdmin = useIsAdmin() === true;
	const [tick, setTick] = useState(0);
	const reload = () => setTick((t) => t + 1);

	const domains = useApi(
		"domains",
		"for_group",
		{ server_group_id: groupId },
		[groupId, tick],
	);
	const zones = useApi("domains", "zones", {}, []);
	// The names in use beneath each claim, so whether the group's names are
	// covered is answerable here rather than server by server.
	// spec: CRT#presentation
	const health = useApi(
		"certificates",
		"for_group",
		{ server_group_id: groupId },
		[groupId, tick],
	);

	if (domains.status === "loading" || domains.status === "idle") {
		return (
			<Paper variant="outlined" sx={{ p: 2 }}>
				<SectionHeading />
				<LinearProgress />
			</Paper>
		);
	}
	if (domains.status === "error") {
		return (
			<Paper variant="outlined" sx={{ p: 2 }}>
				<SectionHeading />
				<Alert severity="error">{domains.error.message}</Alert>
			</Paper>
		);
	}

	const rows = domains.data;
	const zoneList = zones.status === "ok" ? zones.data : [];

	// Nothing claimed and nothing an operator can do about it: stay out of the
	// way rather than showing an empty box to every reader.
	if (rows.length === 0 && !isAdmin) return null;

	// Nothing claimed and no zone to claim into: the feature isn't in use here,
	// so say nothing at all rather than sitting a standing warning on every
	// group page of a Canopy instance that hasn't been given zones yet.
	// spec: DOM#when-the-zone-configuration-changes
	if (rows.length === 0 && zones.status === "ok" && zoneList.length === 0)
		return null;

	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<SectionHeading />
			<Stack spacing={2}>
				{rows.length === 0 ? (
					<Alert severity="info">
						This group controls no domains yet.
					</Alert>
				) : (
					<Stack spacing={1}>
						{rows.map((row) => (
							<DomainRow
								key={row.id}
								id={row.id}
								domain={row.domain}
								zone={row.zone}
								createdAt={row.created_at}
								createdBy={row.created_by}
								names={
									health.status === "ok"
										? (health.data.find((h) => h.domain === row.domain)
												?.names ?? [])
										: []
								}
								isAdmin={isAdmin}
								onChanged={reload}
							/>
						))}
					</Stack>
				)}

				{/* Wait for the zone list before offering the form: an empty
				    list mid-fetch would claim Canopy has no zones at all. */}
				{isAdmin && zones.status === "ok" && (
					<ClaimForm
						groupId={groupId}
						zoneApexes={zoneList.map((z) => z.apex)}
						onClaimed={reload}
					/>
				)}
			</Stack>
		</Paper>
	);
}

function SectionHeading() {
	return (
		<Typography variant="h6" component="h2" gutterBottom>
			Domains
			<Typography
				component="span"
				variant="body2"
				color="text.secondary"
				sx={{ ml: 1 }}
			>
				— this group controls every name beneath each of these, and no other
				group can claim a name overlapping them.
			</Typography>
		</Typography>
	);
}

type DomainName = {
	name: string;
	server_id: string;
	server_name: string | null;
	published: boolean | null;
	certificate: boolean;
	risk: string | null;
	not_after: string | null;
};

function DomainRow({
	id,
	domain,
	zone,
	createdAt,
	createdBy,
	names,
	isAdmin,
	onChanged,
}: {
	id: string;
	domain: string;
	zone: string | null;
	createdAt: string;
	createdBy: string | null;
	names: DomainName[];
	isAdmin: boolean;
	onChanged: () => void;
}) {
	const release = useApiAction("domains", "release");
	const onRelease = async () => {
		if (
			!window.confirm(
				`Release ${domain}? The group loses control of it and every name beneath it.`,
			)
		)
			return;
		try {
			await release.call({ id });
			onChanged();
		} catch {
			/* surfaced via release.error */
		}
	};

	return (
		<Box>
			<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
				<Typography variant="body1" sx={{ fontFamily: "monospace" }}>
					{domain}
				</Typography>
				{zone ? (
					<Chip size="small" variant="outlined" label={`zone ${zone}`} />
				) : (
					<Tooltip title="No configured DNS zone covers this domain any more, so Canopy can act on no name beneath it. Restore the zone to Canopy's configuration, or release the claim.">
						<Chip
							size="small"
							color="warning"
							variant="outlined"
							icon={<WarningAmberIcon />}
							label="no matching zone"
						/>
					</Tooltip>
				)}
				<Box sx={{ flex: 1 }} />
				<Typography variant="caption" color="text.secondary">
					claimed <TimeAgo timestamp={createdAt} />
					{createdBy && ` by ${createdBy}`}
				</Typography>
				{isAdmin && (
					<GradedAction calls="domains/release">
						<IconButton
							size="small"
							aria-label={`Release ${domain}`}
							onClick={onRelease}
							disabled={release.pending}
						>
							<DeleteIcon fontSize="small" />
						</IconButton>
					</GradedAction>
				)}
			</Stack>
			{names.length > 0 && (
				<Stack spacing={0.25} sx={{ mt: 0.5, ml: 2 }}>
					{names.map((row) => (
						<DomainNameRow key={row.name} row={row} />
					))}
				</Stack>
			)}
			{release.error && (
				<Alert severity="error" sx={{ mt: 1 }}>
					{release.error.message}
				</Alert>
			)}
		</Box>
	);
}

/// One name in use beneath a claim, with whether it is covered by a certificate
/// and whether its address records have caught up.
function DomainNameRow({ row }: { row: DomainName }) {
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
			<CertificateChip row={row} />
			{row.published === false && (
				<Tooltip title="Canopy has not yet written this name's address records into the zone.">
					<Chip
						size="small"
						variant="outlined"
						color="warning"
						label="records pending"
					/>
				</Tooltip>
			)}
			<Box sx={{ flex: 1 }} />
			{row.server_name && (
				<Typography variant="caption" color="text.secondary">
					{row.server_name}
				</Typography>
			)}
		</Stack>
	);
}

function CertificateChip({ row }: { row: DomainName }) {
	if (!row.certificate) {
		return (
			<Tooltip title="Canopy holds no valid certificate for this name. It may never have been requested, or an order may be failing — the server's page has the reason.">
				<Chip size="small" variant="outlined" label="no certificate" />
			</Tooltip>
		);
	}
	if (row.risk === "critical")
		return <Chip size="small" color="error" label="expiring" />;
	if (row.risk === "at_risk")
		return <Chip size="small" color="warning" label="due for renewal" />;
	return (
		<Chip size="small" color="success" variant="outlined" label="certified" />
	);
}

function ClaimForm({
	groupId,
	zoneApexes,
	onClaimed,
}: {
	groupId: string;
	zoneApexes: string[];
	onClaimed: () => void;
}) {
	const [domain, setDomain] = useState("");
	const claim = useApiAction("domains", "claim");

	const onSubmit = async (e: React.FormEvent) => {
		e.preventDefault();
		if (domain.trim() === "") return;
		try {
			await claim.call({ server_group_id: groupId, domain: domain.trim() });
			setDomain("");
			onClaimed();
		} catch {
			/* surfaced via claim.error */
		}
	};

	return (
		<Stack spacing={1} component="form" onSubmit={onSubmit}>
			{zoneApexes.length === 0 ? (
				<Alert severity="warning">
					Canopy has no managed DNS zones configured, so no domain can be
					claimed. Zones are set by the infrastructure that provisions Canopy.
				</Alert>
			) : (
				<Stack direction="row" spacing={1} sx={{ alignItems: "flex-start" }}>
					<TextField
						label="Domain"
						size="small"
						value={domain}
						onChange={(e) => setDomain(e.target.value)}
						disabled={claim.pending}
						sx={{ flex: 1, maxWidth: 420 }}
						helperText={`Must be at or under one of: ${zoneApexes.join(", ")}`}
					/>
					<GradedAction calls="domains/claim">
						<Button
							type="submit"
							variant="outlined"
							startIcon={<AddIcon />}
							disabled={claim.pending || domain.trim() === ""}
						>
							Claim
						</Button>
					</GradedAction>
				</Stack>
			)}
			{claim.error && <Alert severity="error">{claim.error.message}</Alert>}
		</Stack>
	);
}
