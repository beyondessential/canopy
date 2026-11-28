import {
	Alert,
	Box,
	Button,
	IconButton,
	LinearProgress,
	Paper,
	Snackbar,
	Stack,
	Table,
	TableBody,
	TableCell,
	TableHead,
	TableRow,
	TextField,
	Typography,
} from "@mui/material";
import CloseIcon from "@mui/icons-material/Close";
import { type FormEvent, useState } from "react";
import { ApiError, callApi, useApi } from "../api";
import { GradedAction } from "../components/GradedAction";
import TimeAgo from "../components/TimeAgo";
import { useAdminStatus } from "../hooks/useIsAdmin";
import { usePageTitle } from "../hooks/usePageTitle";
import type { ApiResponse, ListLevel } from "../types";
import NotFound from "./NotFound";

type Overview = ApiResponse<"access", "list">;
type Holders = Overview["levels"][number];
type Coverage = Holders["granted_to"][number]["covers"];

const LEVEL_NAMES: Record<ListLevel, string> = { admin: "Admin", pm: "PM" };

const LEVEL_POWERS: Record<ListLevel | "all", string> = {
	admin: "Everything, including Canopy's settings.",
	pm: "Edit groups' project details.",
	all: "Read everything not kept to admins.",
};

const mono = { fontFamily: "monospace" } as const;
const cellSx = { py: 1.5, verticalAlign: "top" } as const;

/** The levels, who holds each and how, and how to grant one on the tailnet.
 * Only an administrator sees it. spec: ADM */
export default function Permissions() {
	usePageTitle("Permissions");
	const { resolved, isAdmin } = useAdminStatus();
	if (!resolved) return null;
	if (!isAdmin) return <NotFound />;
	return <PermissionsPage />;
}

function PermissionsPage() {
	const overview = useApi("access", "list");

	if (overview.status === "loading" || overview.status === "idle") {
		return <LinearProgress />;
	}
	if (overview.status === "error") {
		return <Alert severity="error">{overview.error.message}</Alert>;
	}
	const { levels, policy } = overview.data;

	return (
		<Stack spacing={3}>
			<Paper variant="outlined">
				<Table aria-label="Access levels">
					<TableHead>
						<TableRow>
							<TableCell sx={{ width: "26%" }}>Level</TableCell>
							<TableCell sx={{ width: "34%" }}>Tailnet grant</TableCell>
							<TableCell>Email list</TableCell>
						</TableRow>
					</TableHead>
					<TableBody>
						{levels.map((holders) => (
							<LevelRow
								key={holders.level}
								holders={holders}
								policyRead={policy.configured && policy.read_at != null}
								onChange={overview.reload}
							/>
						))}
						<TableRow aria-label="All">
							<LevelCell name="All" powers={LEVEL_POWERS.all} />
							<TableCell colSpan={2} sx={cellSx}>
								<Typography variant="body2" color="text.secondary">
									Everyone on the tailnet who can reach Canopy
								</Typography>
							</TableCell>
						</TableRow>
					</TableBody>
				</Table>
			</Paper>
			<GrantingPanel policy={overview.data.policy} />
		</Stack>
	);
}

function LevelCell({ name, powers }: { name: string; powers: string }) {
	return (
		<TableCell component="th" scope="row" sx={cellSx}>
			<Typography variant="body2" sx={{ fontWeight: 500 }}>
				{name}
			</Typography>
			<Typography variant="caption" color="text.secondary">
				{powers}
			</Typography>
		</TableCell>
	);
}

function LevelRow({
	holders,
	policyRead,
	onChange,
}: {
	holders: Holders;
	/** Whether the policy has been read, so an empty grant is known to be empty. */
	policyRead: boolean;
	onChange: () => void;
}) {
	const name = LEVEL_NAMES[holders.level];
	return (
		<TableRow aria-label={name}>
			<LevelCell name={name} powers={LEVEL_POWERS[holders.level]} />
			<TableCell sx={cellSx}>
				<Typography variant="body2" sx={mono}>
					{holders.key}: true
				</Typography>
				{holders.granted_to.length > 0 ? (
					holders.granted_to.map(({ source, covers }) => (
						<Typography
							key={source}
							variant="caption"
							color="text.secondary"
							component="div"
						>
							<Box component="span" sx={mono}>
								{source}
							</Box>{" "}
							({coverage(covers)})
						</Typography>
					))
				) : (
					<Typography variant="caption" color="text.secondary" component="div">
						{policyRead ? "Granted to no one" : "Policy not read"}
					</Typography>
				)}
			</TableCell>
			<TableCell sx={cellSx}>
				<LevelList holders={holders} name={name} onChange={onChange} />
			</TableCell>
		</TableRow>
	);
}

function coverage(covers: Coverage): string {
	switch (covers.kind) {
		case "group":
			return covers.members === 1 ? "1 person" : `${covers.members} people`;
		case "person":
			return "one person";
		case "everyone":
			return "everyone on the tailnet";
		case "no_one":
			return "no one";
	}
}

function LevelList({
	holders,
	name,
	onChange,
}: {
	holders: Holders;
	name: string;
	onChange: () => void;
}) {
	const [login, setLogin] = useState("");
	const [pending, setPending] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const [toast, setToast] = useState<string | null>(null);

	const onAdd = async (e: FormEvent) => {
		e.preventDefault();
		const trimmed = login.trim();
		if (!trimmed) return setError("Email cannot be empty");
		if (!trimmed.includes("@")) return setError("Please enter a valid email address");
		setPending(true);
		setError(null);
		try {
			await callApi("access", "add", { level: holders.level, login: trimmed });
			setLogin("");
			setToast(`Added to ${name}`);
			onChange();
		} catch (err) {
			setError(formatError(err));
		} finally {
			setPending(false);
		}
	};

	const onRemove = async (target: string) => {
		try {
			await callApi("access", "remove", { level: holders.level, login: target });
			onChange();
		} catch (err) {
			setError(formatError(err));
		}
	};

	return (
		<>
			{holders.list.length > 0 && (
				<Stack
					component="ul"
					aria-label={`${name} list`}
					spacing={0.5}
					sx={{ listStyle: "none", m: 0, mb: 1, p: 0 }}
				>
					{holders.list.map(({ login: listed }) => (
						<Stack
							component="li"
							key={listed}
							direction="row"
							sx={{ alignItems: "center", gap: 0.5 }}
						>
							<Typography variant="body2" sx={{ ...mono, flex: 1 }}>
								{listed}
							</Typography>
							<GradedAction
								calls="access/remove"
								action={`Remove ${listed} from ${name}`}
							>
								<IconButton
									size="small"
									aria-label={`Remove ${listed} from ${name}`}
									onClick={() => onRemove(listed)}
								>
									<CloseIcon fontSize="small" />
								</IconButton>
							</GradedAction>
						</Stack>
					))}
				</Stack>
			)}
			<Box component="form" onSubmit={onAdd}>
				<Stack direction="row" spacing={1} sx={{ alignItems: "flex-start" }}>
					<TextField
						type="email"
						size="small"
						fullWidth
						placeholder="Add email"
						value={login}
						onChange={(e) => setLogin(e.target.value)}
						disabled={pending}
						slotProps={{ htmlInput: { "aria-label": `Add email to ${name}` } }}
					/>
					<GradedAction calls="access/add" action={`Add ${name}`}>
						<Button
							type="submit"
							variant="contained"
							disabled={pending}
							aria-label={`Add ${name}`}
							sx={{ whiteSpace: "nowrap", flexShrink: 0 }}
						>
							{pending ? "Adding…" : "Add"}
						</Button>
					</GradedAction>
				</Stack>
				{error && (
					<Alert severity="error" sx={{ mt: 1 }}>
						{error}
					</Alert>
				)}
			</Box>
			<Snackbar
				open={!!toast}
				autoHideDuration={3000}
				onClose={() => setToast(null)}
				message={toast ?? ""}
			/>
		</>
	);
}

function GrantingPanel({ policy }: { policy: Overview["policy"] }) {
	const example = JSON.stringify(
		{
			src: ["group:example"],
			dst: [policy.service_tag],
			app: { [policy.capability]: [{ pm: true, home: "/fleet/pm" }] },
		},
		null,
		2,
	);
	return (
		<Paper variant="outlined" sx={{ p: 2 }}>
			<Typography variant="h6" component="h2" gutterBottom>
				Granting a level on the tailnet
			</Typography>
			<Typography variant="body2" sx={{ mb: 1.5 }}>
				Add a grant to the tailnet policy with{" "}
				<Box component="span" sx={mono}>
					{policy.service_tag}
				</Box>{" "}
				as its destination and the{" "}
				<Box component="span" sx={mono}>
					{policy.capability}
				</Box>{" "}
				capability. Its sources get every level whose key is true. Each level
				includes the ones below it.
			</Typography>
			<Box
				component="pre"
				sx={{
					...mono,
					m: 0,
					px: 2,
					py: 1.5,
					bgcolor: "action.hover",
					borderRadius: 1,
					overflowX: "auto",
					lineHeight: 1.5,
				}}
			>
				{example}
			</Box>
			<Typography variant="body2" sx={{ mt: 1.5 }}>
				<Box component="span" sx={mono}>
					home
				</Box>{" "}
				sets the default homepage for people who haven't chosen one. When
				grants disagree, the highest{" "}
				<Box component="span" sx={mono}>
					priority
				</Box>{" "}
				wins, then the path with the most segments, then the alphabetically
				first.
			</Typography>
			<Typography
				variant="caption"
				color="text.secondary"
				component="div"
				sx={{ mt: 1 }}
			>
				{!policy.configured ? (
					"Canopy isn't configured to read the tailnet policy."
				) : policy.read_at ? (
					<>
						Policy read <TimeAgo timestamp={policy.read_at} />.
					</>
				) : (
					"The policy hasn't been read yet."
				)}
			</Typography>
		</Paper>
	);
}

function formatError(err: unknown): string {
	if (err instanceof ApiError) {
		const detail = err.detail as { title?: string } | null;
		if (detail?.title) return detail.title;
		return err.message;
	}
	if (err instanceof Error) return err.message;
	return String(err);
}
