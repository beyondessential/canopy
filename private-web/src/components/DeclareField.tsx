import { Alert, Box, Button, Stack, TextField } from "@mui/material";
import { useState } from "react";
import { useApiAction } from "../api";
import { type DnsNameKind, KIND_MODULES, KIND_NOUNS } from "../dnsNames";
import { GradedAction } from "./GradedAction";

/// Declares a DNS name for one kind on an application. The same field for both
/// kinds, calling the module of the kind it is given.
// spec: DNS#declared-dns-names
export default function DeclareField({
	kind,
	serverId,
	onChanged,
}: {
	kind: DnsNameKind;
	serverId: string;
	onChanged: () => void;
}) {
	const module = KIND_MODULES[kind];
	const [name, setName] = useState("");
	const declare = useApiAction(module, "declare");

	const onDeclare = async () => {
		try {
			await declare.call({ application_id: serverId, name: name.trim() });
			setName("");
			onChanged();
		} catch {
			/* surfaced via declare.error */
		}
	};

	return (
		<Box
			component="form"
			onSubmit={(e) => {
				e.preventDefault();
				onDeclare();
			}}
		>
			<Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
				<TextField
					size="small"
					placeholder="app.example.tamanu.app"
					value={name}
					onChange={(e) => setName(e.target.value)}
					disabled={declare.pending}
					slotProps={{
						htmlInput: {
							"aria-label": `DNS name to declare for ${KIND_NOUNS[kind]}`,
						},
					}}
					sx={{ minWidth: 280, "& input": { fontFamily: "monospace" } }}
				/>
				<GradedAction
					calls={`${module}/declare`}
					action={`Declare DNS name ${name.trim()} for ${KIND_NOUNS[kind]}`}
				>
					<Button
						type="submit"
						variant="outlined"
						size="small"
						disabled={declare.pending || name.trim() === ""}
					>
						Declare
					</Button>
				</GradedAction>
			</Stack>
			{declare.error && (
				<Alert severity="error" sx={{ mt: 1 }}>
					{declare.error.message}
				</Alert>
			)}
		</Box>
	);
}
