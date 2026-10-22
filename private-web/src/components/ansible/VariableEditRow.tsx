import CloseIcon from "@mui/icons-material/Close";
import LockIcon from "@mui/icons-material/Lock";
import LockOpenIcon from "@mui/icons-material/LockOpen";
import { Alert, Box, Button, IconButton, Stack, Tooltip } from "@mui/material";
import { useState } from "react";
import { useApiAction } from "../../api";
import type { CatalogueView } from "../../types";
import { GradedAction } from "../GradedAction";
import SchemaField from "./SchemaField";
import { type CatalogueVariable, type Schema, problems } from "./schema";

/// A variable's value opened for editing in its row, in the form adding it
/// takes, holding the value it has. A secret's field opens empty, since its
/// value is never shown. Enter saves; Escape leaves it as it was.
// spec: ANSVAR#presentation
export default function VariableEditRow({
	name,
	value: current,
	secret: wasSecret,
	scope,
	catalogue,
	onClose,
	onSaved,
}: {
	name: string;
	/** The value it has, unknown for a secret. */
	value: unknown;
	secret: boolean;
	/** The scope arguments of the call. */
	scope: Record<string, unknown>;
	catalogue: CatalogueView | null;
	onClose: () => void;
	onSaved: () => void;
}) {
	const [value, setValue] = useState<unknown>(wasSecret ? undefined : current);
	const [secretToggle, setSecretToggle] = useState(wasSecret);
	const set = useApiAction("ansible_variables", "set");

	const described: CatalogueVariable | undefined = catalogue?.variables.find(
		(variable) => variable.name === name,
	);
	const secret = described ? described.secret : secretToggle;
	const schema: Schema = (described?.schema as Schema | undefined) ?? {};
	const complete = value !== undefined && problems(schema, value).length === 0;

	const save = () => {
		if (!complete || set.pending) return;
		set
			.call({ ...scope, name, value, secret })
			.then(() => {
				onSaved();
				onClose();
			})
			.catch(() => {
				// Surfaced by the alert below.
			});
	};

	return (
		<Box
			data-testid="edit-variable"
			onKeyDown={(event) => {
				if (event.key === "Escape") {
					event.stopPropagation();
					onClose();
				}
			}}
		>
			<GradedAction calls="ansible_variables/set" action={`Set ${name}`}>
				<Stack
					direction="row"
					spacing={1}
					useFlexGap
					sx={{ flexWrap: "wrap", alignItems: "flex-start" }}
				>
					<SchemaField
						schema={schema}
						value={value}
						onChange={setValue}
						secret={secret}
						autoFocus
						onSubmit={save}
					/>
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
							>
								{secret ? (
									<LockIcon fontSize="small" color="warning" />
								) : (
									<LockOpenIcon fontSize="small" />
								)}
							</IconButton>
						</span>
					</Tooltip>
					<Button
						size="small"
						variant="outlined"
						onClick={save}
						disabled={!complete || set.pending}
					>
						Save
					</Button>
					<IconButton
						size="small"
						aria-label={`Stop editing ${name}`}
						onClick={onClose}
					>
						<CloseIcon fontSize="small" />
					</IconButton>
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
