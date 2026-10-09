import { Tooltip, Typography } from "@mui/material";
import { useEffect, useState } from "react";

const MINUTE = 60;
const HOUR = 3600;
const DAY = 86400;

/// How long until, or since, an instant, in whole units that suit its size.
/// Floored in every unit, so the same moment reads the same wherever it is shown.
export function describeTimeLeft(notAfter: number, now: number): string {
	const secs = (notAfter - now) / 1000;
	const total = Math.abs(secs);
	const unit = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;
	const spelled =
		total >= DAY
			? unit(Math.floor(total / DAY), "day")
			: total >= HOUR
				? unit(Math.floor(total / HOUR), "hour")
				: total >= MINUTE
					? unit(Math.floor(total / MINUTE), "minute")
					: "under a minute";
	return secs < 0 ? `expired ${spelled} ago` : `expires in ${spelled}`;
}

const RISK_COLOURS: Record<string, string> = {
	none: "text.secondary",
	at_risk: "warning.main",
	critical: "error.main",
};

/// How long a certificate has left, shown once, with the exact instant on hover.
///
/// Coloured by the certificate's own state rather than by a fixed number of
/// days: the API judges `risk` against the certificate's lifetime and renewal
/// point, the same measure the state chip uses, so the two change together.
// spec: CRT#presentation
export default function TimeLeft({
	notAfter,
	risk,
}: {
	notAfter: string;
	risk: string | null;
}) {
	const ts = Date.parse(notAfter);
	const [now, setNow] = useState(() => Date.now());

	useEffect(() => {
		if (Number.isNaN(ts)) return;
		const id = window.setInterval(() => {
			if (!document.hidden) setNow(Date.now());
		}, 10_000);
		return () => window.clearInterval(id);
	}, [ts]);

	if (Number.isNaN(ts)) return <span>?</span>;

	return (
		<Tooltip title={new Date(ts).toLocaleString()}>
			<Typography
				variant="caption"
				component="span"
				data-risk={risk ?? "none"}
				sx={{ color: RISK_COLOURS[risk ?? "none"] ?? "text.secondary" }}
			>
				{describeTimeLeft(ts, now)}
			</Typography>
		</Tooltip>
	);
}
