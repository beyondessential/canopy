# Test cases: no plain IDs in human-readable text

## Applications read by name or type

- [ ] An incident notification lists an issue on an unnamed, hostless application by its type ("on Tamanu central"), never its id (verifies spec: INC, FLT)
- [ ] The application reachability check's message names an unnamed application by its type (verifies spec: FLT)
- [ ] The forgotten-pause self-alert names an unnamed application by its type (verifies spec: FLT)
- [ ] A restore migration alert names an unnamed application by its type, qualified with its host when it has one (verifies spec: BKJ)
- [ ] The application breadcrumb, the upgrade plan's failed test, the inventory, and the migration tests table read an unnamed application as its type (verifies spec: FLT)

## Machines always have a name

- [ ] Creating a machine with a blank name is refused (verifies spec: FLT)
- [ ] Renaming a machine to blank is refused, in the API and in the edit form (verifies spec: FLT)
- [ ] The migration names an unnamed machine from its reported hostname, then its tailnet node, then its first named application, then "Unnamed machine" (verifies spec: FLT)
- [ ] The backup recent-runs table and the restore checks table name the machine a run came from, including one that has since left the group (verifies spec: FLT)

## Groups and identities in messages

- [ ] Claiming a domain that overlaps another group's names that group (verifies spec: FLT)
- [ ] Configuring a bucket/prefix already used by another group names that group
- [ ] Attaching a tailnet identity already bound elsewhere names the machine it speaks for
- [ ] A Slack delivery failure self-alert names the incident's target rather than outbox or incident ids
- [ ] A device with no named key reads by its tailnet name, and one with nothing to go by reads as "Unnamed device"
