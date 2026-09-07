// Wire types for the private-server API.
//
// The schemas in this file are *generated* from the rust handler annotations
// (utoipa) → `private-web/openapi.json` → `api-types.ts`. Regenerate with
// `just gen-openapi` after changing any Rust handler's request or response.
//
// UI-only types and constants (display order, label maps, etc.) stay
// hand-written below the re-exports.

import type { components, paths } from "./api-types";

type Schemas = components["schemas"];

// ── Path-based typing for `callApi` ────────────────────────────────────────
//
// `openapi-typescript` emits a `paths` interface keyed by the OpenAPI path
// strings (e.g. `/api/admins/list`). These helpers project that into the
// `(module, fn)` pair the React hooks have always used, and pull the post
// operation's request body and 200 response type so consumers can stop
// hand-writing them.

export type ApiPath = keyof paths & string;

/// Split a path's tail at its **last** slash, so a module may itself be
/// nested: `fleet/applications/get_detail` is the `get_detail` function of the
/// `fleet/applications` module, not an `applications/get_detail` function of a
/// `fleet` module. Inferring on the first slash would say the latter, and
/// collapse every fleet module into one.
type SplitLast<
	S extends string,
	Head extends string = "",
> = S extends `${infer Segment}/${infer Rest}`
	? SplitLast<Rest, Head extends "" ? Segment : `${Head}/${Segment}`>
	: [Head, S];

type ModuleOf<P extends string> = P extends `/api/${infer Tail}`
	? SplitLast<Tail>[0]
	: never;
type FnOf<P extends string> = P extends `/api/${infer Tail}`
	? SplitLast<Tail>[1]
	: never;

export type ApiModule = ModuleOf<ApiPath>;
export type ApiFn<M extends ApiModule> = {
	[P in ApiPath]: ModuleOf<P> extends M ? FnOf<P> : never;
}[ApiPath];

export type ApiPathFor<M extends ApiModule, F extends ApiFn<M>> =
	`/api/${M}/${F}` extends ApiPath ? `/api/${M}/${F}` : never;

type PostOp<P extends ApiPath> = paths[P]["post"];

export type ApiResponse<M extends ApiModule, F extends ApiFn<M>> =
	PostOp<ApiPathFor<M, F>> extends {
		responses: { 200: { content: { "application/json": infer R } } };
	}
		? Solidify<R>
		: void;

export type ApiBody<M extends ApiModule, F extends ApiFn<M>> =
	PostOp<ApiPathFor<M, F>> extends {
		requestBody: { content: { "application/json": infer B } };
	}
		? Solidify<B>
		: Record<string, unknown> | undefined;

// utoipa marks `Option<T>` Rust fields as not-required AND nullable, so
// `openapi-typescript` emits them as `field?: T | null`. But serde's default
// for `Option<T>` is to always emit the field (as `null` for None), so the
// optional `?` is wrong at runtime — every field is present. `Solidify` peels
// that off, making `field: T | null`, which is what the wire shape actually
// gives us.
//
// Tuples need separate handling so `[u64, u64]` doesn't collapse into
// `(u64 | u64)[]`. `number extends T['length']` is true for regular arrays
// and false for fixed-length tuples.
export type Solidify<T> = T extends readonly unknown[]
	? number extends T["length"]
		? Solidify<T[number]>[]
		: { [K in keyof T]: Solidify<Exclude<T[K], undefined>> }
	: T extends object
		? { [K in keyof T]-?: Solidify<Exclude<T[K], undefined>> }
		: T;

// ── Wire types ─────────────────────────────────────────────────────────────

export type ShortStatus = Solidify<Schemas["ShortStatus"]>;
export type HealthState = Solidify<Schemas["HealthState"]>;
export type ApplicationType = Solidify<Schemas["ApplicationType"]>;
export type VersionTracking = Solidify<Schemas["VersionTracking"]>;
export type Caps = Solidify<Schemas["Caps"]>;
export type ApplicationTypeInfo = Solidify<Schemas["ApplicationTypeInfo"]>;
export type MachineDetailData = Solidify<Schemas["MachineDetailData"]>;
export type GroupMachine = Solidify<Schemas["GroupMachine"]>;
export type ServerRank = Solidify<Schemas["ServerRank"]>;
export type VersionStatus = Solidify<Schemas["VersionStatus"]>;
export type DeviceRole = Solidify<Schemas["DeviceRole"]>;
export type ProvisionedCredential = Solidify<Schemas["ProvisionedCredential"]>;
export type MaintenanceWindow = Solidify<Schemas["MaintenanceWindow"]>;
export type ResolvedReason = Solidify<Schemas["ResolvedReason"]>;

export type VersionStr = Solidify<Schemas["VersionStr"]>;

export type GeoPoint = Solidify<Schemas["GeoPoint"]>;
export type OperatorPresence = Solidify<Schemas["OperatorPresence"]>;
export type FacilityServerStatus = Solidify<Schemas["FacilityServerStatus"]>;
export type ServerGroupCard = Solidify<Schemas["ServerGroupCard"]>;
export type ServerGroup = Solidify<Schemas["ServerGroup"]>;
export type GroupDetail = Solidify<Schemas["GroupDetail"]>;
export type InventorySecretVariable = Solidify<
	Schemas["InventorySecretVariable"]
>;
export type SummaryData = Solidify<Schemas["SummaryData"]>;
export type CheckDetailData = Solidify<Schemas["CheckDetailData"]>;
export type NamespaceRef = Solidify<Schemas["NamespaceRef"]>;
export type CheckDetailServerData = Solidify<
	Schemas["CheckDetailServerData"]
>;
export type CheckDetailGroupData = Solidify<Schemas["CheckDetailGroupData"]>;
export type CheckDetailCanopyData = Solidify<
	Schemas["CheckDetailCanopyData"]
>;
export type TagMap = Solidify<Schemas["TagMap"]>;

export type VersionData = Solidify<Schemas["VersionData"]>;
export type MinorVersionGroup = Solidify<Schemas["MinorVersionGroup"]>;
export type RelatedVersionData = Solidify<Schemas["RelatedVersionData"]>;
export type VersionDetail = Solidify<Schemas["VersionDetail"]>;
export type KnownIssueData = Solidify<Schemas["KnownIssueData"]>;
export type ArtifactData = Solidify<Schemas["ArtifactData"]>;

export type ServerInfo = Solidify<Schemas["ServerInfo"]>;
export type ServerLastStatusData = Solidify<Schemas["ServerLastStatusData"]>;
export type ServerDetailData = Solidify<Schemas["ServerDetailData"]>;
export type StatusSnapshotData = Solidify<Schemas["StatusSnapshotData"]>;
export type FleetServerDetailData = Solidify<
	Schemas["FleetServerDetailData"]
>;
export type FleetMachineDetailData = Solidify<
	Schemas["FleetMachineDetailData"]
>;
export type FleetDetailData = Solidify<Schemas["FleetDetailData"]>;
export type ServerSilencedRef = Solidify<Schemas["ServerSilencedRef"]>;
export type ServerGroupSilencedRef = Solidify<Schemas["ServerGroupSilencedRef"]>;

export type EnrollmentTicket = Solidify<Schemas["EnrollmentTicket"]>;
export type EnrollmentStatus = Solidify<Schemas["EnrollmentStatus"]>;

export type DeviceData = Solidify<Schemas["DeviceData"]>;
export type DeviceKeyInfo = Solidify<Schemas["DeviceKeyInfo"]>;
export type DeviceConnectionData = Solidify<Schemas["DeviceConnectionData"]>;
export type DeviceInfo = Solidify<Schemas["DeviceInfo"]>;
export type TailnetLiveInfo = Solidify<Schemas["TailnetLiveInfo"]>;

export type CheckPolicyData = Solidify<Schemas["CheckPolicyData"]>;
export type SourceData = Solidify<Schemas["SourceData"]>;
export type ConsolidatedCheck = Solidify<Schemas["ConsolidatedCheck"]>;
export type ConsolidatedChecks = Solidify<Schemas["ConsolidatedChecks"]>;
export type ReachabilityMode = Solidify<Schemas["ReachabilityMode"]>;
export type IngestMode = Solidify<Schemas["IngestMode"]>;
export type StabilityData = Solidify<Schemas["StabilityData"]>;
export type HealthcheckSample = Solidify<Schemas["HealthcheckSample"]>;
export type HealthcheckSampleResponse = Solidify<Schemas["HealthcheckSampleResponse"]>;

export type IssueData = Solidify<Schemas["IssueData"]>;
export type IssueIncidentLink = Solidify<Schemas["IssueIncidentLink"]>;
export type IncidentData = Solidify<Schemas["IncidentData"]>;

/// An open incident whose last effective failure has recovered: it stays
/// open for the group's linger window in case the failure comes back, and
/// closes (backdated) if things stay quiet. Distinct from "held", which is
/// about the Slack open notice still sitting inside the notification delay.
export function isIncidentLingering(
	incident: Pick<IncidentData, "closed_at" | "lingering_since">,
): boolean {
	return incident.closed_at == null && incident.lingering_since != null;
}
export type IncidentIssueData = Solidify<Schemas["IncidentIssueData"]>;
export type IncidentWithIssues = Solidify<Schemas["IncidentWithIssues"]>;
export type IssueNoteData = Solidify<Schemas["IssueNoteData"]>;
export type IncidentNoteData = Solidify<Schemas["IncidentNoteData"]>;

export type BestoolSnippetInfo = Solidify<Schemas["BestoolSnippetInfo"]>;
export type BestoolSnippetDetail = Solidify<Schemas["BestoolSnippetDetail"]>;

export type SqlResult = Solidify<Schemas["SqlResult"]>;
export type SqlHistoryEntry = Solidify<Schemas["SqlHistoryEntry"]>;

export type BackupConfigView = Solidify<Schemas["BackupConfigView"]>;
export type BackupConfigSummary = Solidify<Schemas["BackupConfigSummary"]>;
export type ScheduleView = Solidify<Schemas["ScheduleView"]>;
export type RetentionPolicy = Solidify<Schemas["RetentionPolicy"]>;
export type BackupStatsView = Solidify<Schemas["BackupStatsView"]>;
export type BackupRepoStats = Solidify<Schemas["BackupRepoStats"]>;
export type RecentRun = Solidify<Schemas["RecentRun"]>;
export type RunStatus = Schemas["RunStatus"];
export type LiveProgress = Solidify<Schemas["LiveProgress"]>;
export type RunProgressPoint = Solidify<Schemas["RunProgressPoint"]>;
export type RestoreActivity = Solidify<Schemas["RestoreActivity"]>;
export type RestoreConsumerView = Solidify<Schemas["RestoreConsumerView"]>;
export type RestoreReplicaView = Solidify<Schemas["RestoreReplicaView"]>;
export type IntentDescriptor = Solidify<Schemas["IntentDescriptor"]>;
export type ParamSpec = Solidify<Schemas["BTreeMap"][string]>;
export type ParamType = Solidify<Schemas["ParamType"]>;
export type BackupMaintenanceRun = Solidify<Schemas["BackupMaintenanceRun"]>;
export type PendingRequestRow = Solidify<Schemas["PendingRequestRow"]>;
export type MachineBackupCapabilityView = Solidify<
	Schemas["MachineBackupCapabilityView"]
>;
export type RestoreWindowRow = Solidify<Schemas["RestoreWindowRow"]>;
export type RestoreWindowView = Solidify<Schemas["RestoreWindowView"]>;
export type RedactionGap = Solidify<Schemas["RedactionGap"]>;

/** The masking parameters Canopy resolves itself for a `redact` intent. They
 * are filtered out of the parameter form: an operator sets whether a replica
 * redacts, not where its masking comes from. */
export const REDACTION_PARAMS = [
	"redaction_manifest_url",
	"redaction_version_query",
	"redaction_version_fallback_to_base",
];

/** What each redaction gap means for the replica. Only the first withholds
 * the replica; the second lets it be restored, and the consumer holds the
 * switchover if the manifest really is missing for the version it finds. */
export const REDACTION_GAP_LABELS: Record<string, string> = {
	product_has_no_manifest:
		"product publishes no masking manifest, so nothing is restored for it",
	version_has_no_manifest:
		"no manifest published for the version it reports, so its redaction may not resolve",
};

export type ManagedZoneView = Solidify<Schemas["ManagedZoneView"]>;
export type GroupDomainView = Solidify<Schemas["GroupDomainView"]>;

// `mode`/`status` arrive as plain strings on the wire (the Rust enums use a
// custom Text serializer, so utoipa emits `string`). Narrow them in the UI so
// switch/label maps are exhaustive.
export type BackupRepoMode = "from_birth" | "passphrase";
export type BackupConfigStatus = "provisioning" | "ready";

// ── Pagination wrapper ─────────────────────────────────────────────────────
//
// utoipa emits one schema per concrete `Page<T>` instantiation
// (`Page_DeviceInfo`, `Page_ServerInfo`, …) — there's no parametric `Page<T>`
// in the generated file. We keep a hand-written generic here so call sites
// (mostly state shapes outside `useApi`) can still write `Page<DeviceInfo>`;
// it's structurally identical to each emitted variant.
export interface Page<T> {
	items: T[];
	total: number;
}

// ── UI-only types ──────────────────────────────────────────────────────────

export type ServerGroupedIds = Partial<Record<ServerRank, string[]>>;

// ── UI-only display order / labels ─────────────────────────────────────────

export const SERVER_RANK_ORDER: ServerRank[] = [
	"production",
	"clone",
	"demo",
	"test",
	"dev",
];

/// Sort key for a rank, with `null` ranks pushed last. Ranks are an ordered
/// set; types are not, so a type tiebreak sorts alphabetically at the
/// comparison rather than through a table here.
export function serverSortKey(s: { rank?: ServerRank | null }): number {
	const rankIndex =
		s.rank == null ? Infinity : SERVER_RANK_ORDER.indexOf(s.rank);
	return rankIndex === -1 ? SERVER_RANK_ORDER.length : rankIndex;
}

/// How a type reads when nothing has named the application: the sentence case
/// of the type, so `tamanu-central` reads as "Tamanu central".
///
/// The backend derives the catalogue's label by the same rule, so the two
/// agree and a name is never blank while the catalogue is still loading.
/// spec: FLT#naming
export function applicationTypeLabel(type: ApplicationType): string {
	const words = type.replace(/-/g, " ");
	return words.charAt(0).toUpperCase() + words.slice(1);
}

/// What an application is called. An application's name is optional and an
/// operator's alone to set; one with no name presents as its type.
/// spec: FLT#naming
export function applicationName(application: {
	name?: string | null;
	type: ApplicationType;
}): string {
	return application.name || applicationTypeLabel(application.type);
}

/// Rank first, then type alphabetically, then name.
///
/// Application types are a flat, open set: a group can report a type
/// Canopy has never seen, so there is no precedence to consult and an invented
/// one would be surprising to read. Alphabetical is the rule everywhere types
/// are listed.
/// spec: APP#where-a-type-comes-from
export function compareServersByRankThenType<
	T extends {
		rank?: ServerRank | null;
		type: ApplicationType;
		name?: string | null;
	},
>(a: T, b: T): number {
	const ar = serverSortKey(a);
	const br = serverSortKey(b);
	if (ar !== br) return ar - br;
	const byType = a.type.localeCompare(b.type);
	if (byType !== 0) return byType;
	return (a.name ?? "").localeCompare(b.name ?? "");
}

/// Group a flat application list into rank buckets in display order, with
/// each bucket internally sorted by type (centrals first) then name.
/// Applications without a rank land in a trailing `null` bucket.
export function groupServersByRank<
	T extends {
		rank?: ServerRank | null;
		type: ApplicationType;
		name?: string | null;
	},
>(servers: readonly T[]): Array<[ServerRank | null, T[]]> {
	const buckets = new Map<ServerRank | null, T[]>();
	for (const s of servers) {
		const rank = s.rank ?? null;
		const list = buckets.get(rank);
		if (list) list.push(s);
		else buckets.set(rank, [s]);
	}
	const order: Array<ServerRank | null> = [...SERVER_RANK_ORDER, null];
	const result: Array<[ServerRank | null, T[]]> = [];
	for (const rank of order) {
		const list = buckets.get(rank);
		if (list && list.length > 0) {
			list.sort(compareServersByRankThenType);
			result.push([rank, list]);
		}
	}
	return result;
}

/// A machine carrying the applications on it, ranked so it can go through the
/// same bucketing as a workload.
export interface RankedMachine {
	machine: GroupMachine;
	applications: ServerInfo[];
	rank: ServerRank | null;
	type: ApplicationType;
	name: string;
}

/// Give each machine the rank of its highest-ranked workload, so a box sorts
/// into the same bands the fleet uses everywhere else.
///
/// Rank is a workload's property, so a box shared by a production and a test
/// workload is a production box. A machine carrying nothing yet has no rank to
/// take and sorts last: awaiting check-in, not an error.
/// spec: FLT
export function rankMachines(
	machines: readonly GroupMachine[],
	applications: readonly ServerInfo[],
): RankedMachine[] {
	const byMachine = new Map<string, ServerInfo[]>();
	for (const application of applications) {
		const list = byMachine.get(application.machine_id);
		if (list) list.push(application);
		else byMachine.set(application.machine_id, [application]);
	}
	return machines.map((machine) => {
		const on = [...(byMachine.get(machine.id) ?? [])].sort(
			compareServersByRankThenType,
		);
		const [best] = on;
		return {
			machine,
			applications: on,
			rank: best?.rank ?? null,
			// A box carrying nothing has no type to take. It has no rank
			// either, so it sorts last on rank alone and this never decides an
			// ordering — naming a type here would be inventing one.
			type: best?.type ?? "",
			name: machine.name ?? "",
		};
	});
}

/// Per-check result vocabulary. Hand-written mirror of the Rust
/// `commons_types::status::CheckResult` (the source of truth) — the
/// private API ships `health[]` as raw JSON, so this never appears in
/// the generated schema. Also the UI display order: most to least
/// urgent, with skipped checks sorted last (a skipped check ran no
/// assertion, so it's the least interesting to surface).
export type CheckResult =
	| "failed"
	| "warning"
	| "broken"
	| "passed"
	| "skipped";

export const CHECK_RESULT_ORDER: CheckResult[] = [
	"failed",
	"warning",
	"broken",
	"passed",
	"skipped",
];

/// Short one-line description of what each effective result means for
/// the incident workflow. Used as the CheckResultChip tooltip and in
/// rule-outcome dropdowns so operators see the semantic meaning at the
/// point of choice.
export const CHECK_RESULT_INTENT: Record<CheckResult, string> = {
	failed: "Failing — opens (or holds open) an incident",
	warning: "Degraded — joins an open incident; doesn't open one",
	broken: "The check itself couldn't run; counts as a warning",
	passed: "Healthy — raises nothing",
	skipped: "Didn't run — raises nothing",
};

/// A policy ceiling: the maximum effective result for a check. `broken`
/// is not a valid ceiling — it describes the check runner, not a grade.
export type Ceiling = Exclude<CheckResult, "broken">;

/// Ceiling vocabulary for the policy editor, loud → quiet.
export const CEILINGS: Ceiling[] = ["failed", "warning", "passed", "skipped"];

/// Short one-line description of what each ceiling does to a check's
/// observed results.
export const CEILING_INTENT: Record<Ceiling, string> = {
	failed: "Failures count in full and can open incidents",
	warning: "Failures grade down to warnings; never opens incidents",
	passed: "Recorded but never alerts",
	skipped: "Never alerts, and the reporting agent may stop running the check",
};

/// Normalise a raw `health[]` entry to its result. Mirror of the Rust
/// `CheckResult::from_entry`: prefer a valid `result` string (an
/// unknown string is null, NOT reinterpreted via `healthy`), else the
/// legacy `healthy: bool` (true → passed, false → failed), else null
/// (malformed entry, callers skip it).
export function checkResultOf(
	entry: Record<string, unknown>,
): CheckResult | null {
	const result = entry.result;
	if (typeof result === "string") {
		return (CHECK_RESULT_ORDER as string[]).includes(result)
			? (result as CheckResult)
			: null;
	}
	const healthy = entry.healthy;
	if (typeof healthy === "boolean") return healthy ? "passed" : "failed";
	return null;
}

/// A namespace as one URL segment: `-` for a curated source's unqualified
/// entry, `machine` for the box's, `application.<type>` for one application
/// type's.
///
/// The subject leads even though the type alone would usually be enough,
/// because the type set is open: a group can report an application type
/// called `machine`, and `application.machine` is still unmistakably that
/// type rather than the box.
export function namespaceSegment(namespace: NamespaceRef | undefined): string {
	const subject = namespace?.subject ?? null;
	if (subject === null) return "-";
	if (subject === "application") {
		return `application.${encodeURIComponent(namespace?.application_type ?? "")}`;
	}
	return encodeURIComponent(subject);
}

/// The namespace a URL segment names, or `null` if it names none of the three
/// shapes an entry can have. A bad segment is a broken link to report, not one
/// to guess at.
export function namespaceFromSegment(segment: string): NamespaceRef | null {
	if (segment === "-") return { subject: null, application_type: null };
	if (segment === "machine") return { subject: "machine", application_type: null };
	const type = segment.startsWith("application.") ? segment.slice(12) : null;
	if (type) {
		return { subject: "application", application_type: decodeURIComponent(type) };
	}
	return null;
}

/// Whether two namespace refs name the same catalog entry. An absent field
/// and an explicit `null` are the same absence, so they compare equal.
export function sameNamespace(a: NamespaceRef | undefined, b: NamespaceRef | undefined): boolean {
	return (
		(a?.subject ?? null) === (b?.subject ?? null) &&
		(a?.application_type ?? null) === (b?.application_type ?? null)
	);
}

/// How a check reads to an operator: `<type>.<check>` where it is one
/// application type's, the bare name otherwise. The qualification is
/// presentation — the name is stored on its own.
export function qualifiedCheckName(
	namespace: NamespaceRef | undefined,
	check: string,
): string {
	return namespace?.subject === "application" && namespace.application_type
		? `${namespace.application_type}.${check}`
		: check;
}

/// Route to the per-healthcheck "who's affected" page for `check`. Check
/// names are arbitrary strings reported by devices (not restricted to
/// URL-safe characters), so every link builder must go through this
/// instead of interpolating the name directly. A check's identity is the
/// (source, namespace, check) triple — a same-named check from another
/// source, or from another application type, is a different check.
export function healthcheckPath(
	source: string,
	namespace: NamespaceRef | undefined,
	check: string,
): string {
	return `/healthchecks/${encodeURIComponent(source)}/${namespaceSegment(namespace)}/${encodeURIComponent(check)}`;
}

/// Route to the policy editor for `check`. Like [`healthcheckPath`], a
/// check's identity is the (source, namespace, check) triple — the editor is
/// scoped to one such entry, so every link builder must carry all three and go
/// through this instead of interpolating the (arbitrary, not necessarily
/// URL-safe) name directly.
export function healthcheckSettingsPath(
	source: string,
	namespace: NamespaceRef | undefined,
	check: string,
): string {
	return `/settings/healthchecks/${encodeURIComponent(source)}/${namespaceSegment(namespace)}/${encodeURIComponent(check)}`;
}

/// Route to the Sources page — the per-source reachability/ingest policy
/// editor, reached from the healthcheck catalog. A single static page, so
/// no interpolation is needed.
export const HEALTHCHECK_SOURCES_PATH = "/settings/healthchecks/sources";

/// The check name embedded in a health issue's `ref` (`health/<check>`,
/// filed under whichever source reports the check), or `null` for
/// issues whose ref isn't a healthcheck (backups, manual, canopy
/// reachability, …).
export function healthcheckNameFromRef(
	_source: string,
	ref: string,
): string | null {
	const prefix = "health/";
	if (!ref.startsWith(prefix)) return null;
	return ref.slice(prefix.length);
}

/// Sources canopy reserves for its own conditions. They file at bare
/// refs; every other source's checks are namespaced under `health/`.
export const RESERVED_SOURCES = ["canopy", "manual"];

/// The silence ref for a check, which is what the silence endpoints and
/// the silence listings speak. Mirrors `database::silenced_refs`: bare
/// for the reserved sources, `health/`-prefixed for reported checks.
/// Building it by hand gets the reserved sources wrong, and a mismatched
/// ref silently fails to match an existing silence.
export function silenceRef(source: string, check: string): string {
	return RESERVED_SOURCES.includes(source) ? check : `health/${check}`;
}

/// A silence ref as it reads to an operator: the `health/` prefix, where the
/// source has one, then the qualified check name. Two application types
/// silenced for one check name are two rows, and only the qualifier tells
/// them apart.
export function qualifiedSilenceRef(
	namespace: NamespaceRef | undefined,
	ref: string,
): string {
	const slash = ref.indexOf("/");
	const prefix = slash === -1 ? "" : ref.slice(0, slash + 1);
	const check = slash === -1 ? ref : ref.slice(slash + 1);
	return `${prefix}${qualifiedCheckName(namespace, check)}`;
}

/// Canopy's per-server reachability check, with the ref its silence is
/// keyed by. The server form's "alert when unreachable" switch and the
/// check's own silence button both write this one silence.
export const REACHABILITY_CHECK = {
	source: "canopy",
	check: "reachability",
	ref: silenceRef("canopy", "reachability"),
} as const;

/// One person connected somewhere in a server group, with the boxes they're
/// logged in to. Produced by [`aggregateOperators`].
export type AggregatedOperator = {
	op: OperatorPresence;
	machines: string[];
};

/// Aggregate per-member operator presence into distinct people across the
/// group: deduped by login, keeping the earliest `connected_since` and
/// collecting which boxes each person is logged in to. Shared by the
/// status-page group cards and the group detail page so both show the same
/// numbers.
///
/// A person is logged in to a box, not to the software on it, so a box
/// running two applications is one place a person is rather than two.
// spec: FLT
export function aggregateOperators(
	members: FacilityServerStatus[],
): AggregatedOperator[] {
	const byLogin = new Map<string, AggregatedOperator>();
	const seenOn = new Map<string, Set<string>>();
	for (const m of members) {
		// An unnamed box reads as the application leading it, the same
		// fallback the dot strip's enclosures use.
		const machineName = m.machine_name ?? m.name;
		for (const op of m.operators) {
			const existing = byLogin.get(op.login);
			if (!existing) {
				byLogin.set(op.login, { op, machines: [machineName] });
				seenOn.set(op.login, new Set([m.machine_id]));
				continue;
			}
			const on = seenOn.get(op.login)!;
			if (!on.has(m.machine_id)) {
				on.add(m.machine_id);
				existing.machines.push(machineName);
			}
			if (
				op.connected_since &&
				(!existing.op.connected_since ||
					Date.parse(op.connected_since) <
						Date.parse(existing.op.connected_since))
			) {
				existing.op = { ...existing.op, connected_since: op.connected_since };
			}
		}
	}
	return [...byLogin.values()];
}

export const RESOLVED_REASONS: ResolvedReason[] = [
	"fixed",
	"wont_fix",
	"expected",
	"duplicate",
	"flapping",
];

export const RESOLVED_REASON_LABEL: Record<ResolvedReason, string> = {
	fixed: "Fixed",
	wont_fix: "Won't fix",
	expected: "Expected",
	duplicate: "Duplicate",
	flapping: "Flapping",
	// Applied as a side effect of decommissioning a check, not operator-
	// selectable — but shown when an affected issue is displayed.
	decommissioned: "Decommissioned",
};

// Shown wherever a resolver is named but no operator login is attached: the
// incident/issue retired because its healthcheck started reporting healthy
// again. Phrased as the triggering event, not an actor — it describes what
// canopy observed, and does not imply that nobody intervened. Keep in sync
// with the Slack `incident_resolve` default in
// crates/database/src/slack_outbox/vars.rs.
export const AUTOMATION_RESOLVER_LABEL = "the healthcheck recovering";

// ── Backup lifecycle UI constants ───────────────────────────────────────────

/// Display label per backup-repo lifecycle status.
export const BACKUP_STATUS_LABEL: Record<BackupConfigStatus, string> = {
	provisioning: "Provisioning",
	ready: "Ready",
};

/// MUI Chip colour per status (loud → calm).
export const BACKUP_STATUS_INTENT: Record<
	BackupConfigStatus,
	"info" | "warning" | "success"
> = {
	provisioning: "info",
	ready: "success",
};

/// One-line explanation of what each status means for the operator.
export const BACKUP_STATUS_HELP: Record<BackupConfigStatus, string> = {
	provisioning: "Repository is being created; backups are dormant until ready.",
	ready: "Backups are active for this group.",
};

export const BACKUP_MODE_LABEL: Record<BackupRepoMode, string> = {
	from_birth: "From birth (Canopy generates the passphrase)",
	passphrase: "Existing repository (connect with its passphrase)",
};

/// Org-minimum retention floors, enforced server-side and mirrored client-side
/// (helper text + disabled submit). Keep in sync with
/// `database::backups::RetentionPolicy` FLOOR_* constants.
export const RETENTION_FLOORS = {
	keep_daily: 7,
	keep_weekly: 4,
	keep_monthly: 6,
} as const;
