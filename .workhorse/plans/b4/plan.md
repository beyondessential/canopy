# Facility-level targeting for central-reported checks

## Problem

`sync_facility_stale` is reported by central's alertd as one check whose detail holds `fail` and `warn` arrays of facilities.
Policy rules match scalar `check.<field>` values only, so no rule or silence can reach one facility.
A facility known to be offline (its reachability silenced) keeps central's check alerting, which hides every other facility behind it.

## Direction (workshop)

Two layers.
Layer 1 is this card; layer 2 is to be split out into its own card.

The motivating case, facilities known to be offline, is resolved by layer 1 alone: their instances are silenced on central's check.
Layer 2 does not reach that case (see its gates below), which is accepted.

### Layer 1: reported checks carry instances

A health check in a push gains an `instances` field, each instance with its own result, optional label, and a nested `detail` object.
Canopy grades reported instances the way it already grades its own (see CHK, "Checks with instances"; `file_check_instances`).
Additive to the public API, so compatible under the api-compatibility rule.
Payload shapes are in the instanced check payloads mockup (`.workhorse/design/mockups/b4/instanced-check-payloads.html`).

#### Per-check `detail` object (folded into this card)

STA already says a health check carries its own `detail` object, but that was never implemented: V2 nested detail for targets (`parse_target_report`) and left checks flat (`HealthCheck.extra`, `#[serde(flatten)]`, read by stripping `check`/`healthy` at ingestion).
This card implements it, since the instance shape leans on it:

- a check entry accepts a `detail` object; the flat form stays accepted for plain checks, per the compatibility rule;
- a plain check carrying flat fields and `detail` together is a 400 rather than a guessed merge;
- an instanced check takes its fields in `detail` only, and a flat field beside `instances` is a 400, so `instances` is never read as a detail field;
- `detail` that isn't an object, on a check or an instance, is a 400;
- rules read `check.<field>` the same from either form.

The narrow remaining break: a reporter sending a flat detail field literally named `detail` or `instances` has it read as structure. bestool sends neither.
The generated client gains a `detail` field on `HealthCheck`; bestool's `to_health_check` moves to it.

#### Instanced check rules

- A check entry carries exactly one of `result`, `healthy`, `instances`; `result` with `instances` is a 400.
- An instance takes `result` only, never the legacy `healthy`; an empty key is a 400.
- `instances: {}` recovers every instance the check held, and the check is passed.
- The check's own `detail` is shared by every instance; rules read `check.<field>` from the instance's detail first, then the check's.
- A check moving between plain and instanced is still one state.
- The push response stays per check: one query produces every instance, so there is nothing for a reporter to skip per instance.
- Machine checks can be instanced too, the shape being the same `HealthCheck`.

Instance identity follows the application pattern: `instances` is an object keyed by an instance key the reporter chooses, unique within the check and stable across pushes.
A payload therefore cannot express two instances sharing a key.
The check's instances are the source's complete set for it, so an instance omitted from a push has recovered; reporters send passing instances too.

An instance may carry a label beside its result and detail, falling back to its key without one.
Canopy writes an instanced check's message from the graded instances, naming the degraded ones by label, as it does for its own instanced checks.
A reporter's own message would count instances a silence has since taken out.

Canopy's own instanced checks (backup and restore families) move to keys in this card, replacing the convention of joining multi-field identities into one detail field.
One identity mechanism for every instance, reported or determined.

An instance silence is a scoped silence that also names an instance key, recording who and when like any silence, at the scopes the check itself can be silenced at.
It is offered wherever a target's check is presented with its instances: the target's checks, the issue, the incident.
It is never offered on the fleet-wide check page, which would list every instance on every target.
An instance silence is also the fallback wherever layer 2 cannot resolve a reference.

### Layer 2: an instance can name what it concerns

Canopy never shares its own identifiers and must not learn reporters' domain concepts (Tamanu facility, device).
So correlation is opaque:

- an application's report declares aliases it is known by, as an `aliases` object keyed by kind, one value per kind, which Canopy does not interpret (e.g. kind `tamanu-device`);
- an instance names the alias it concerns, in a `concerns` object of the same shape;
- Canopy resolves the alias within the reporting application's group.

The issue stays the reporting target's for health rollup and incident placement.
The reference only changes how the instance is gated, and where it is presented.

A resolved instance inherits the target-wide gates of the target it concerns: its monitoring switch and its maintenance windows.
It does not inherit that target's silences, reachability included: a silence quiets one check, and an instance concerning a target is not that target's check.

A resolved instance is also presented on the target it concerns, marked as reported by the reporting target, the way a machine's checks present on its applications.
It counts towards the reporting target's health, not the referenced one's.

Resolution rules so far:

- aliases persist from an application's last report, since the target most often referenced is the one that has gone quiet;
- an alias claimed by more than one application in the group resolves to nothing, so ambiguity never silences anything (a facility restored from another's backup can share a device id);
- an unresolved reference leaves the instance graded on the reporting target's policy alone.

### Keyed by device, not facility

Tamanu's `sync_sessions.parameters` carries `deviceId` alongside `facilityIds`.
A device is one facility server, which is what a Canopy application models; a facility is a Tamanu domain concept a server can carry several of.
So instances key on `deviceId`, with the facility ids (and names, from central's `facilities` table) as instance detail.

### Other checks that take instances

`fhir_materialisation` already keys its detail by resource (`resources.<Name>`), but rule fields are one dot-free name, so no resource is reachable on its own.
Its four side lists fold into instance results: `disabled` and `upstream_absent` are skipped, `errored` and `unmonitored` are warning (as they grade today), never broken.
That also stops a failing resource from hiding an unmeasurable one, which today's reporter-chosen headline does.
Open: `unmonitored` entries are table names where the other keys are resource names.

### Cross-repo (bestool)

- `sync_facility_stale` emits one instance per active non-mobile device, keyed by `deviceId`, labelled with its facilities' names.
- `fhir_materialisation` emits one instance per resource, keyed by resource name.
- A facility's alertd declares its `deviceId` (from `local_system_facts`) as an alias on its application.

## Rejected

- Filing each facility's staleness on the facility application: state is keyed `(target, source, check)` and both reporters are `alertd`, so the facility's own push would recover central's filing by omission.
- One check name per facility: breaks the no-parameters-in-check-names rule.

## Notes from spec drafting

- The relay's substrate filings already carry instances (`relay_protocol::SubstrateInstance`: label, observed, detail), and K8S already promises per-instance silences. They take keys like every other instance, so the relay protocol gains a key per instance. A substrate check that holds once is one instance today, with an empty label.
- Substrate filings currently pass the relay's own `message` through (`ingest_substrate` hands `file_check_instances` a closure returning it). Under CHK, Canopy writes an instanced check's message from its graded instances, so the relay's message gives way to Canopy's for these.
- Broken is a whole-check result, never an instance's: an instance reporting `broken` is refused, and a broken check (reported with `result: broken`, no instances) presents every instance it held as broken, retains its last definite result through the existing broken rule, and recovers none of its instances. One state per check, nothing held per instance.
- The relay emits broken only as whole-check brokenness wrapped in a single unnamed instance (`Determination::refused`, `watch.rs` `broken`). Those become a check-level broken result in the relay protocol.
- Restore replicas key on type, intent and declared name together (the existing `ReplicaKey`), which replaces the joined type-and-intent detail field.
- The bestool side is card P3 in the bestool workspace.
- Rules apply to instances transparently (CHK, "Checks with instances"): one grading path, with a plain check graded as its own single instance. `file_check_instances` already grades each instance through the catalog entry and scoped chain with `check.result` set to the instance's result. Two gaps to close:
  - Canopy's own filings pass an empty `status_extra`; reported instances must get the push's report detail, as a plain check from the same push does.
  - The rule-authoring sample (`fns/healthchecks.rs`, `sample`) must present one instance's fields merged over the check's shared detail, not the raw `instances` object.

## Notes from the push wire shape (section 4)

- A reported check is read into one shape, `database::issues::ReportedCheck` (name, `CheckOutcome`, detail with flat fields folded in), from the stored `health` array, so the private server's as-of-past re-grade (section 6) can read stored instanced and nested-detail entries the same way; it still reads entries itself today.
- A plain check's stored `detail` no longer carries the entry's `check` / `result` / `healthy` keys, so the flat and nested forms store the same detail.
- An instanced check is headlined `Health check '<check>' is degraded` (or `is broken`) whatever its instances come to, and that title is stamped on every filing, because lifting an instance silence can bring it back into trouble at any grade.
- Unrecognised keys on an instance are ignored rather than refused, as unrecognised keys in the current push format are.
- Retaining a failure through brokenness keys on the effective result being broken, whether reported broken or graded broken by a rule (CHK "Stability"); `GradedCheck::retain_through_brokenness` applies it, and the push path and `file_check_instances` read the prior state only once the check comes out broken. A rule grading one instance of an instanced check as broken grades it as a warning (CHK "Checks with instances").
- The client generator marks every optional collection property `#[builder(default)]`: typify renders an optional object as a bare map, which the builder would otherwise demand, so adding `HealthCheck.detail` would have broken every call site building a check. Four existing fields (`IntentDescriptor.semantics`, `Entitlements.applications`, `ReportingSchemaArgs.artifacts`, `ProgressArgs.extra`) are relaxed by the same rule.

## Notes from Canopy's own instanced checks (section 5)

- A restore replica's key is `type:intent:name`, or `type:intent` for a replica no declaration names, each part percent-encoding `%` and `:` so that open-ended types, intents and names can never collide. The label (`name (type / intent)`) is unchanged; the detail keeps `type`, `intent` and `replica` for rules, and no longer carries the joined `replica_key`. `RESTORE_VERIFICATION_DOC` now points at an instance silence for one replica rather than at `check.replica_key` (catalog documentation seeds on first sight only, so existing rows keep the old text).
- `file_check_instances`' message composer takes the `GradedCheck` rather than the degraded instances, so a filer can tell a plain check from an instanced one; Canopy's own composers read `graded.degraded()`.
- Relay protocol, changed in place: `SubstrateInstance { key, label: Option, observed, detail }`; `SubstrateFiling.instances` became `outcome: SubstrateOutcome { Instances(Vec<SubstrateInstance>), Broken { detail } }`; `SubstrateFiling.message` became optional, carried for a check that holds once or could not be read and left out for a check with instances. The relay's own node-pool message composer is gone.
- Ingest: a plain substrate check (holds once, or broken with no instances held) keeps the relay's message, as Canopy's own plain filings keep their filer's; an instanced one, broken included, takes `GradedCheck::message`. Ingest refuses a broken instance, a duplicated key, and an empty key beside others, rather than tripping `grade_instances`' debug assertions on a relay's input.

## Notes from the private API (section 6)

- A consolidated instance carries `key`, `label`, `observed`, `effective`, its own `detail` (the mockup shows per-instance facts), and two flags, `silenced_on_target` and `silenced_on_group`, rather than a scope value: the frontend already knows which grain a row is filed against, so the two flags are the whole answer, and no parallel scope enum is introduced.
- Listed instances are the degraded ones and those an instance silence quiets; `passing_instances` counts the instances whose effective result is passed, and `skipped_instances` those skipped other than by a silence (reported skipped, or graded skipped by a rule), as CHK "Silencing one instance" now says.
- A check silenced whole presents every instance as skipped, as the check itself presents, so it lists only its instance-silenced instances and counts none as skipped.
- The as-of-past view grades with no prior state, as it always has for plain checks, so a past broken instanced check presents as broken with no instances listed.
- Listed silences gain `instance_label` (from the current state; `null` when the key is not reported or has no label) and `instance_reported` (`null` for a whole-check silence). A group silence's key is looked for across every state the silence covers in the group.
- The issue payload's `instances` is its degraded instances only (key, label, effective), most urgent first.
- MCP: `find_issues` and `get_incident` issues gain `degraded_instances` (absent for a plain check); `get_issue` gains the check's `detail` and every instance with its own fields.
- Two small frontend guards landed with this section, ahead of section 7: the checks table ignores instance silences when matching a row's whole-check silence, and the silences section passes a row's instance when unsilencing it.

## Notes from the frontend (section 7)

- An instance is named by its label with its key after it, shortened to its first eight and last four characters past fourteen (`shortInstanceKey`), the full key on hover; an instance without a label is named by its key alone (`InstanceName`).
- A silence whose key the check no longer reports has no label to show (the list gives `instance_label: null` for it), so it reads by its key with the "not reported" chip, where the mockup draws a label.
- The mockup's orange left rule marks what is new, not a border to draw, so the instance list and the issue's picker carry none.
- The issue's picker defaults to the whole check, as the panel did before it had a choice.
- `e2e/seed.ts` gains `seedInstancedCheck` (instances and grading inputs stored as ingestion writes them, its catalog entry reviewed at a `failed` ceiling so a re-grade keeps failures) and an `instance` option on both silence seeds.

## Build checklist

Layer 1 only; layer 2 is split out (see the last section).
Each section leaves the tree building and tested, so they can land as separate commits in this order.

### 1. One grading path for instances (database)

- [x] `database::issues::CheckInstance`: add `key: String`; make `label` an `Option<String>` that falls back to the key wherever an instance is named. Mirror both in `GradedInstance`
- [x] Extract the per-instance grading and aggregation out of `file_check_instances` into a shared `grade_instances` (fleet grading + scoped chain per instance, most urgent non-skipped wins, all-skipped is skipped, `escalates` from non-skipped instances only), so push ingestion and Canopy's own filings call the same function
- [x] Rule context per instance (spec: CHK#checks-with-instances): instance detail merged over the check's shared detail, `result` set to the instance's observed result, and the caller's `status_extra` passed through instead of the empty map `file_check_instances` uses today
- [x] A plain check grades as one instance with an empty key and no label, so its rule context is identical to today's
- [x] Store every instance keyed by key (label, observed, effective, own detail) in its own nullable `issues.instances` column (null for a plain check), instead of degraded instances only, with `detail` holding a plain check's fields or an instanced check's shared ones; counts are derived (`StoredInstances::degraded`). Whether a state has instances is the column, never the shape of `detail`. The silenced-instance listing, "not reported" marking and broken presentation all read the full set from here
- [x] Store the inputs the filing graded the instances with (`issues.grading_context`: the `status_extra` and tags passed to `grade_instances`), set exactly when `instances` is, so a re-grade replays them rather than reconstructing them
- [x] Store the filing's title in `issues.title` whatever the result (`description` stays the headline only while degraded); a stamp without a title keeps the stored one
- [x] Message: an instanced check's message is composed by Canopy from the degraded graded instances, named by label

### 2. Instance silences (storage)

- [x] `just migration instance_silences`: add `instance_key TEXT NULL` to `scoped_check_policies`, and recreate every per-scope unique index (application, machine, group, cluster, global) to include it with `NULLS NOT DISTINCT`, as `2026-09-02-080628-0000_check_namespace` does for the namespace columns. The same migration adds `issues.title`, `issues.instances` and `issues.grading_context` (section 1)
- [x] `ScopedCheckPolicy::{get, silence, unsilence, list_silences}`: take `Option<&str>` instance key; `chain_for` / `chains_for_scope` return instance rows tagged with their key
- [x] `grade_instances` applies an instance-keyed row only to the instance with that key; a row with no key applies to every instance, as now
- [x] `silenced_refs.rs`: `ServerSilencedRef`, `MachineSilencedRef`, `ServerGroupSilencedRef`, `ClusterSilencedRef` gain `instance: Option<String>` through `add` / `remove` / `list_*`. `is_silenced` and `silenced_health_checks_for_server` only count keyless rows as silencing the whole check
- [x] Silencing or unsilencing an instance re-grades the stored state from its kept instances (observed + detail) and stored grading inputs right away, then re-runs incident membership. The existing `reevaluate_open_issues_for_*_ref` only re-checks membership, which is enough for a whole-check silence but not for one that changes the check's effective result. A re-graded state takes the generic instanced message (`GradedCheck::message`), which is what reported instanced checks are filed with; Canopy's own instanced checks file with their own composers, whose wording their next sweep restores
- [x] Database tests (`crates/database/tests/it/`): instance silence at application and group scope, uniqueness with and without a key, re-grade on silence/unsilence, all-instances-silenced is skipped

### 3. Brokenness is whole-check

- [x] A `broken` result for a check whose stored state holds instances keeps those instances, presents each as broken, retains the last definite contribution through the existing broken path, and recovers none of them
- [x] `grade_instances` refuses a broken instance (debug assertion for Canopy's own callers; the push path rejects it in section 4 before it gets here)

### 4. Push wire shape (public server)

- [x] `statuses.rs` `HealthCheck`: add `detail: Option<Map>` and `instances: Option<BTreeMap<String, HealthCheckInstance>>`; new `HealthCheckInstance { result, label, detail }` with utoipa docs. `extra` stays flattened for the flat form
- [x] `parse_health` refusals, each a path-qualified `BadRequest` like the existing ones:
  - [x] more than one of `result`, `healthy`, `instances`, or none of them
  - [x] flat fields together with `detail`
  - [x] flat fields next to `instances`
  - [x] `detail` that isn't an object, on a check or an instance
  - [x] empty instance key
  - [x] instance without `result`, with `healthy`, or with `broken`
- [x] Replace `collect_check_results`' `(CheckResult, &Map)` with one parsed shape (name, single result or instances, detail), with flat extras folded into detail so everything downstream reads one form. `per_check_description` reads that detail
- [x] `file_health_events`: grade every check through `grade_instances`, a plain check as its single instance, keeping the existing issue upsert, omission recovery and broken handling. `instances: {}` recovers every held instance; a check switching between plain and instanced stays one state
- [x] Response (`effective_check_severities`): an instanced check is answered once per check; confirm no per-instance entries leak in
- [x] Status history records the push verbatim, `instances` included (HST)
- [x] `just gen-openapi && just gen-api`; commit `crates/public-server/openapi.json` and `crates/canopy-api/`. `HealthCheck` is `#[non_exhaustive]` with a builder, so the new fields pass `cargo-semver-checks`; run `just check-generated`
- [x] Public-server tests, new `tests/it/instanced_checks.rs` (declared in `tests/it/main.rs`):
  - [x] every refusal above
  - [x] nested `detail` and flat form grade alike
  - [x] instance grading and aggregation, message naming degraded instances by label
  - [x] instance omission recovers it; `instances: {}` recovers all
  - [x] broken check keeps and presents held instances, recovers none
  - [x] plain → instanced → plain is one state
  - [x] one catalog rule grades the plain and instanced forms of a check alike
  - [x] instance silence on the application quiets one instance only
  - [x] response answers an instanced check once

### 5. Canopy's own instanced checks take keys

- [x] `backup/staleness.rs`, `backup/reconcile.rs`: key by backup type
- [x] `restore.rs`: key from `ReplicaKey` (type, intent, declared name); drop the joined type-and-intent field from `instance_identity`
- [x] `reporting_schemas.rs`: key by version
- [x] `relay-protocol` `SubstrateInstance`: add `key`; `SubstrateFiling` gains a check-level broken outcome; `SubstrateInstance::only` keys `""`. The relay has no deployments yet, so the protocol changes in place: no version bump or compatibility with older relays
- [x] `crates/relay`: node pools keyed by pool name; `Determination::refused` and `watch.rs`'s `broken()` report a check-level broken instead of a broken instance
- [x] `jobs/relay/ingest.rs`: map keys and check-level broken; stop passing the relay's `message` through for instanced checks
- [ ] Self-alerts relay-version check (SELF): confirm how its per-cluster instances are filed and key them by cluster
  - Nothing files this condition yet: there is no relay-version check in `self_alerts.rs` or anywhere else, and no named relay version is stored (K8S "Keeping a relay current" is unimplemented). So there is nothing to key on this card; whoever implements it files one instance per registered cluster keyed by the cluster's id, which is stable where its name is not.
- [x] Update `crates/database/tests/it/cluster_checks.rs` and the backup/restore tests for keys

### 6. Private API

- [x] `commons_types::status::ConsolidatedCheck`: add `instances` (key, label, observed, effective, silenced scope) for degraded and silenced instances, and `passing_instances: usize`; `detail` becomes the shared detail for an instanced check
- [x] `fns/statuses.rs`: populate both for current and as-of-past consolidated checks
- [x] `fns/silenced_refs.rs`: `silence_*` / `unsilence_*` take optional `instance`; `list_*` return the instance key, its label from the state, and whether the check currently reports that key
- [x] `fns/issues.rs`: issue payload carries its degraded instances (key, label, effective) for the issue silence picker
- [x] `fns/healthchecks.rs` `sample`: for an instanced check, present the most urgent instance's fields merged over the shared detail, as a rule reads them
- [x] MCP check-state tools: the stored detail shape changes, so check what they return reads well for instanced checks (MCP)
- [x] `just gen-openapi`; commit `private-web/openapi.json` and `private-web/src/api-types.ts`
- [x] Private-server tests: instance silence/unsilence endpoints at each scope, list with the reported flag, consolidated instances and passing count, issue instances

### 7. Frontend

- [x] `ChecksTable.tsx` `CheckRow`: instance sub-list (result icon, label, truncated key), passing count, per-instance silence button reusing `SilenceScopeRow`, per-instance silenced chip; `CheckExtrasList` shows the shared detail only
- [x] `IssueRow.tsx` silence panel: select of "Whole check" plus the issue's degraded instances, shown when the issue has instances; scope buttons pass the chosen instance
- [x] `SilencedRefsSection.tsx`: instance label and key on instance silences, "not reported" chip when the check no longer reports the key
- [x] `types.ts`: re-export the new wire types
- [x] e2e: extend `e2e/seed.ts` to seed an instanced check state and an instance silence; new `e2e/instance-silences.spec.ts` covering silencing from the checks table, from an incident's issue, and listing (including "not reported")
- [x] `just typecheck`

### 8. Wrap-up

- [ ] `just check`, `just test`, `cargo fmt`, no new warnings
- [ ] Draft the card's test cases ([Draft test cases] skill)
- [ ] Split layer 2 into its own card via the card breakdown; drop the layer 2 prose from this plan once it lives there
- [ ] Note on #bestool/P3 when a `bes-canopy-api` release carries the new `HealthCheck` shape
