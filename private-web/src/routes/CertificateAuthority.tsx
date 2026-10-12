import {
	Alert,
	AlertTitle,
	Box,
	Chip,
	LinearProgress,
	Paper,
	Stack,
	Typography,
} from "@mui/material";
import { useApi } from "../api";

/// The certificate authority Canopy is configured to use, its advertised
/// profiles, and whether Canopy's account with it is usable.
///
/// Here rather than on a server's page because that is where a misconfiguration
/// of issuance actually shows up: nothing in the fleet can obtain a certificate
/// when the account is wrong, so blaming any one group would send an
/// operator to the wrong place.
// spec: CRT#issuance-authority
export default function CertificateAuthority() {
	const authority = useApi("certificates", "authority", {}, []);

	if (authority.status === "loading" || authority.status === "idle") {
		return (
			<Paper variant="outlined" sx={{ p: 2 }}>
				<Heading />
				<LinearProgress />
			</Paper>
		);
	}
	if (authority.status === "error") {
		return (
			<Paper variant="outlined" sx={{ p: 2 }}>
				<Heading />
				<Alert severity="error">{authority.error.message}</Alert>
			</Paper>
		);
	}

	const data = authority.data;

	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<Heading />
			<Stack spacing={2}>
				{!data.directory ? (
					<Alert severity="info">
						<AlertTitle>No certificate authority configured</AlertTitle>
						Canopy issues no TLS certificates. Servers can still register their
						public names and have their address records published; a certificate
						request is accepted and left pending. The authority is set by the
						infrastructure that provisions Canopy.
					</Alert>
				) : (
					<Box>
						<Typography variant="subtitle2">Directory</Typography>
						<Typography
							variant="body2"
							sx={{ fontFamily: "monospace", wordBreak: "break-all" }}
						>
							{data.directory}
						</Typography>
					</Box>
				)}

				{data.problem ? (
					<Alert severity="error">
						<AlertTitle>Canopy cannot issue certificates</AlertTitle>
						{data.problem}
					</Alert>
				) : (
					data.directory && (
						<Alert severity={data.account_usable ? "success" : "warning"}>
							{data.account_usable
								? "Canopy holds a usable account at this authority."
								: "Canopy could not use its account at this authority when it last started. Certificates are not being issued."}
						</Alert>
					)
				)}

				<Box>
					<Typography variant="subtitle2" gutterBottom>
						Profiles on offer
					</Typography>
					{data.profiles.length === 0 ? (
						<Typography variant="body2" color="text.secondary">
							This authority advertises no profiles, so it decides each
							certificate's lifetime itself. Every server takes that default.
						</Typography>
					) : (
						<>
							<Stack direction="row" spacing={1} sx={{ flexWrap: "wrap", rowGap: 1 }}>
								{data.profiles.map((profile) => (
									<Chip key={profile} label={profile} variant="outlined" />
								))}
							</Stack>
							<Typography
								variant="caption"
								color="text.secondary"
								sx={{ display: "block", mt: 1 }}
							>
								A profile is the authority's name for a lifetime. Each server's is
								set on its own page, because lifetime is a property of how a
								server is run: a cloud server whose issuance is exercised
								constantly can carry a short one where an on-premises server
								that may be offline for days cannot. Every server takes the
								authority's default until an operator chooses otherwise.
							</Typography>
						</>
					)}
				</Box>
			</Stack>
		</Paper>
	);
}

function Heading() {
	return (
		<Typography variant="h6" component="h2" gutterBottom>
			Certificate authority
		</Typography>
	);
}
