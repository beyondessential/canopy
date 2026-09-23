import { Button, type ButtonProps } from "@mui/material";
import type { ReactNode } from "react";
import { Link as RouterLink } from "react-router-dom";

/** An icon button that reveals its text label on hover or keyboard focus.
 * Collapsed it shows just the icon; the label text stays in the DOM and the
 * accessible name is always set via `aria-label`, so it remains usable and
 * announceable while collapsed. Renders as an external link (`href`), an
 * in-app router link (`to`), or a plain action (`onClick`). Used to build the
 * server-detail action row, where actions accrete over time. */
export default function ActionButton({
	icon,
	label,
	color,
	title,
	href,
	to,
	onClick,
	disabled,
}: {
	icon: ReactNode;
	label: string;
	color?: ButtonProps["color"];
	/** Native tooltip; the visible label already appears on hover, so only
	 * set this to explain state the label can't (e.g. why it's coloured). */
	title?: string;
	href?: string;
	to?: string;
	onClick?: () => void;
	/** Forwarded to the button, so a caller that blocks this control (see
	 * `GradedAction`) takes it out of the keyboard's reach as well. */
	disabled?: boolean;
}) {
	const common = {
		variant: "outlined" as const,
		size: "small" as const,
		color,
		title,
		disabled,
		"aria-label": label,
		startIcon: icon,
		sx: {
			minWidth: 0,
			"& .MuiButton-startIcon": { mx: 0 },
			"& .action-label": {
				display: "inline-block",
				maxWidth: 0,
				ml: 0,
				overflow: "hidden",
				whiteSpace: "nowrap",
				opacity: 0,
				transition: (theme) =>
					theme.transitions.create(["max-width", "opacity", "margin"], {
						duration: theme.transitions.duration.shorter,
					}),
			},
			"&:hover .action-label, &:focus-visible .action-label": {
				maxWidth: "14rem",
				ml: 0.5,
				opacity: 1,
			},
		},
	} satisfies ButtonProps;

	const content = <span className="action-label">{label}</span>;

	if (href) {
		return (
			<Button
				{...common}
				component="a"
				href={href}
				target="_blank"
				rel="noopener noreferrer"
			>
				{content}
			</Button>
		);
	}
	if (to) {
		return (
			<Button {...common} component={RouterLink} to={to}>
				{content}
			</Button>
		);
	}
	return (
		<Button {...common} onClick={onClick}>
			{content}
		</Button>
	);
}
