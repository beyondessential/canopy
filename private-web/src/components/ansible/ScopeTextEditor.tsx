import {
	type Completion,
	type CompletionContext,
	autocompletion,
} from "@codemirror/autocomplete";
import { type Diagnostic, linter, lintGutter } from "@codemirror/lint";
import { Prec } from "@codemirror/state";
import { keymap } from "@codemirror/view";
import { Alert, Box, Button, Stack, Typography, useTheme } from "@mui/material";
import CodeMirror from "@uiw/react-codemirror";
import { useMemo, useRef, useState } from "react";
import { useApiAction } from "../../api";
import type { CatalogueView } from "../../types";
import { GradedAction } from "../GradedAction";
import { type ScopeKind, describeType, offered } from "./schema";
import {
	type ScopeVariable,
	checkScope,
	diffScope,
	formatScope,
	parseScope,
} from "./yamlScope";

/// A scope's variables edited as text: one `name: value` per line, YAML values,
/// names completing from the catalogue. Saving applies only what changed, as
/// variables set and removed, and is refused while a line breaks a rule.
// spec: ANSVAR#presentation
export default function ScopeTextEditor({
	scope,
	kind,
	applicationTypes,
	catalogue,
	variables,
	onClose,
	onSaved,
}: {
	/** The scope arguments of each call. */
	scope: Record<string, unknown>;
	kind: ScopeKind;
	applicationTypes: ReadonlyArray<string>;
	catalogue: CatalogueView | null;
	variables: ReadonlyArray<ScopeVariable>;
	onClose: () => void;
	onSaved: () => void;
}) {
	const theme = useTheme();
	const [text, setText] = useState(() => formatScope(variables));
	const set = useApiAction("ansible_variables", "set");
	const remove = useApiAction("ansible_variables", "remove");
	const [failure, setFailure] = useState<string | null>(null);
	const [saving, setSaving] = useState(false);

	const parsed = useMemo(() => parseScope(text), [text]);
	const found = useMemo(
		() => checkScope(parsed, variables, kind, applicationTypes, catalogue),
		[parsed, variables, kind, applicationTypes, catalogue],
	);
	const changes = useMemo(
		() => diffScope(variables, parsed, catalogue),
		[variables, parsed, catalogue],
	);
	const count = changes.set.length + changes.remove.length;
	const ready = found.length === 0 && count > 0 && !saving;

	const save = async () => {
		if (!ready) return;
		setSaving(true);
		setFailure(null);
		try {
			// Removals first, so a name that moves between a plain value and a
			// secret does not meet its own earlier form.
			for (const name of changes.remove) {
				await remove.call({ ...scope, name });
			}
			for (const change of changes.set) {
				await set.call({ ...scope, ...change });
			}
			onSaved();
			onClose();
		} catch (error) {
			setFailure(error instanceof Error ? error.message : String(error));
			setSaving(false);
		}
	};
	const latest = useRef({ save, onClose });
	latest.current = { save, onClose };

	const extensions = useMemo(() => {
		const names = (catalogue?.variables ?? []).filter((variable) =>
			offered(variable, kind, applicationTypes),
		);
		const complete = (context: CompletionContext) => {
			const word = context.matchBefore(/^[-._a-zA-Z0-9]*/);
			if (!word || (word.from === word.to && !context.explicit)) return null;
			const setAt = new Map(
				parseScope(context.state.doc.toString()).entries.map((entry) => [
					entry.name,
					entry.line,
				]),
			);
			const options: Completion[] = names.map((variable) => ({
				label: variable.name,
				detail: setAt.has(variable.name)
					? `set on line ${setAt.get(variable.name)}`
					: describeType(variable),
				info: variable.description ?? undefined,
				apply: `${variable.name}: `,
				boost: setAt.has(variable.name) ? -99 : 0,
			}));
			return { from: word.from, options, validFor: /^[-._a-zA-Z0-9]*$/ };
		};
		return [
			Prec.highest(
				keymap.of([
					{
						key: "Mod-Enter",
						preventDefault: true,
						run: () => {
							void latest.current.save();
							return true;
						},
					},
					{
						key: "Escape",
						run: () => {
							latest.current.onClose();
							return true;
						},
					},
				]),
			),
			autocompletion({ override: [complete], activateOnTyping: true }),
			lintGutter(),
			linter((view): Diagnostic[] => {
				const checked = checkScope(
					parseScope(view.state.doc.toString()),
					variables,
					kind,
					applicationTypes,
					catalogue,
				);
				return checked.map((problem) => {
					const line = view.state.doc.line(
						Math.min(Math.max(problem.line, 1), view.state.doc.lines),
					);
					return {
						from: line.from,
						to: line.to,
						severity: "error",
						message: problem.message,
					};
				});
			}),
		];
	}, [catalogue, kind, applicationTypes, variables]);

	return (
		<Box data-testid="scope-text-editor">
			<CodeMirror
				value={text}
				minHeight="8em"
				theme={theme.palette.mode === "dark" ? "dark" : "light"}
				extensions={extensions}
				onChange={setText}
				basicSetup={{ lineNumbers: true, highlightActiveLine: true }}
				autoFocus
			/>
			<Stack
				direction="row"
				spacing={1}
				sx={{ mt: 1, alignItems: "center", justifyContent: "space-between" }}
			>
				<Typography variant="caption" color="error">
					{found[0] ? `Line ${found[0].line}: ${found[0].message}` : ""}
				</Typography>
				<Stack direction="row" spacing={1}>
					<Button size="small" onClick={onClose} data-testid="scope-discard">
						Discard
					</Button>
					<GradedAction
						calls="ansible_variables/set"
						action="Edit Ansible variables as text"
					>
						<Button
							size="small"
							variant="contained"
							disabled={!ready}
							onClick={() => void save()}
							data-testid="scope-save"
						>
							{count === 0
								? "Save"
								: `Save ${count} change${count === 1 ? "" : "s"}`}
						</Button>
					</GradedAction>
				</Stack>
			</Stack>
			{failure && (
				<Alert severity="warning" sx={{ mt: 1 }}>
					{failure}
				</Alert>
			)}
		</Box>
	);
}
