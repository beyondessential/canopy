import { Outlet } from "react-router-dom";

/// Settings groups the infrequently-used admin pages under one section of the
/// sidebar, so they don't each hold a place among the sections.
export default function Settings() {
	return <Outlet />;
}
