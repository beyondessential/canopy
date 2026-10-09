import { Chip } from "@mui/material";
import type { CertificateView } from "../types";

/// A certificate's state as the chip an operator reads it by. Its colour is
/// what the time left is coloured by too (see `TimeLeft`), so the two change
/// together.
// spec: CRT#presentation
export default function CertificateStateChip({
	cert,
}: {
	cert: Pick<CertificateView, "state" | "collectable" | "risk">;
}) {
	if (cert.state === "revoked")
		return <Chip size="small" color="error" label="revoked" />;
	if (cert.state === "failed")
		return <Chip size="small" color="error" variant="outlined" label="failed" />;
	if (cert.state === "pending" && !cert.collectable)
		return <Chip size="small" variant="outlined" label="pending" />;
	if (cert.risk === "critical")
		return <Chip size="small" color="error" label="expiring" />;
	if (cert.risk === "at_risk")
		return <Chip size="small" color="warning" label="due for renewal" />;
	return <Chip size="small" color="success" variant="outlined" label="valid" />;
}
