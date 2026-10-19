import { Box, Typography } from "@mui/material";
import { Outlet, useParams } from "react-router-dom";
import { useApi } from "../api";

export default function Servers() {
	const params = useParams<{ id?: string }>();

	return (
		<Box>
			{params.id && <ServerNameBreadcrumb serverId={params.id} />}
			<Outlet />
		</Box>
	);
}

function ServerNameBreadcrumb({ serverId }: { serverId: string }) {
	const result = useApi(
		"fleet/applications",
		"get_name",
		{ server_id: serverId },
		[serverId],
	);
	if (result.status !== "ok") return null;
	return (
		<Typography variant="body2" color="text.secondary" sx={{ mb: 1 }}>
			{result.data}
		</Typography>
	);
}
