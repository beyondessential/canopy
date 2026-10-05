# Test cases: no plain IDs in human-readable text

## Applications read by name or type

- [x] An incident notification lists an issue on an unnamed, hostless application by its type ("on Tamanu central"), never its id (verifies spec: INC, FLT)
- [x] The application reachability check's message names an unnamed application by its type (verifies spec: FLT)
- [x] The forgotten-pause self-alert names an unnamed application by its type (verifies spec: FLT)
- [ ] A restore migration alert names an unnamed application by its type, qualified with its host when it has one (verifies spec: BKJ)
- [x] The application breadcrumb and the migration tests table read an unnamed application as its type (verifies spec: FLT)
- [ ] The upgrade plan's failed test and the inventory read an unnamed application as its type (verifies spec: FLT)

## Machines always have a name

- [x] Creating a machine with a blank name is refused (verifies spec: FLT)
- [x] Renaming a machine to blank is refused, in the API and in the edit form (verifies spec: FLT)
- [x] The migration names an unnamed machine from its reported hostname, then its tailnet node, then its first named application, then "Unnamed machine" (verifies spec: FLT)
- [x] The backup recent-runs table and the restore checks table name the machine a run came from, including one that has since left the group (verifies spec: FLT)

## Groups and identities in messages

- [x] Claiming a domain that overlaps another group's names that group (verifies spec: FLT)
- [x] Configuring a bucket/prefix already used by another group names that group
- [ ] Attaching a tailnet identity already bound elsewhere names the machine it speaks for
- [x] A Slack delivery failure self-alert names the incident's target rather than outbox or incident ids
- [x] A device with no named key reads by its tailnet name, and one with nothing to go by reads as "Unnamed device"
