import DnsIcon from "@mui/icons-material/Dns";
import VpnKeyIcon from "@mui/icons-material/VpnKey";
import { Avatar } from "@mui/material";
import type { AuditActor } from "../types";
import UserAvatar from "./UserAvatar";

/** The avatar of whoever took an audited action. Humans are in colour and
 * everything else is greyscale. */
export default function ActorAvatar({
	actor,
	size = 24,
}: {
	actor: AuditActor;
	size?: number;
}) {
	const sx = { width: size, height: size };
	switch (actor.kind) {
		case "operator":
			return <UserAvatar login={actor.login} name={actor.name} size={size} />;
		case "device":
			return (
				<Avatar sx={sx}>
					<DnsIcon sx={{ fontSize: size * 0.62 }} />
				</Avatar>
			);
		case "token":
			return (
				<Avatar sx={sx}>
					<VpnKeyIcon sx={{ fontSize: size * 0.62 }} />
				</Avatar>
			);
		case "canopy":
			return (
				<Avatar
					src="/favicon.svg"
					alt="Canopy"
					sx={{ ...sx, bgcolor: "transparent", filter: "grayscale(1)" }}
				/>
			);
	}
}
