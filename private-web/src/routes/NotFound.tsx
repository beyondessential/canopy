import { Alert } from "@mui/material";
import { Link } from "react-router-dom";
import { usePageTitle } from "../hooks/usePageTitle";

/** What any path naming no page opens. */
export default function NotFound() {
	usePageTitle("Page not found");
	return (
		<Alert severity="info">
			There's no page here. <Link to="/status">Go to the status page</Link>.
		</Alert>
	);
}
