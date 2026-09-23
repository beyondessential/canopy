import {
	Alert,
	Button,
	LinearProgress,
	Paper,
	Stack,
	TextField,
	Typography,
} from "@mui/material";
import { useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { callApi, useApi } from "../api";
import { usePageTitle } from "../hooks/usePageTitle";
import type { ClusterDetail } from "../types";

/// Editing a cluster from its page: its name and how long it may go unheard.
/// Registering it and re-issuing its relay's credential stay on the registry.
/// spec: K8S
export default function ClusterEdit() {
	const { id = "" } = useParams<{ id: string }>();
	const detail = useApi("fleet/clusters", "get_detail", { cluster_id: id }, [id]);
	usePageTitle(
		detail.status === "ok" ? `Edit ${detail.data.cluster.name}` : "Edit cluster",
	);

	if (detail.status === "loading" || detail.status === "idle") {
		return <LinearProgress />;
	}
	if (detail.status === "error") {
		return <Alert severity="error">{detail.error.message}</Alert>;
	}
	return <Form cluster={detail.data.cluster} />;
}

function Form({ cluster }: { cluster: ClusterDetail["cluster"] }) {
	const navigate = useNavigate();
	const [name, setName] = useState(cluster.name);
	const [minutes, setMinutes] = useState(
		Math.max(1, Math.round(cluster.alert_when_down_for / 60)).toString(),
	);
	const [pending, setPending] = useState(false);
	const [error, setError] = useState<string | null>(null);

	const onSubmit = async (e: React.FormEvent) => {
		e.preventDefault();
		setPending(true);
		setError(null);
		try {
			await callApi("fleet/clusters", "update", {
				cluster_id: cluster.id,
				name: name.trim(),
				alert_when_down_for: Math.max(60, Math.round(Number(minutes) * 60)),
			});
			navigate(`/fleet/clusters/${cluster.id}`);
		} catch (err) {
			setError(err instanceof Error ? err.message : String(err));
		} finally {
			setPending(false);
		}
	};

	return (
		<Stack spacing={3} component="form" onSubmit={onSubmit}>
			<Typography variant="h5" component="h1">
				Edit {cluster.name}
			</Typography>

			<Paper variant="outlined" sx={{ p: 3 }}>
				<Stack spacing={2}>
					<TextField
						label="Name"
						value={name}
						onChange={(e) => setName(e.target.value)}
						disabled={pending}
						required
					/>
					<Stack
						direction={{ xs: "column", md: "row" }}
						spacing={2}
						sx={{ alignItems: { md: "center" } }}
					>
						<Typography variant="body2">
							File an issue when this cluster is unreachable for
						</Typography>
						<TextField
							label="minutes"
							type="number"
							value={minutes}
							onChange={(e) => setMinutes(e.target.value)}
							disabled={pending}
							slotProps={{ htmlInput: { min: 1, step: 1 } }}
							sx={{ width: 140 }}
						/>
					</Stack>
				</Stack>
			</Paper>

			{error && <Alert severity="error">{error}</Alert>}

			<Stack direction="row" spacing={1}>
				<Button
					type="submit"
					variant="contained"
					disabled={pending || name.trim() === ""}
				>
					{pending ? "Saving…" : "Save"}
				</Button>
				<Button
					type="button"
					variant="outlined"
					color="error"
					onClick={() => navigate(`/fleet/clusters/${cluster.id}`)}
					disabled={pending}
				>
					Cancel
				</Button>
			</Stack>
		</Stack>
	);
}
