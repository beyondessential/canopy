import type { MaintenanceWindow } from "../types";
import TimeAgo from "./TimeAgo";

/// When a window ends, as the sentence reads it: a window a run declared lasts
/// until the run does, wherever an expected end would be shown.
// spec: MNT#presentation
export default function WindowEnd({ window }: { window: MaintenanceWindow }) {
	if (window.lease_id) return <>when the run ends</>;
	return <TimeAgo timestamp={window.expected_end} />;
}
