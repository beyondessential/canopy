import {
	Alert,
	Box,
	Button,
	Chip,
	LinearProgress,
	Paper,
	Stack,
	Typography,
} from "@mui/material";
import { useState } from "react";
import { Link as RouterLink } from "react-router-dom";
import { useApi, useApiAction } from "../api";
import { GradedAction } from "../components/GradedAction";
import { healthcheckSettingsPath } from "../types";
import { usePageTitle } from "../hooks/usePageTitle";
import { useIsAdmin } from "../hooks/useIsAdmin";
function chipColor(result: string | null): "error" | "warning" | "info" | "default" {
	switch (result) {
		case "failed":
			return "error";
		case "warning":
		case "broken":
			return "warning";
		case "skipped":
			return "default";
		default:
			return "info";
	}
}

/// One check an alert named, as the alert's detail carries it.
interface QuietCheck {
	source: string;
	check: string;
	qualified_name: string;
	subject: string | null;
	application_type: string | null;
}

function quietChecks(detail: unknown): QuietCheck[] {
	if (typeof detail !== "object" || detail === null) return [];
	const checks = (detail as { checks?: unknown }).checks;
	if (!Array.isArray(checks)) return [];
	return checks.filter(
		(c): c is QuietCheck =>
			typeof c === "object" &&
			c !== null &&
			typeof (c as QuietCheck).source === "string" &&
			typeof (c as QuietCheck).check === "string",
	);
}

/// The checks an alert names, each linked to its own policy page.
///
/// The stale-healthcheck alert asks the operator to decommission what has gone
/// away, and decommissioning lives on a check's policy page. Naming the checks
/// in a sentence and leaving the operator to find them is what made the alert
/// feel unresolvable: resolving it does nothing, because the condition is real
/// until the checks are retired.
/// spec: SELF#presentation
function QuietChecks({ detail }: { detail: unknown }) {
	const checks = quietChecks(detail);
	if (checks.length === 0) return null;
	return (
		<Box sx={{ display: "flex", flexWrap: "wrap", gap: 0.5 }}>
			{checks.map((c) => (
				<Chip
					key={`${c.source}/${c.subject ?? ""}/${c.application_type ?? ""}/${c.check}`}
					size="small"
					variant="outlined"
					clickable
					component={RouterLink}
					to={healthcheckSettingsPath(
						c.source,
						{
							subject: c.subject,
							application_type: c.application_type,
						},
						c.check,
					)}
					label={`${c.source}/${c.qualified_name}`}
				/>
			))}
		</Box>
	);
}

/// Canopy's alerts about its own operation — kept apart from fleet
/// issues/incidents (these belong to no server or group).
export default function SelfAlerts() {
	usePageTitle("Canopy alerts");
	const list = useApi("self_alerts", "list");
	const isAdmin = useIsAdmin();
	// useApiAction (not bare callApi) so the resolve broadcasts
	// canopy-data-changed and the app-level banner refetches immediately.
	const resolveAction = useApiAction("self_alerts", "resolve");
	const [error, setError] = useState<string | null>(null);

	if (list.status === "loading" || list.status === "idle") {
		return <LinearProgress />;
	}
	if (list.status === "error") {
		return <Alert severity="error">{list.error.message}</Alert>;
	}

	const alerting = list.data.filter((a) => a.active && !a.resolved_at);
	const rest = list.data.filter((a) => !(a.active && !a.resolved_at));

	const onResolve = async (id: string) => {
		try {
			await resolveAction.call({ id });
			list.reload();
		} catch (err) {
			setError(err instanceof Error ? err.message : String(err));
		}
	};

	return (
		<Stack spacing={3}>
			<Typography variant="h5" component="h1">
				Canopy alerts
			</Typography>
			<Typography variant="body2" color="text.secondary">
				Problems with canopy's own operation. These don't belong to any
				server or group, so they live here instead of the incidents page.
			</Typography>
			{error && <Alert severity="error">{error}</Alert>}

			{alerting.length === 0 ? (
				<Alert severity="success">No active alerts.</Alert>
			) : (
				alerting.map((a) => (
					<Paper key={a.id} variant="outlined" sx={{ p: 2 }}>
						<Stack spacing={1}>
							<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
								<Chip
									size="small"
									color={chipColor(a.effective_result)}
									label={a.effective_result ?? (a.active ? "active" : "recovered")}
								/>
								<Typography variant="subtitle1" sx={{ fontWeight: 600 }}>
									{a.title ?? a.ref}
								</Typography>
								<Box sx={{ flex: 1 }} />
								{isAdmin && (
									<GradedAction calls="self_alerts/resolve">
										<Button
											size="small"
											color="error"
											variant="outlined"
											onClick={() => onResolve(a.id)}
										>
											Resolve
										</Button>
									</GradedAction>
								)}
							</Stack>
							<Typography
								variant="body2"
								sx={{ whiteSpace: "pre-wrap", fontFamily: "monospace" }}
							>
								{a.message}
							</Typography>
							<QuietChecks detail={a.detail} />
							<Typography variant="caption" color="text.secondary">
								{a.ref} · first seen{" "}
								{new Date(a.first_seen).toLocaleString()} · last activity{" "}
								{new Date(a.last_seen).toLocaleString()}
							</Typography>
						</Stack>
					</Paper>
				))
			)}

			{rest.length > 0 && (
				<Box>
					<Typography variant="h6" component="h2" gutterBottom>
						Recovered and resolved
					</Typography>
					<Stack spacing={1}>
						{rest.map((a) => (
							<Paper key={a.id} variant="outlined" sx={{ p: 1.5 }}>
								<Stack
									direction="row"
									spacing={1}
									sx={{ alignItems: "center", flexWrap: "wrap" }}
								>
									<Chip
										size="small"
										label={a.resolved_at ? "resolved" : "recovered"}
									/>
									<Typography variant="body2">{a.title ?? a.ref}</Typography>
									<Box sx={{ flex: 1 }} />
									<Typography variant="caption" color="text.secondary">
										{a.resolved_by ? `by ${a.resolved_by} · ` : ""}
										last activity {new Date(a.last_seen).toLocaleString()}
									</Typography>
								</Stack>
							</Paper>
						))}
					</Stack>
				</Box>
			)}
		</Stack>
	);
}
