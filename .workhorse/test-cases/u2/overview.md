# Audit log: test cases

Covers this card's part of [AUD](../../specs/platform/audit-log.md). Archive and provenance cases belong to the follow-up cards in the breakdown.

## Declaring

- [ ] A route with no audit clause fails to compile (verifies spec: AUD#what-is-recorded)
- [ ] An audited route whose handler does not take the audit extractor fails to compile
- [ ] An audit-silent write produces no entry; an audited read (SQL playground) produces one (verifies spec: AUD#what-is-recorded)
- [ ] A SQL playground query that errors leaves its entry, settled failed (verifies spec: AUD#writing-as-a-condition-of-the-action)
- [ ] An audited handler that succeeds without recording an action fails and its change is rolled back (verifies spec: AUD#what-is-recorded)
- [ ] An audited handler recording an action its route does not declare fails and its change is rolled back (verifies spec: AUD#what-is-recorded)
- [ ] A bulk change records one entry per action (verifies spec: AUD#what-is-recorded)

## Entries

- [ ] An entry holds when, actor, action, the records acted on and the action's detail; a secret sent in the request never appears (verifies spec: AUD#what-an-entry-holds)
- [ ] An action that creates a record names the record it created (verifies spec: AUD#what-an-entry-holds)
- [ ] An entry naming a record still shows the record's name as it was after the record is renamed, and after it is deleted; deleting the record is not blocked (verifies spec: AUD#what-an-entry-holds)
- [ ] A credential issuance records the issued credential's identifier (verifies spec: AUD#what-an-entry-holds)
- [ ] An action's name is unaffected by renaming the route that takes it (verifies spec: AUD#actions)
- [ ] Entries are found by a record acted on and by a detail value, and counted by a detail value over a window (verifies spec: AUD#what-an-entry-holds)

## Actors

- [ ] An operator action records the tailnet login (verifies spec: AUD#actors)
- [ ] A device credential issuance records the device, and the application where it acted for one; a status push that creates nothing records nothing (verifies spec: AUD#actors)
- [ ] A status push bringing two new applications records two arrivals, the reporting machine as actor (verifies spec: AUD#what-is-recorded, FLT)
- [ ] A status push bringing two checks new to the catalogue records two registrations; a check already catalogued records none (verifies spec: AUD#what-is-recorded, CHK)
- [ ] A device of a role other than machine (relay, backup-restore) is recorded and presented as a device of its role (verifies spec: AUD#actors)
- [ ] A machine enrolment records the enrolled device as its actor (verifies spec: AUD#actors)
- [ ] An audited fleet query tool records the access token (verifies spec: AUD#actors)
- [ ] A job using the recorder records Canopy and the job (verifies spec: AUD#actors)

## Writing as a condition

- [ ] An audited action whose entry cannot be written is refused and changes nothing (verifies spec: AUD#writing-as-a-condition-of-the-action)
- [ ] An audited action that errors leaves no entry and no change (verifies spec: AUD#writing-as-a-condition-of-the-action)
- [ ] An action with an outside effect leaves a pending entry before the effect, settled done with the rest of its work (verifies spec: AUD#writing-as-a-condition-of-the-action)
- [ ] Settling adds what the action learned to the entry's detail, such as an issued credential's identifier (verifies spec: AUD#writing-as-a-condition-of-the-action)
- [ ] An outside effect known not to have taken place settles its entry failed, and the failure stands though the action rolls back (verifies spec: AUD#writing-as-a-condition-of-the-action)
- [ ] An entry already settled cannot be settled again (verifies spec: AUD#writing-as-a-condition-of-the-action)
- [ ] A pending entry is absent from the log page and "Find audit entries" until it settles or lapses (verifies spec: AUD#writing-as-a-condition-of-the-action)
- [ ] A pending entry over an hour old cannot be settled and is presented as unconfirmed (verifies spec: AUD#writing-as-a-condition-of-the-action)
- [ ] An entry committed ahead stands when the handler then errors and its other changes roll back (verifies spec: AUD#writing-as-a-condition-of-the-action)
- [ ] An outside-effect action that fails after the effect without settling leaves its entry pending, shown as unconfirmed once it lapses (verifies spec: AUD#writing-as-a-condition-of-the-action)
- [ ] With no range to write into, audited actions are refused and the runway self-alert is active (verifies spec: AUD#self-alerts)


## Reading

- [ ] The log page lists entries newest first and filters by actor, action, time, and the records acted on (verifies spec: AUD#reading-the-log)
- [ ] Opening the log page reads only the last ninety days from the database and fetches nothing from the archive (verifies spec: AUD#reading-the-log)
- [ ] A non-administrator cannot reach the log page (verifies spec: AUD#reading-the-log)
- [ ] "Find audit entries" returns entries with the same filters to any caller of the fleet query interface (verifies spec: MCP#audit-log)
- [ ] "Find audit entries" refuses a window longer than ninety days, naming the limit, and returns no year-kept entry older than that (verifies spec: MCP#audit-log)

## Keeping entries

- [ ] Nothing is removed from an unarchived week, however old (verifies spec: AUD#keeping-entries)
- [ ] An action marked for a year is kept a year (verifies spec: AUD#keeping-entries)
