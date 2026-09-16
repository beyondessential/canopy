/// Whole seconds as a single unit, keeping a half-hour visible: a 1.5h window
/// is a different night's work to a 2h one.
export function formatDuration(seconds: number) {
	if (seconds < 60) return `${seconds}s`;
	if (seconds < 3600) return `${Math.round(seconds / 60)}m`;
	const hours = seconds / 3600;
	return `${hours.toFixed(hours < 10 ? 1 : 0)}h`;
}
