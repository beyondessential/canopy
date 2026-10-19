# Audit log: test cases

Covers this card's part of [AUD](../../specs/platform/audit-log.md). Archive and provenance cases belong to the follow-up cards in the breakdown.

## Declaring

- [x] A route with no audit clause fails to compile (verifies spec: AUD#what-is-recorded)
- [x] An audited route whose handler does not take the audit extractor fails to compile (compile_fail doctests on the macro, run with `cargo test --doc -p canopy-utoipa-axum`)
- [x] An audit-silent write produces no entry; an audited read (SQL playground) produces one (verifies spec: AUD#what-is-recorded)
- [x] A SQL playground query that errors leaves its entry, settled failed (verifies spec: AUD#writing-as-a-condition-of-the-action)
- [x] An audited handler that succeeds without recording an action fails and its change is rolled back (verifies spec: AUD#what-is-recorded)
- [x] An audited handler that declares the request changed nothing completes with no entry, and fails if it also records an action (verifies spec: AUD#what-is-recorded)
- [x] An audited handler recording an action its route does not declare fails and its change is rolled back (verifies spec: AUD#what-is-recorded)
- [x] A bulk change records one entry per action (verifies spec: AUD#what-is-recorded)

## Entries

- [x] An entry holds when, actor, action, the records acted on and the action's detail; a secret sent in the request never appears (verifies spec: AUD#what-an-entry-holds)
- [x] An action that creates a record names the record it created (verifies spec: AUD#what-an-entry-holds)
- [x] An entry naming a record still shows the record's name as it was after the record is renamed, and after it is deleted; deleting the record is not blocked (verifies spec: AUD#what-an-entry-holds)
- [x] A credential issuance records the issued credential's identifier (verifies spec: AUD#what-an-entry-holds)
- [x] An action's name is unaffected by renaming the route that takes it (verifies spec: AUD#actions)
- [x] Entries are found by a record acted on and by a detail value, and counted by a detail value over a window (verifies spec: AUD#what-an-entry-holds)

## Actors

- [x] An operator action records the tailnet login (verifies spec: AUD#actors)
- [x] A device credential issuance records the device, and the application where it acted for one; a status push that creates nothing records nothing (verifies spec: AUD#actors)
- [x] A status push bringing two new applications records two arrivals, the reporting machine as actor (verifies spec: AUD#what-is-recorded, FLT)
- [x] A status push bringing two checks new to the catalogue records two registrations; a check already catalogued records none (verifies spec: AUD#what-is-recorded, CHK)
- [x] A device of a role other than machine (relay, backup-restore) is recorded and presented as a device of its role (verifies spec: AUD#actors)
- [x] A machine enrolment records the enrolled device as its actor (verifies spec: AUD#actors)
- [ ] An audited fleet query tool records the access token (verifies spec: AUD#actors)
- [x] A job using the recorder records Canopy and the job (verifies spec: AUD#actors)

## Writing as a condition

- [x] An audited action whose entry cannot be written is refused and changes nothing (verifies spec: AUD#writing-as-a-condition-of-the-action)
- [x] An audited action that errors leaves no entry and no change (verifies spec: AUD#writing-as-a-condition-of-the-action)
- [x] An action with an outside effect leaves a pending entry before the effect, settled done with the rest of its work (verifies spec: AUD#writing-as-a-condition-of-the-action)
- [x] Settling adds what the action learned to the entry's detail, such as an issued credential's identifier (verifies spec: AUD#writing-as-a-condition-of-the-action)
- [x] An outside effect known not to have taken place settles its entry failed, and the failure stands though the action rolls back (verifies spec: AUD#writing-as-a-condition-of-the-action)
- [x] An entry already settled cannot be settled again (verifies spec: AUD#writing-as-a-condition-of-the-action)
- [x] A pending entry is absent from the log page and "Find audit entries" until it settles or lapses (verifies spec: AUD#writing-as-a-condition-of-the-action)
- [x] A pending entry over an hour old cannot be settled and is presented as unconfirmed (verifies spec: AUD#writing-as-a-condition-of-the-action)
- [x] An entry committed ahead stands when the handler then errors and its other changes roll back (verifies spec: AUD#writing-as-a-condition-of-the-action)
- [x] An outside-effect action that fails after the effect without settling leaves its entry pending, shown as unconfirmed once it lapses (verifies spec: AUD#writing-as-a-condition-of-the-action)
- [x] With no range to write into, audited actions are refused and the runway self-alert is active (verifies spec: AUD#self-alerts)


## Importance

- [x] The log page and "Find audit entries" leave routine actions out by default, and include them on request or when the action is named (verifies spec: AUD#reading-the-log, MCP#audit-log)
- [x] Every route declares whether it is audited, and every action a route names is in the catalogue (verifies spec: AUD#what-is-recorded)
- [x] A handler declared as recording only for what it creates completes without an entry when it creates nothing, and is still held to its declared actions (verifies spec: AUD#what-is-recorded)

## Reading

- [x] The log page lists entries newest first and filters by actor, action, time, and the records acted on (verifies spec: AUD#reading-the-log)
- [x] Opening the log page reads only the last ninety days from the database and fetches nothing from the archive (verifies spec: AUD#reading-the-log)
- [x] A non-administrator cannot reach the log page (verifies spec: AUD#reading-the-log)
- [x] "Find audit entries" returns entries with the same filters to any caller of the fleet query interface (verifies spec: MCP#audit-log)
- [x] "Find audit entries" refuses a window longer than ninety days, naming the limit, and returns no year-kept entry older than that (verifies spec: MCP#audit-log)

## Keeping entries

- [ ] Nothing is removed from an unarchived week, however old (verifies spec: AUD#keeping-entries)
- [x] An action marked for a year is kept a year (verifies spec: AUD#keeping-entries)
