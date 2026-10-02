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
Its four side lists fold into instance results: `disabled` and `upstream_absent` are skipped, `errored` and `unmonitored` are broken.
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
- Brokenness is not held per instance: CHK keeps one state per check. A broken instance feeds the check's most-urgent aggregate like any other result, and a check left broken by that aggregate retains its last definite result through the existing broken rule.
- Restore replicas key on type, intent and declared name together (the existing `ReplicaKey`), which replaces the joined type-and-intent detail field.
- The bestool side is card P3 in the bestool workspace.
- Rules apply to instances transparently (CHK, "Checks with instances"): one grading path, with a plain check graded as its own single instance. `file_check_instances` already grades each instance through the catalog entry and scoped chain with `check.result` set to the instance's result. Two gaps to close:
  - Canopy's own filings pass an empty `status_extra`; reported instances must get the push's report detail, as a plain check from the same push does.
  - The rule-authoring sample (`fns/healthchecks.rs`, `sample`) must present one instance's fields merged over the check's shared detail, not the raw `instances` object.
