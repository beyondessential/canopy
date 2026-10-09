import {
	Alert,
	Box,
	Button,
	Chip,
	LinearProgress,
	Paper,
	Stack,
	Tooltip,
	Typography,
} from "@mui/material";
import ErrorOutlineIcon from "@mui/icons-material/ErrorOutlineOutlined";
import WarningAmberIcon from "@mui/icons-material/WarningAmber";
import { useState } from "react";
import { useApi, useApiAction } from "../api";
import { KIND_HEADINGS } from "../dnsNames";
import { useIsAdmin } from "../hooks/useIsAdmin";
import type { DnsNameView } from "../types";
import DeclareField from "./DeclareField";
import { GradedAction } from "./GradedAction";
import GrantChip from "./GrantChip";
import { PauseBanner, PauseButton } from "./PauseControls";
import TimeAgo from "./TimeAgo";

/// The DNS names an application declares for addresses, on the application's
/// page, with the addresses Canopy publishes for each. A separate section from
/// the application's TLS certificates: the two are separate features that
/// share infrastructure, and this one shows nothing about certificates.
///
/// Absent while the application neither may manage DNS nor declares a DNS name
/// for addresses, so a page does not carry an empty box for a feature it does
/// not use.
// spec: ADR#presentation
export default function ServerDnsNamesSection({
	serverId,
	refreshKey,
	onChanged,
}: {
	serverId: string;
	/// Changes when the other of the application's two sections changed
	/// something they share, such as the pause.
	refreshKey: number;
	onChanged: () => void;
}) {
	const isAdmin = useIsAdmin() === true;
	const [tick, setTick] = useState(0);
	const reload = () => {
		setTick((t) => t + 1);
		onChanged();
	};

	const detail = useApi("dns_names", "for_server", { server_id: serverId }, [
		serverId,
		tick,
		refreshKey,
	]);

	if (detail.status === "loading" || detail.status === "idle") {
		return (
			<Paper variant="outlined" sx={{ p: 2 }}>
				<SectionHeading />
				<LinearProgress />
			</Paper>
		);
	}
	if (detail.status === "error") {
		return (
			<Paper variant="outlined" sx={{ p: 2 }}>
				<SectionHeading />
				<Alert severity="error">{detail.error.message}</Alert>
			</Paper>
		);
	}

	const data = detail.data;
	if (!data.may_manage_dns && data.names.length === 0) return null;

	return (
		<Paper
			variant="outlined"
			sx={{ p: 2 }}
			data-testid="application-names-addresses"
		>
			<SectionHeading />
			<Stack spacing={2}>
				{data.pause && (
					<PauseBanner
						serverId={serverId}
						pause={data.pause}
						isAdmin={isAdmin}
						onChanged={reload}
					/>
				)}

				<Stack
					direction="row"
					spacing={1}
					sx={{ alignItems: "center", flexWrap: "wrap", rowGap: 1 }}
				>
					<GrantChip label="DNS records" granted={data.may_manage_dns} />
					<Typography variant="caption" color="text.secondary">
						{data.domains.length > 0
							? `within ${data.domains.join(", ")}`
							: "its group controls no domain, so it is entitled to no name"}
					</Typography>
					<Box sx={{ flex: 1 }} />
					{!data.pause && isAdmin && (
						<PauseButton serverId={serverId} onChanged={reload} />
					)}
				</Stack>

				<Box>
					<Stack
						direction="row"
						spacing={1}
						sx={{ alignItems: "flex-start", mb: 1, flexWrap: "wrap", rowGap: 1 }}
					>
						<Box sx={{ flex: 1 }} />
						{isAdmin && (
							<DeclareField
								kind="addresses"
								serverId={serverId}
								onChanged={reload}
							/>
						)}
					</Stack>
					{data.names.length === 0 ? (
						<Alert severity="info">
							This application declares no DNS names for addresses.
						</Alert>
					) : (
						<Stack spacing={1}>
							{data.names.map((row) => (
								<NameRowView
									key={row.id}
									serverId={serverId}
									row={row}
									isAdmin={isAdmin}
									onChanged={reload}
								/>
							))}
						</Stack>
					)}
				</Box>
			</Stack>
		</Paper>
	);
}

function SectionHeading() {
	return (
		<Typography variant="h6" component="h2" gutterBottom>
			{KIND_HEADINGS.addresses}
			<Typography
				component="span"
				variant="body2"
				color="text.secondary"
				sx={{ ml: 1 }}
			>
				— the DNS names this application serves, and the addresses Canopy
				publishes for them.
			</Typography>
		</Typography>
	);
}

/// A DNS name an application declares for addresses. A declaration is routing
/// only: one with no addresses registered is declared, not withdrawn, and
/// publishes nothing.
// spec: ADR#presentation
function NameRowView({
	serverId,
	row,
	isAdmin,
	onChanged,
}: {
	serverId: string;
	row: DnsNameView;
	isAdmin: boolean;
	onChanged: () => void;
}) {
	const release = useApiAction("dns_names", "release");
	// Nothing wanted and nothing published: an operator's declaration, or an
	// agent's request, with no addresses ever registered.
	const declaredOnly =
		row.addresses.length === 0 && row.published_addresses.length === 0;

	const onRelease = async () => {
		if (
			!confirm(
				`Release ${row.name} for addresses? Records already in place stay.`,
			)
		)
			return;
		try {
			await release.call({ application_id: serverId, name: row.name });
			onChanged();
		} catch {
			/* surfaced via release.error */
		}
	};

	return (
		<Box data-testid="dns-name-row">
			<Stack
				direction="row"
				spacing={1}
				sx={{ alignItems: "center", flexWrap: "wrap", rowGap: 0.5 }}
			>
				<Typography variant="body2" sx={{ fontFamily: "monospace" }}>
					{row.name}
				</Typography>
				{declaredOnly ? (
					<Chip size="small" label="declared" />
				) : row.published ? (
					<Chip
						size="small"
						variant="outlined"
						color="success"
						label="published"
					/>
				) : (
					<Tooltip title="Canopy has not yet written what this application asked for into the zone. It retries every pass.">
						<Chip
							size="small"
							variant="outlined"
							color="warning"
							label="waiting to publish"
						/>
					</Tooltip>
				)}
				{!row.within_domains && (
					<Tooltip title="Nothing can be published for it until the group controls a domain covering it.">
						<Chip
							size="small"
							variant="outlined"
							color="warning"
							icon={<WarningAmberIcon />}
							label="outside the group's domains"
						/>
					</Tooltip>
				)}
				{!row.zone && (
					<Tooltip title="No configured DNS zone covers this name, so Canopy can publish nothing for it.">
						<Chip
							size="small"
							variant="outlined"
							color="error"
							icon={<WarningAmberIcon />}
							label="no matching zone"
						/>
					</Tooltip>
				)}
				<Box sx={{ flex: 1 }} />
				<Typography variant="caption" color="text.secondary">
					{declaredOnly ? (
						"no addresses registered"
					) : row.published_at ? (
						<>
							published <TimeAgo timestamp={row.published_at} />
						</>
					) : (
						"never published"
					)}
				</Typography>
				{isAdmin && (
					<GradedAction
						calls="dns_names/release"
						action={`Release DNS name ${row.name} for addresses`}
					>
						<Button
							size="small"
							color="error"
							onClick={onRelease}
							disabled={release.pending}
						>
							Release
						</Button>
					</GradedAction>
				)}
			</Stack>
			{!declaredOnly && (
				<Typography
					variant="caption"
					color="text.secondary"
					sx={{ fontFamily: "monospace" }}
				>
					{row.addresses.length > 0 ? row.addresses.join(", ") : "withdrawn"}
					{!row.published &&
						row.published_addresses.length > 0 &&
						` (currently ${row.published_addresses.join(", ")})`}
				</Typography>
			)}
			{row.last_error && (
				<Alert severity="error" sx={{ mt: 0.5 }} icon={<ErrorOutlineIcon />}>
					{row.last_error}
				</Alert>
			)}
			{release.error && (
				<Alert severity="error" sx={{ mt: 0.5 }}>
					{release.error.message}
				</Alert>
			)}
		</Box>
	);
}
