import AddIcon from "@mui/icons-material/Add";
import CloseIcon from "@mui/icons-material/Close";
import VisibilityIcon from "@mui/icons-material/Visibility";
import VisibilityOffIcon from "@mui/icons-material/VisibilityOff";
import {
	Box,
	Button,
	Chip,
	IconButton,
	InputAdornment,
	MenuItem,
	Stack,
	TextField,
} from "@mui/material";
import { useRef, useState } from "react";
import { type Schema, kindOf, problems, requires } from "./schema";

type Props = {
	schema: Schema;
	value: unknown;
	onChange: (value: unknown) => void;
	/** The field's label, where it has one: a property's name. */
	label?: string;
	required?: boolean;
	/** Mask what is typed, as for a secret. */
	secret?: boolean;
	autoFocus?: boolean;
	/** Enter in a field sets the variable. */
	onSubmit?: () => void;
	/** Where in the value this is, for naming a fault. */
	path?: string;
};

/// A field for a value shaped by its schema: a typed field for a scalar, a
/// field for each property an object names, one item at a time for a list, and
/// JSON for anything it cannot shape.
// spec: ANSVAR#presentation
export default function SchemaField(props: Props) {
	const { schema } = props;
	switch (kindOf(schema)) {
		case "boolean":
			return <BooleanField {...props} />;
		case "integer":
		case "number":
			return <NumberField {...props} />;
		case "string":
			return <StringField {...props} />;
		case "object":
			return schema.properties ? (
				<ObjectField {...props} />
			) : (
				<JsonField {...props} />
			);
		case "array":
			return <ListField {...props} />;
		default:
			return <JsonField {...props} />;
	}
}

const enter =
	(onSubmit?: () => void) => (event: React.KeyboardEvent<HTMLElement>) => {
		if (event.key === "Enter" && !event.shiftKey && onSubmit) {
			event.preventDefault();
			onSubmit();
		}
	};

function faults(schema: Schema, value: unknown): string[] {
	return problems(schema, value);
}

function BooleanField({
	schema,
	value,
	onChange,
	label,
	required,
	autoFocus,
}: Props) {
	return (
		<TextField
			select
			size="small"
			label={label ?? "Value"}
			required={required}
			value={typeof value === "boolean" ? String(value) : ""}
			onChange={(event) => onChange(event.target.value === "true")}
			autoFocus={autoFocus}
			helperText={schema.description}
			sx={{ minWidth: 140 }}
		>
			<MenuItem value="true">true</MenuItem>
			<MenuItem value="false">false</MenuItem>
		</TextField>
	);
}

function NumberField({
	schema,
	value,
	onChange,
	label,
	required,
	autoFocus,
	onSubmit,
}: Props) {
	const [text, setText] = useState(value === undefined ? "" : String(value));
	const found = faults(schema, value);
	return (
		<TextField
			size="small"
			type="number"
			label={label ?? "Value"}
			required={required}
			value={text}
			autoFocus={autoFocus}
			error={found.length > 0}
			helperText={found[0] ?? schema.description}
			onChange={(event) => {
				setText(event.target.value);
				onChange(event.target.value === "" ? undefined : Number(event.target.value));
			}}
			onKeyDown={enter(onSubmit)}
			slotProps={{
				htmlInput: { min: schema.minimum, max: schema.maximum },
			}}
		/>
	);
}

function StringField({
	schema,
	value,
	onChange,
	label,
	required,
	secret,
	autoFocus,
	onSubmit,
}: Props) {
	const [shown, setShown] = useState(false);
	const found = faults(schema, value);
	if (schema.enum) {
		return (
			<TextField
				select
				size="small"
				label={label ?? "Value"}
				required={required}
				value={typeof value === "string" ? value : ""}
				onChange={(event) => onChange(event.target.value)}
				autoFocus={autoFocus}
				helperText={schema.description}
				sx={{ minWidth: 180 }}
			>
				{schema.enum.map((option) => (
					<MenuItem key={String(option)} value={String(option)}>
						{String(option)}
					</MenuItem>
				))}
			</TextField>
		);
	}
	return (
		<TextField
			size="small"
			label={label ?? "Value"}
			required={required}
			value={typeof value === "string" ? value : ""}
			type={secret && !shown ? "password" : "text"}
			autoFocus={autoFocus}
			error={found.length > 0}
			helperText={found[0] ?? schema.description}
			onChange={(event) =>
				onChange(event.target.value === "" ? undefined : event.target.value)
			}
			onKeyDown={enter(onSubmit)}
			slotProps={{
				htmlInput: { style: { fontFamily: "monospace" } },
				input: secret
					? {
							endAdornment: (
								<InputAdornment position="end">
									<IconButton
										size="small"
										aria-label={shown ? "Hide" : "Show"}
										onClick={() => setShown((s) => !s)}
									>
										{shown ? (
											<VisibilityOffIcon fontSize="small" />
										) : (
											<VisibilityIcon fontSize="small" />
										)}
									</IconButton>
								</InputAdornment>
							),
						}
					: undefined,
			}}
			sx={{ minWidth: 220 }}
		/>
	);
}

function ObjectField({
	schema,
	value,
	onChange,
	secret,
	autoFocus,
	onSubmit,
	path = "",
}: Props) {
	const record =
		typeof value === "object" && value !== null && !Array.isArray(value)
			? (value as Record<string, unknown>)
			: {};
	const names = Object.keys(schema.properties ?? {});
	return (
		<Stack
			direction="row"
			spacing={1}
			useFlexGap
			sx={{ flexWrap: "wrap", alignItems: "flex-start" }}
			data-testid="object-field"
		>
			{names.map((name, index) => (
				<SchemaField
					key={name}
					schema={(schema.properties as Record<string, Schema>)[name]}
					label={name}
					required={requires(schema, name)}
					value={record[name]}
					secret={secret}
					autoFocus={autoFocus && index === 0}
					path={path === "" ? name : `${path}.${name}`}
					onSubmit={index === names.length - 1 ? onSubmit : undefined}
					onChange={(next) => {
						const updated = { ...record };
						if (next === undefined) delete updated[name];
						else updated[name] = next;
						onChange(Object.keys(updated).length === 0 ? undefined : updated);
					}}
				/>
			))}
		</Stack>
	);
}

function ListField({
	schema,
	value,
	onChange,
	label,
	secret,
	autoFocus,
	onSubmit,
}: Props) {
	const items = Array.isArray(value) ? value : [];
	const itemSchema: Schema = schema.items ?? {};
	const objects = kindOf(itemSchema) === "object" && itemSchema.properties;
	const [draft, setDraft] = useState("");
	const input = useRef<HTMLInputElement>(null);

	const set = (next: unknown[]) => onChange(next.length === 0 ? undefined : next);

	if (objects) {
		return (
			<Stack spacing={1} data-testid="list-field">
				{items.map((item, index) => (
					<Stack
						// biome-ignore lint/suspicious/noArrayIndexKey: items have no identity of their own.
						key={index}
						direction="row"
						spacing={1}
						sx={{ alignItems: "flex-start" }}
					>
						<SchemaField
							schema={itemSchema}
							value={item}
							secret={secret}
							onChange={(next) =>
								set(items.map((old, at) => (at === index ? next : old)))
							}
							onSubmit={onSubmit}
						/>
						<IconButton
							size="small"
							aria-label="Remove item"
							onClick={() => set(items.filter((_, at) => at !== index))}
						>
							<CloseIcon fontSize="small" />
						</IconButton>
					</Stack>
				))}
				<Box>
					<Button
						size="small"
						startIcon={<AddIcon />}
						autoFocus={autoFocus && items.length === 0}
						onClick={() => set([...items, {}])}
					>
						Add
					</Button>
				</Box>
			</Stack>
		);
	}

	const add = () => {
		const text = draft.trim();
		if (text === "") return;
		const kind = kindOf(itemSchema);
		const parsed =
			kind === "integer" || kind === "number" ? Number(text) : text;
		if (faults(itemSchema, parsed).length > 0) return;
		set([...items, parsed]);
		setDraft("");
		input.current?.focus();
	};

	return (
		<Stack
			direction="row"
			spacing={0.5}
			useFlexGap
			sx={{ flexWrap: "wrap", alignItems: "center" }}
			data-testid="list-field"
		>
			{items.map((item, index) => (
				<Chip
					// biome-ignore lint/suspicious/noArrayIndexKey: items have no identity of their own.
					key={index}
					size="small"
					label={String(item)}
					onDelete={() => set(items.filter((_, at) => at !== index))}
					sx={{ fontFamily: "monospace" }}
				/>
			))}
			<TextField
				size="small"
				label={label ?? "Item"}
				value={draft}
				autoFocus={autoFocus}
				inputRef={input}
				error={draft !== "" && faults(itemSchema, draft).length > 0 && kindOf(itemSchema) === "string"}
				onChange={(event) => setDraft(event.target.value)}
				onKeyDown={(event) => {
					if (event.key === "Enter") {
						event.preventDefault();
						if (draft.trim() !== "") add();
						else onSubmit?.();
					}
				}}
				onBlur={add}
				helperText={schema.description}
				sx={{ minWidth: 200 }}
			/>
		</Stack>
	);
}

function JsonField({ schema, value, onChange, label, autoFocus, onSubmit }: Props) {
	const [text, setText] = useState(
		value === undefined ? "" : JSON.stringify(value),
	);
	const [invalid, setInvalid] = useState(false);
	return (
		<TextField
			size="small"
			label={label ?? "Value"}
			value={text}
			autoFocus={autoFocus}
			error={invalid}
			helperText={invalid ? "Not JSON" : (schema.description ?? "JSON")}
			onChange={(event) => {
				const next = event.target.value;
				setText(next);
				if (next.trim() === "") {
					setInvalid(false);
					onChange(undefined);
					return;
				}
				try {
					onChange(JSON.parse(next));
					setInvalid(false);
				} catch {
					// A bare word is a string, as it is wherever a value is typed.
					setInvalid(false);
					onChange(next);
				}
			}}
			onKeyDown={enter(onSubmit)}
			slotProps={{ htmlInput: { style: { fontFamily: "monospace" } } }}
			sx={{ minWidth: 260 }}
		/>
	);
}
