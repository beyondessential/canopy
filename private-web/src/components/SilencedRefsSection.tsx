import {
	Alert,
	Box,
	Button,
	LinearProgress,
	Paper,
	Stack,
	Typography,
} from "@mui/material";
import NotificationsActiveOutlinedIcon from "@mui/icons-material/NotificationsActiveOutlined";
import { useState } from "react";
import { useApi, useApiAction } from "../api";
import { useIsAdmin } from "../hooks/useIsAdmin";
import { qualifiedSilenceRef, namespaceSegment, type NamespaceRef } from "../types";
import { GradedAction } from "./GradedAction";
import TimeAgo from "./TimeAgo";

/// Listed at the bottom of the server / group detail page. Renders the
/// `(source, ref)` tuples that the operator has silenced at that scope and
/// offers an un-silence button per row. Silenced refs don't contribute to
/// incidents — see `database::silenced_refs` for the matching backend.
export default function SilencedRefsSection({
	scope,
	id,
	refreshKey,
	onChanged,
}: {
	scope: "server" | "machine" | "group";
	id: string;
	/** Parent-controlled cache-bust; bump to refetch the list after the
	 * parent took an action that may have added a silence (e.g. silencing
	 * a healthcheck from the ChecksTable). */
	refreshKey?: number;
	/** Called after this section un-silences a row. Lets the parent
	 * (e.g. ServerDetail) refresh sibling views that key off silence
	 * state — the ChecksTable's per-row "silenced" chip would otherwise
	 * stay stale until the next page-wide refresh. */
	onChanged?: () => void;
}) {
	const isAdmin = useIsAdmin() === true;
	const [tick, setTick] = useState(0);
	const reload = () => {
		setTick((t) => t + 1);
		onChanged?.();
	};

	// One hook call per scope, all three always invoked: hooks cannot be
	// called conditionally, and only the one this section is for is enabled.
	const serverResult = useApi(
		"silenced_refs",
		"list_for_server",
		{ server_id: scope === "server" ? id : "" },
		[scope, id, tick, refreshKey],
	);
	const machineResult = useApi(
		"silenced_refs",
		"list_for_machine",
		{ machine_id: scope === "machine" ? id : "" },
		[scope, id, tick, refreshKey],
	);
	const groupResult = useApi(
		"silenced_refs",
		"list_for_group",
		{ server_group_id: scope === "group" ? id : "" },
		[scope, id, tick, refreshKey],
	);
	const result =
		scope === "server"
			? serverResult
			: scope === "machine"
				? machineResult
				: groupResult;

	if (result.status === "loading" || result.status === "idle") {
		return (
			<Paper variant="outlined" sx={{ p: 2 }}>
				<SectionHeading scope={scope} />
				<LinearProgress />
			</Paper>
		);
	}
	if (result.status === "error") {
		return (
			<Paper variant="outlined" sx={{ p: 2 }}>
				<SectionHeading scope={scope} />
				<Alert severity="error">{result.error.message}</Alert>
			</Paper>
		);
	}
	const rows = result.data;
	if (rows.length === 0) {
		return null;
	}

	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<SectionHeading scope={scope} />
			<Stack spacing={1}>
				{rows.map((row) => (
					<SilencedRow
						key={`${row.source}\x00${namespaceSegment(
							"namespace" in row ? row.namespace : undefined,
						)}\x00${row.ref}`}
						scope={scope}
						id={id}
						source={row.source}
						namespace={"namespace" in row ? row.namespace : null}
						refName={row.ref}
						createdAt={row.created_at}
						createdBy={row.created_by ?? null}
						isAdmin={isAdmin}
						onChanged={reload}
					/>
				))}
			</Stack>
		</Paper>
	);
}

function SectionHeading({
	scope,
}: {
	scope: "server" | "machine" | "group";
}) {
	return (
		<Typography variant="h6" component="h2" gutterBottom>
			Silenced refs
			<Typography
				component="span"
				variant="body2"
				color="text.secondary"
				sx={{ ml: 1 }}
			>
				{scope === "server"
					? "— issues with these refs on this server don't open incidents."
					: scope === "machine"
						? "— issues with these refs on this machine don't open incidents."
						: "— issues with these refs anywhere in this group don't open incidents."}
			</Typography>
		</Typography>
	);
}

function SilencedRow({
	scope,
	id,
	source,
	namespace,
	refName,
	createdAt,
	createdBy,
	isAdmin,
	onChanged,
}: {
	scope: "server" | "machine" | "group";
	id: string;
	source: string;
	/** Which catalog entry a group-scoped silence quiets. A group spans
	 * several application types, so the ref alone does not say. A server's
	 * or machine's own silences need none: the target fixes the namespace. */
	namespace: NamespaceRef | null;
	refName: string;
	createdAt: string;
	createdBy: string | null;
	isAdmin: boolean;
	onChanged: () => void;
}) {
	const unsilenceServer = useApiAction("silenced_refs", "unsilence_server");
	const unsilenceMachine = useApiAction("silenced_refs", "unsilence_machine");
	const unsilenceGroup = useApiAction("silenced_refs", "unsilence_group");
	const action =
		scope === "server"
			? unsilenceServer
			: scope === "machine"
				? unsilenceMachine
				: unsilenceGroup;
	const unsilenceCall =
		scope === "server"
			? "silenced_refs/unsilence_server"
			: scope === "machine"
				? "silenced_refs/unsilence_machine"
				: "silenced_refs/unsilence_group";
	const pending = action.pending;
	const error = action.error;
	const unsilence = async () => {
		try {
			if (scope === "server") {
				await unsilenceServer.call({ server_id: id, source, ref: refName });
			} else if (scope === "machine") {
				await unsilenceMachine.call({ machine_id: id, source, ref: refName });
			} else {
				await unsilenceGroup.call({
					server_group_id: id,
					source,
					ref: refName,
					application_type: namespace?.application_type ?? null,
				});
			}
			onChanged();
		} catch {
			/* surfaced via error */
		}
	};

	return (
		<Box
			sx={{
				p: 1.5,
				border: 1,
				borderColor: "divider",
				borderRadius: 1,
			}}
		>
			<Stack
				direction="row"
				spacing={1}
				sx={{ alignItems: "center", flexWrap: "wrap" }}
				useFlexGap
			>
				<Typography
					component="code"
					sx={{ fontFamily: "monospace", fontWeight: 500 }}
				>
					{source}/{qualifiedSilenceRef(namespace ?? undefined, refName)}
				</Typography>
				<Box sx={{ flex: 1 }} />
				<Typography variant="caption" color="text.secondary">
					silenced <TimeAgo timestamp={createdAt} />
					{createdBy && ` by ${createdBy}`}
				</Typography>
				{isAdmin && (
					<GradedAction calls={unsilenceCall}>
						<Button
							size="small"
							variant="outlined"
							startIcon={<NotificationsActiveOutlinedIcon />}
							disabled={pending}
							onClick={unsilence}
						>
							Un-silence
						</Button>
					</GradedAction>
				)}
			</Stack>
			{error && <Alert severity="error" sx={{ mt: 1 }}>{error.message}</Alert>}
		</Box>
	);
}
