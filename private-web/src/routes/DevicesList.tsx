import {
	Alert,
	Box,
	Button,
	LinearProgress,
	Pagination,
	Stack,
} from "@mui/material";
import AddIcon from "@mui/icons-material/Add";
import { useState } from "react";
import DeviceShorty from "../components/DeviceShorty";
import { GradedAction } from "../components/GradedAction";
import ProvisionCredentialDialog from "../components/ProvisionCredentialDialog";
import { useApi } from "../api";
import { usePageTitle } from "../hooks/usePageTitle";

const PAGE_SIZE = 10;

export default function DevicesList() {
	usePageTitle("Devices");
	const [page, setPage] = useState(0);
	const [createOpen, setCreateOpen] = useState(false);

	const result = useApi(
		"devices",
		"list_trusted",
		{ offset: page * PAGE_SIZE, limit: PAGE_SIZE },
		[page],
	);

	const total = result.status === "ok" ? result.data.total : 0;
	const pageCount = Math.max(1, Math.ceil(total / PAGE_SIZE));

	return (
		<Stack spacing={2}>
			<Box sx={{ display: "flex", justifyContent: "flex-end" }}>
				<GradedAction calls="devices/provision_credential">
					<Button
						variant="contained"
						startIcon={<AddIcon />}
						onClick={() => setCreateOpen(true)}
					>
						Create device
					</Button>
				</GradedAction>
				<ProvisionCredentialDialog
					open={createOpen}
					onClose={() => setCreateOpen(false)}
					onProvisioned={() => result.reload()}
				/>
			</Box>
			{result.status === "loading" || result.status === "idle" ? (
				<LinearProgress />
			) : result.status === "error" ? (
				<Alert severity="error">{result.error.message}</Alert>
			) : result.data.items.length === 0 ? (
				<Alert severity="info">No devices found.</Alert>
			) : (
				<Stack spacing={1}>
					{result.data.items.map((d) => (
						<DeviceShorty key={d.device.id} device={d} />
					))}
				</Stack>
			)}
			{pageCount > 1 && (
				<Box sx={{ display: "flex", justifyContent: "center" }}>
					<Pagination
						count={pageCount}
						page={page + 1}
						onChange={(_, p) => setPage(p - 1)}
					/>
				</Box>
			)}
		</Stack>
	);
}
