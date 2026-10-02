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

- [ ] `statuses.rs` `HealthCheck`: add `detail: Option<Map>` and `instances: Option<BTreeMap<String, HealthCheckInstance>>`; new `HealthCheckInstance { result, label, detail }` with utoipa docs. `extra` stays flattened for the flat form
- [ ] `parse_health` refusals, each a path-qualified `BadRequest` like the existing ones:
  - [ ] more than one of `result`, `healthy`, `instances`, or none of them
  - [ ] flat fields together with `detail`
  - [ ] flat fields next to `instances`
  - [ ] `detail` that isn't an object, on a check or an instance
  - [ ] empty instance key
  - [ ] instance without `result`, with `healthy`, or with `broken`
- [ ] Replace `collect_check_results`' `(CheckResult, &Map)` with one parsed shape (name, single result or instances, detail), with flat extras folded into detail so everything downstream reads one form. `per_check_description` reads that detail
- [ ] `file_health_events`: grade every check through `grade_instances`, a plain check as its single instance, keeping the existing issue upsert, omission recovery and broken handling. `instances: {}` recovers every held instance; a check switching between plain and instanced stays one state
- [ ] Response (`effective_check_severities`): an instanced check is answered once per check; confirm no per-instance entries leak in
- [ ] Status history records the push verbatim, `instances` included (HST)
- [ ] `just gen-openapi && just gen-api`; commit `crates/public-server/openapi.json` and `crates/canopy-api/`. `HealthCheck` is `#[non_exhaustive]` with a builder, so the new fields pass `cargo-semver-checks`; run `just check-generated`
- [ ] Public-server tests, new `tests/it/instanced_checks.rs` (declared in `tests/it/main.rs`):
  - [ ] every refusal above
  - [ ] nested `detail` and flat form grade alike
  - [ ] instance grading and aggregation, message naming degraded instances by label
  - [ ] instance omission recovers it; `instances: {}` recovers all
  - [ ] broken check keeps and presents held instances, recovers none
  - [ ] plain → instanced → plain is one state
  - [ ] one catalog rule grades the plain and instanced forms of a check alike
  - [ ] instance silence on the application quiets one instance only
  - [ ] response answers an instanced check once

### 5. Canopy's own instanced checks take keys

- [ ] `backup/staleness.rs`, `backup/reconcile.rs`: key by backup type
- [ ] `restore.rs`: key from `ReplicaKey` (type, intent, declared name); drop the joined type-and-intent field from `instance_identity`
- [ ] `reporting_schemas.rs`: key by version
- [ ] `relay-protocol` `SubstrateInstance`: add `key`; `SubstrateFiling` gains a check-level broken outcome; `SubstrateInstance::only` keys `""`. The relay has no deployments yet, so the protocol changes in place: no version bump or compatibility with older relays
- [ ] `crates/relay`: node pools keyed by pool name; `Determination::refused` and `watch.rs`'s `broken()` report a check-level broken instead of a broken instance
- [ ] `jobs/relay/ingest.rs`: map keys and check-level broken; stop passing the relay's `message` through for instanced checks
- [ ] Self-alerts relay-version check (SELF): confirm how its per-cluster instances are filed and key them by cluster
- [ ] Update `crates/database/tests/it/cluster_checks.rs` and the backup/restore tests for keys

### 6. Private API

- [ ] `commons_types::status::ConsolidatedCheck`: add `instances` (key, label, observed, effective, silenced scope) for degraded and silenced instances, and `passing_instances: usize`; `detail` becomes the shared detail for an instanced check
- [ ] `fns/statuses.rs`: populate both for current and as-of-past consolidated checks
- [ ] `fns/silenced_refs.rs`: `silence_*` / `unsilence_*` take optional `instance`; `list_*` return the instance key, its label from the state, and whether the check currently reports that key
- [ ] `fns/issues.rs`: issue payload carries its degraded instances (key, label, effective) for the issue silence picker
- [ ] `fns/healthchecks.rs` `sample`: for an instanced check, present the most urgent instance's fields merged over the shared detail, as a rule reads them
- [ ] MCP check-state tools: the stored detail shape changes, so check what they return reads well for instanced checks (MCP)
- [ ] `just gen-openapi`; commit `private-web/openapi.json` and `private-web/src/api-types.ts`
- [ ] Private-server tests: instance silence/unsilence endpoints at each scope, list with the reported flag, consolidated instances and passing count, issue instances

### 7. Frontend

- [ ] `ChecksTable.tsx` `CheckRow`: instance sub-list (result icon, label, truncated key), passing count, per-instance silence button reusing `SilenceScopeRow`, per-instance silenced chip; `CheckExtrasList` shows the shared detail only
- [ ] `IssueRow.tsx` silence panel: select of "Whole check" plus the issue's degraded instances, shown when the issue has instances; scope buttons pass the chosen instance
- [ ] `SilencedRefsSection.tsx`: instance label and key on instance silences, "not reported" chip when the check no longer reports the key
- [ ] `types.ts`: re-export the new wire types
- [ ] e2e: extend `e2e/seed.ts` to seed an instanced check state and an instance silence; new `e2e/instance-silences.spec.ts` covering silencing from the checks table, from an incident's issue, and listing (including "not reported")
- [ ] `just typecheck`

### 8. Wrap-up

- [ ] `just check`, `just test`, `cargo fmt`, no new warnings
- [ ] Draft the card's test cases ([Draft test cases] skill)
- [ ] Split layer 2 into its own card via the card breakdown; drop the layer 2 prose from this plan once it lives there
- [ ] Note on #bestool/P3 when a `bes-canopy-api` release carries the new `HealthCheck` shape
