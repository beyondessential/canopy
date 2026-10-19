import type { AuditActor, AuditRecord, DeviceRole } from "../types";

const DEVICE_ROLE_LABEL: Record<DeviceRole, string> = {
	machine: "Machine device",
	relay: "Relay device",
	"backup-restore": "Backup-restore device",
	admin: "Admin device",
	releaser: "Releaser device",
};

export function deviceRoleLabel(role: string): string {
	return DEVICE_ROLE_LABEL[role as DeviceRole] ?? `${role} device`;
}

/** What a job is called to an operator: "dns_records" becomes "Dns records". */
export function jobLabel(job: string): string {
	const text = job.replace(/[_-]+/g, " ").trim();
	return text.charAt(0).toUpperCase() + text.slice(1);
}

/** The value the actor filter holds for an actor, and what it sends back. */
export function actorKey(actor: AuditActor): string {
	return `${actor.kind}:${actorId(actor)}`;
}

export function actorId(actor: AuditActor): string {
	switch (actor.kind) {
		case "operator":
			return actor.login;
		case "canopy":
			return actor.job;
		default:
			return actor.id;
	}
}

/** The actor's name as the log shows it, and a caption saying what sort it is. */
export function actorLabel(actor: AuditActor): {
	name: string;
	caption: string | null;
} {
	switch (actor.kind) {
		case "operator":
			return { name: actor.login, caption: null };
		case "device":
			return {
				name: actor.name ?? actor.id,
				caption: deviceRoleLabel(actor.role),
			};
		case "token":
			return { name: actor.name, caption: "Access token" };
		case "canopy":
			return { name: "Canopy", caption: jobLabel(actor.job) };
	}
}

/** The in-app page for a record, by its identifier. Kinds with no page of
 * their own give `null`, and so are shown as text. */
export function recordPath(record: AuditRecord): string | null {
	const id = encodeURIComponent(record.id);
	switch (record.kind) {
		case "machine":
			return `/fleet/machines/${id}`;
		case "group":
			return `/fleet/groups/${id}`;
		case "application":
			return `/fleet/applications/${id}`;
		case "cluster":
			return `/fleet/clusters/${id}`;
		case "device":
			return `/devices/${id}`;
		case "restore_replica":
			return "/settings/restore-consumers";
		default:
			return null;
	}
}

export type DetailValue = { text: string; mono: boolean };

const ISO_TIMESTAMP = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}/;
const ISO_DATE = /^\d{4}-\d{2}-\d{2}$/;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** One value of an entry's detail as it is shown, or `null` where there is
 * nothing to show. Times are the viewer's locale's. */
export function detailValue(value: unknown): DetailValue | null {
	if (value === null || value === undefined) return null;
	if (typeof value === "string") {
		if (value === "") return null;
		if (ISO_TIMESTAMP.test(value)) {
			const at = new Date(value);
			if (!Number.isNaN(at.getTime())) {
				return {
					text: new Intl.DateTimeFormat(undefined, {
						dateStyle: "medium",
						timeStyle: "short",
					}).format(at),
					mono: false,
				};
			}
		}
		if (ISO_DATE.test(value)) {
			const on = new Date(value);
			if (!Number.isNaN(on.getTime())) {
				return {
					text: new Intl.DateTimeFormat(undefined, {
						dateStyle: "medium",
						timeZone: "UTC",
					}).format(on),
					mono: false,
				};
			}
		}
		return { text: value, mono: UUID.test(value) };
	}
	if (typeof value === "number" || typeof value === "boolean") {
		return { text: String(value), mono: false };
	}
	if (Array.isArray(value) && value.length === 0) return null;
	if (typeof value === "object" && Object.keys(value).length === 0) return null;
	return { text: JSON.stringify(value), mono: true };
}

/** An entry's detail as key and value pairs, leaving out what is empty. */
export function detailPairs(
	detail: Record<string, unknown>,
): Array<{ key: string; value: DetailValue }> {
	const pairs: Array<{ key: string; value: DetailValue }> = [];
	for (const [key, raw] of Object.entries(detail)) {
		const value = detailValue(raw);
		if (value) pairs.push({ key: key.replace(/_/g, " "), value });
	}
	return pairs;
}
