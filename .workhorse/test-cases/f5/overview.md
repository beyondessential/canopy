# F5: Ansible rework, import and runs

Scenarios for the rename to Ansible, the inventory file import, the variable catalogue, runs started from the operator interface, and the maintenance windows runs declare.

## Environments are their machines

- [x] A group's environments are the ranks its machines sit at, whether or not anything on them has reported (verifies spec: GRP, INV)
- [x] A machine with a bound device and nothing on it is served to a run (verifies spec: INV)
- [x] A ranked machine with no device and nothing reported is left out, not refused (verifies spec: INV)
- [x] A rank no machine holds is refused as no environment (verifies spec: INV)
- [x] Two members whose tailnet names share a first label are served under their Canopy names (verifies spec: INV)
- [x] Two members that would still share a host name are refused (verifies spec: INV)
- [x] The picker offers the environment of a ranked machine with nothing on it (verifies spec: GRP)

## Catalogue

- [x] A document that is a JSON Schema with `x-canopy` annotations reads as a catalogue; one that is not is refused with a reason (verifies spec: ANSVAR)
- [x] A value is held to its type, format, bounds and pattern, and a fault inside a structured value names where (verifies spec: ANSVAR)
- [x] A variable is held to the scopes the catalogue allows, and a reserved one cannot be set (verifies spec: ANSVAR)
- [x] A name the catalogue does not describe is set freely (verifies spec: ANSVAR)
- [x] Set is refused where secrecy disagrees with the catalogue (verifies spec: ANSVAR)
- [x] A value set before the catalogue described it stays, marked where it no longer matches (verifies spec: ANSVAR)
- [x] The last copy that read is kept when a later attempt does not, and a good read clears the problem (verifies spec: ANSVAR)
- [x] A copy that does not read raises a self-alert straight away and a good read clears it (verifies spec: SELF)
- [x] An unreachable repository raises the alert only after a day without a good read (verifies spec: SELF)
- [x] A plain value the catalogue comes to call secret moves into the secret store, once, audited as Canopy (verifies spec: ANSVAR)
- [x] A secret the catalogue stops calling secret stays in the secret store (verifies spec: ANSVAR)
- [x] The catalogue reaches the operator interface, null before one is read (verifies spec: ANSVAR)

## Import

- [x] The file's `all` variables go to the environment and a host's own to its machine (verifies spec: ANSIMP)
- [x] A group's variable reaches each machine in it or beneath, nearer group first, then priority, then name (verifies spec: ANSIMP)
- [x] YAML 1.1 scalars read as Ansible reads them: `yes`, octal, sexagesimal, quoted strings (verifies spec: ANSIMP)
- [x] A vault-encrypted value, a file with no `all` group and a malformed file are refused naming why (verifies spec: ANSIMP)
- [x] Reserved variables are left out, and a `tamanu_clone` that disagrees with the environment is flagged (verifies spec: ANSIMP)
- [x] A host is proposed for the one machine it matches by key, tailnet name, public host name or address (verifies spec: ANSIMP)
- [x] A machine with no device matches by its name and its applications' hosts (verifies spec: ANSIMP)
- [x] A host key that is not the machine's address becomes its `ansible_host`, and a `:port` its `ansible_port` (verifies spec: ANSIMP)
- [x] Every imported machine has `managed_by_ansible` set (verifies spec: ANSIMP)
- [x] Secrets are proposed by the catalogue, then by name or the names inside the value, and the operator can change a guess (verifies spec: ANSIMP)
- [x] A secret's value is not shown in the preview or recorded in the log (verifies spec: ANSIMP)
- [x] A variable breaking the rules for setting one by hand is named and stops the import (verifies spec: ANSIMP)
- [x] Two hosts on one machine, or two machines at one address, stop the import (verifies spec: ANSIMP)
- [x] An import is refused where the environment or its machines carry a variable, and where the machines changed since the preview (verifies spec: ANSIMP)
- [x] A group-wide variable does not hold an import back and is named where overridden (verifies spec: ANSIMP)
- [x] Applying sets the variables, writes secrets to the store, appends the notes and records one entry (verifies spec: ANSIMP)
- [ ] A failure after secrets are written removes them and settles the entry as failed (verifies spec: ANSIMP)
- [x] The operator interface offers the import, shows the preview, and applies once every host is matched or skipped (verifies spec: ANSIMP)

## Variables in the operator interface

- [x] Variables are collapsed to a count and open by scope (verifies spec: ANSVAR)
- [x] The entry row sets a variable and leaves a fresh row (verifies spec: ANSVAR)
- [x] A secret is shown by name only (verifies spec: ANSVAR)
- [x] A scope edited as text saves the difference as sets and removes (verifies spec: ANSVAR)
- [x] Names complete from the catalogue, set names sink, and a value is entered in a form shaped by the variable's type
- [x] An object is entered as a field for each property, required ones marked
- [x] A list is entered one item at a time, and a list of objects takes a row of fields per item
- [x] A variable limited to other scopes or application types is not offered

## Runs

- [x] Starting work takes the lease with the playbook, intent, limit, tags and assignments (verifies spec: ANSRUN)
- [x] The lease is pending until the run first reads, and is released by Canopy after five minutes unread (verifies spec: ANSRUN)
- [x] A run's first read is audited as the run starting, and later reads stay routine (verifies spec: ANSRUN)
- [x] A lease expiring, and one released unstarted, are audited as Canopy's own actions (verifies spec: ANSRUN)
- [x] A limit is evaluated as an Ansible host pattern, and one matching nothing or unreadable is refused (verifies spec: ANSRUN)
- [x] A machine with nothing on it is served with its assigned type (verifies spec: ANSRUN)
- [x] At most one machine is assigned Tamanu central, and none where the environment holds one (verifies spec: ANSRUN)
- [x] Taking a lease another operator holds is refused naming who, and taking over is audited (verifies spec: ANSRUN)
- [x] Upgrading production is refused without an open plan (verifies spec: ANSRUN)
- [x] The dialog shows the command only once the lease is taken, leaves the checkout out of the copied text, and shows the time left (verifies spec: ANSRUN)
- [x] The run buttons offer Tamanu's playbooks only where a Tamanu application is held, and Run… needs a catalogue (verifies spec: ANSRUN)

## Maintenance windows for runs

- [x] A window over several machines of one environment covers each as a window over that machine alone (verifies spec: MNT)
- [x] A machine is named by at most one open window, alone or among others (verifies spec: MNT)
- [x] A set left with one machine is that machine's window (verifies spec: MNT)
- [x] Machines a set stops covering serve the settle period (verifies spec: MNT)
- [x] A run's first read declares a window over what it acts on, for the lease's holder (verifies spec: ANSRUN)
- [x] Nothing new is declared where the holder's own windows already cover it all (verifies spec: ANSRUN)
- [x] The window ends when the lease is released or expires, and passes to the new holder on takeover (verifies spec: ANSRUN, MNT)
- [x] Holding the window gives it an expected end and it then ends like any other (verifies spec: MNT)
- [x] A window a lease is served against cannot be moved while the lease is held (verifies spec: MNT)
