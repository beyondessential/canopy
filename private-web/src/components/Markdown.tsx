import { Box, type SxProps, type Theme } from "@mui/material";
import ReactMarkdown from "react-markdown";
import rehypeRaw from "rehype-raw";
import rehypeSanitize from "rehype-sanitize";
import remarkBreaks from "remark-breaks";
import remarkGfm from "remark-gfm";

/** Renders markdown with GFM extensions and a sanitised subset of inline
 * HTML. Sources sometimes wrap their messages in `<b>`, `<a>`, `<code>`
 * and similar — those pass through; anything dangerous (scripts, inline
 * event handlers, iframes, etc.) is stripped by `rehype-sanitize`'s
 * default GitHub-compatible schema. Pass `preserveNewlines` when input
 * may be plain text with significant line breaks (e.g. log messages),
 * which converts soft breaks to `<br>`. */
export default function Markdown({
	children,
	preserveNewlines = false,
	sx,
}: {
	children: string;
	preserveNewlines?: boolean;
	sx?: SxProps<Theme>;
}) {
	const remarkPlugins = preserveNewlines
		? [remarkGfm, remarkBreaks]
		: [remarkGfm];
	return (
		<Box
			sx={[
				{
					"& h1, & h2, & h3, & h4, & h5, & h6": {
						mt: 2,
						mb: 1,
					},
					"& p": { my: 1 },
					"& ul, & ol": { pl: 3, my: 1 },
					"& code": {
						fontFamily: "monospace",
						px: 0.5,
						borderRadius: 0.5,
						bgcolor: "action.hover",
					},
					"& pre code": { bgcolor: "transparent", p: 0 },
					"& pre": {
						p: 1.5,
						borderRadius: 1,
						bgcolor: "action.hover",
						overflow: "auto",
					},
					"& table": {
						borderCollapse: "collapse",
						my: 1,
					},
					"& th, & td": {
						border: 1,
						borderColor: "divider",
						p: 0.5,
					},
					"& a": {
						color: "primary.main",
						textDecoration: "underline",
					},
					"& blockquote": {
						borderLeft: 3,
						borderColor: "divider",
						ml: 0,
						pl: 2,
						color: "text.secondary",
					},
				},
				...(Array.isArray(sx) ? sx : [sx]),
			]}
		>
			<ReactMarkdown
				remarkPlugins={remarkPlugins}
				rehypePlugins={[rehypeRaw, rehypeSanitize]}
			>
				{children}
			</ReactMarkdown>
		</Box>
	);
}

/** Text an operator or agent wrote: notes on a record, an issue or an
 * incident, and incident memory entries. Rendered at body size with single
 * line breaks kept, since these were long shown as preformatted text, and
 * without outer margins so it sits flush in whatever card holds it. */
export function NoteText({
	children,
	color,
}: {
	children: string;
	color?: string;
}) {
	return (
		<Markdown
			preserveNewlines
			sx={{
				typography: "body2",
				color,
				overflowWrap: "anywhere",
				"& > :first-child": { mt: 0 },
				"& > :last-child": { mb: 0 },
			}}
		>
			{children}
		</Markdown>
	);
}
