# Audit log: test cases

Covers this card's part of [AUD](../../specs/platform/audit-log.md). Archive and provenance cases belong to the follow-up cards in the breakdown.

## Declaring

- [ ] A route with no audit clause fails to compile (verifies spec: AUD#what-is-recorded)
- [ ] An audited route whose handler does not take the audit extractor fails to compile
- [ ] An audit-silent write produces no entry; an audited read (SQL playground) produces one (verifies spec: AUD#what-is-recorded)

## Entries

- [ ] An entry holds when, actor, operation and the declared fields only; an undeclared argument, including a secret, never appears (verifies spec: AUD#what-an-entry-holds)
- [ ] An entry carries no grade, mode or session (verifies spec: AUD#what-is-recorded)
- [ ] A field renamed for serde is recorded under the name the request uses
- [ ] Entries are found by a recorded value and counted by it over a window (verifies spec: AUD#what-an-entry-holds)

## Actors

- [ ] An operator action records the tailnet login (verifies spec: AUD#actors)
- [ ] A device credential issuance records the device, and the application where it acted for one; a status push records nothing (verifies spec: AUD#actors)
- [ ] An audited fleet query tool records the access token (verifies spec: AUD#actors)
- [ ] A job using the recorder records Canopy and the job (verifies spec: AUD#actors)

## Writing as a condition

- [ ] An audited action whose entry cannot be written is refused and changes nothing (verifies spec: AUD#writing-as-a-condition-of-the-action)
- [ ] An audited action that errors leaves no entry and no change (verifies spec: AUD#writing-as-a-condition-of-the-action)
- [ ] An action with an outside effect leaves a pending entry before the effect and a confirmation after it (verifies spec: AUD#writing-as-a-condition-of-the-action)
- [ ] An outside-effect action that fails after the effect leaves its pending entry, shown as unconfirmed (verifies spec: AUD#writing-as-a-condition-of-the-action)
- [ ] With no range to write into, audited actions are refused and the runway self-alert is active (verifies spec: AUD#self-alerts)

## Refusals

- [ ] A non-administrator reaching an audited operator handler leaves a refusal entry with the reason (verifies spec: AUD#refusals)
- [ ] A safety-mode refusal leaves no entry (verifies spec: AUD#refusals)

## Reading

- [ ] The log page lists entries newest first and filters by actor, operation, time, and the thing acted on (verifies spec: AUD#reading-the-log)
- [ ] A non-administrator cannot reach the log page (verifies spec: AUD#reading-the-log)
- [ ] "Find audit entries" returns entries with the same filters to any caller of the fleet query interface (verifies spec: MCP#audit-log)

## Keeping entries

- [ ] Nothing is removed from an unarchived week, however old (verifies spec: AUD#keeping-entries)
