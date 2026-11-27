import { IconButton, Snackbar, Tooltip } from "@mui/material";
import HomeIcon from "@mui/icons-material/Home";
import HomeOutlinedIcon from "@mui/icons-material/HomeOutlined";
import { useState } from "react";
import { Navigate, useLocation } from "react-router-dom";
import { FALLBACK_HOMEPAGE, useHomepage } from "../hooks/useHomepage";

/**
 * The home toggle beside the Canopy mark: faint at rest, filled and stronger
 * on the person's homepage, full strength under the pointer. It chooses the
 * current page as the homepage, or clears the choice on the chosen page; on a
 * page that is home only by default it shows on and does nothing.
 *
 * Choosing a homepage is the person's own preference, so it needs no raised
 * mode. spec: HOME
 */
export function HomeToggle() {
	const { pathname } = useLocation();
	const homepage = useHomepage();
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);

	const loaded = homepage.effective !== undefined;
	const on = homepage.effective === pathname;
	const chosen = homepage.chosen === pathname;
	const label = !loaded
		? "Homepage"
		: !on
			? "Make this my homepage"
			: chosen
				? "Clear my homepage"
				: "This is your default homepage";

	const activate = async () => {
		if (!loaded || busy || (on && !chosen)) return;
		setBusy(true);
		try {
			if (chosen) await homepage.clear();
			else await homepage.set(pathname);
		} catch (err) {
			setError(err instanceof Error ? err.message : String(err));
		} finally {
			setBusy(false);
		}
	};

	return (
		<>
			<Tooltip title={label}>
				{/* The span keeps the tooltip working while the button is disabled. */}
				<span>
					<IconButton
						size="small"
						aria-label={label}
						aria-pressed={on}
						disabled={!loaded}
						onClick={activate}
						sx={{
							color: "text.primary",
							opacity: on ? 0.6 : 0.2,
							"&:hover, &:focus-visible": { opacity: 1 },
							"&.Mui-disabled": { color: "text.primary" },
						}}
					>
						{on ? (
							<HomeIcon fontSize="small" />
						) : (
							<HomeOutlinedIcon fontSize="small" />
						)}
					</IconButton>
				</span>
			</Tooltip>
			<Snackbar
				open={error !== null}
				autoHideDuration={5000}
				onClose={() => setError(null)}
				message={`Couldn't change your homepage: ${error ?? ""}`}
			/>
		</>
	);
}

/** The root: opens the person's homepage. A homepage that names no page opens
 * as any missing page does, through the catch-all route. spec: HOME */
export function HomeRedirect() {
	const { effective, error } = useHomepage();
	if (error) return <Navigate to={FALLBACK_HOMEPAGE} replace />;
	if (effective === undefined) return null;
	return <Navigate to={effective} replace />;
}
