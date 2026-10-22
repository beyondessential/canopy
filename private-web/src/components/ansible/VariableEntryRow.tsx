import LockIcon from "@mui/icons-material/Lock";
import LockOpenIcon from "@mui/icons-material/LockOpen";
import {
	Alert,
	Autocomplete,
	Box,
	Button,
	IconButton,
	Stack,
	TextField,
	Tooltip,
	Typography,
} from "@mui/material";
import { type ReactNode, useRef, useState } from "react";
import { useApiAction } from "../../api";
import type { CatalogueView } from "../../types";
import { GradedAction } from "../GradedAction";
import SchemaField from "./SchemaField";
import {
	type CatalogueVariable,
	type Schema,
	type ScopeKind,
	describeType,
	offered,
	problems,
} from "./schema";

const NAME = /^[-._a-zA-Z0-9]+$/;

/// The row that adds a variable to a scope, driven from the keyboard: the name
/// completes from the catalogue, Tab moves to a value field shaped by the
/// variable's type, Enter sets it, and focus returns to a fresh row.
// spec: ANSVAR#presentation
export default function VariableEntryRow({
	label,
	scope,
	kind,
	applicationTypes,
	catalogue,
	taken,
	onSet,
	children,
}: {
	/** What the row adds to, as the placeholder names it. */
	label: string;
	/** The scope arguments of the call. */
	scope: Record<string, unknown>;
	kind: ScopeKind;
	/** The application types the scope holds. */
	applicationTypes: ReadonlyArray<string>;
	catalogue: CatalogueView | null;
	/** Names already set here. */
	taken: ReadonlySet<string>;
	onSet: () => void;
	/** Beside the row, such as the import button. */
	children?: ReactNode;
}) {
	const [name, setName] = useState("");
	const [value, setValue] = useState<unknown>(undefined);
	const [secretToggle, setSecretToggle] = useState(false);
	const input = useRef<HTMLInputElement>(null);
	const set = useApiAction("ansible_variables", "set");

	const described: CatalogueVariable | undefined = catalogue?.variables.find(
		(variable) => variable.name === name,
	);
	const secret = described ? described.secret : secretToggle;
	const schema: Schema = (described?.schema as Schema | undefined) ?? {};

	const options = (catalogue?.variables ?? [])
		.filter((variable) => offered(variable, kind, applicationTypes))
		.sort(
			(a, b) =>
				Number(taken.has(a.name)) - Number(taken.has(b.name)) ||
				a.name.localeCompare(b.name),
		);

	const complete =
		NAME.test(name) &&
		value !== undefined &&
		problems(schema, value).length === 0;

	const submit = () => {
		if (!complete || set.pending) return;
		set
			.call({ ...scope, name, value, secret })
			.then(() => {
				setName("");
				setValue(undefined);
				setSecretToggle(false);
				onSet();
				// A fresh row of the same scope, so several are entered in a run.
				window.setTimeout(() => input.current?.focus(), 0);
			})
			.catch(() => {
				// Surfaced by the alert below.
			});
	};

	return (
		<Box data-testid="set-variable">
			<GradedAction
				calls="ansible_variables/set"
				action={`Set an Ansible variable on ${label}`}
			>
				<Stack
					direction="row"
					spacing={1}
					useFlexGap
					sx={{ flexWrap: "wrap", alignItems: "flex-start" }}
				>
					<Autocomplete
						freeSolo
						autoHighlight
						autoSelect
						size="small"
						options={options}
						inputValue={name}
						onInputChange={(_, next) => {
							setName(next);
							setValue(undefined);
						}}
						getOptionLabel={(option) =>
							typeof option === "string" ? option : option.name
						}
						filterOptions={(all, state) =>
							all.filter((option) =>
								option.name
									.toLowerCase()
									.includes(state.inputValue.toLowerCase()),
							)
						}
						renderOption={(props, option) => (
							<Box
								component="li"
								{...props}
								key={option.name}
								sx={{
									display: "block !important",
									opacity: taken.has(option.name) ? 0.5 : 1,
								}}
							>
								<Typography
									component="span"
									sx={{ fontFamily: "monospace" }}
									variant="body2"
								>
									{option.name}
								</Typography>
								<Typography
									component="span"
									variant="caption"
									color="text.secondary"
									sx={{ ml: 1 }}
								>
									{describeType(option)}
									{option.secret ? " · secret" : ""}
									{taken.has(option.name) ? " · set here" : ""}
								</Typography>
								{option.description && (
									<Typography
										variant="caption"
										color="text.secondary"
										sx={{ display: "block" }}
									>
										{option.description}
									</Typography>
								)}
							</Box>
						)}
						renderInput={(params) => (
							<TextField
								{...params}
								inputRef={input}
								placeholder={`add to ${label}`}
								slotProps={{
									...params.slotProps,
									htmlInput: {
										...params.slotProps.htmlInput,
										style: { fontFamily: "monospace" },
										"data-testid": "variable-name",
										"aria-label": `Name of a variable to add to ${label}`,
									},
								}}
							/>
						)}
						sx={{ minWidth: 300 }}
					/>
					{NAME.test(name) && (
						<SchemaField
							// A different variable is a different shape of field.
							key={name}
							schema={schema}
							value={value}
							onChange={setValue}
							secret={secret}
							onSubmit={submit}
						/>
					)}
					{NAME.test(name) && (
						<Tooltip
							title={
								described
									? secret
										? "Secret in the catalogue"
										: "Not secret in the catalogue"
									: secret
										? "Secret"
										: "Not secret"
							}
						>
							<span>
								<IconButton
									size="small"
									aria-label="Secret"
									aria-pressed={secret}
									disabled={Boolean(described)}
									onClick={() => setSecretToggle((s) => !s)}
									data-testid="value-is-secret"
								>
									{secret ? (
										<LockIcon fontSize="small" color="warning" />
									) : (
										<LockOpenIcon fontSize="small" />
									)}
								</IconButton>
							</span>
						</Tooltip>
					)}
					{NAME.test(name) && (
						<Button
							size="small"
							variant="outlined"
							onClick={submit}
							disabled={!complete || set.pending}
						>
							Set
						</Button>
					)}
					{children}
				</Stack>
			</GradedAction>
			{set.error && (
				<Alert severity="warning" sx={{ mt: 1 }}>
					{set.error.message}
				</Alert>
			)}
		</Box>
	);
}
