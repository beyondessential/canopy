import { Avatar, Tooltip } from "@mui/material";

/** A background colour for a login, the same each time, so people are told
 * apart at a glance where they have no profile picture. */
export function loginColour(login: string): string {
	let hash = 0;
	for (const ch of login) hash = (hash * 31 + ch.codePointAt(0)!) >>> 0;
	return `hsl(${hash % 360}, 45%, 38%)`;
}

/** Small circular avatar for a Tailscale user. Falls back to the first
 * letter of the login or name, on a colour derived from the login, when no
 * profile picture is available.
 *
 * Pass `tooltip={false}` when a parent already wraps this in its own tooltip,
 * to avoid stacking two tooltips on the same element. */
export default function UserAvatar({
	login,
	name,
	profilePic,
	size = 24,
	tooltip = true,
}: {
	login: string | null;
	name?: string | null;
	profilePic?: string | null;
	size?: number;
	tooltip?: boolean;
}) {
	const display = name ?? login ?? "?";
	const initial = (display.trim()[0] ?? "?").toUpperCase();
	const avatar = (
		<Avatar
			src={profilePic ?? undefined}
			alt={display}
			sx={{
				width: size,
				height: size,
				fontSize: size * 0.55,
				bgcolor: loginColour(login ?? display),
				color: "#fff",
			}}
		>
			{initial}
		</Avatar>
	);
	if (!tooltip) return avatar;
	return <Tooltip title={display}>{avatar}</Tooltip>;
}
