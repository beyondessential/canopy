# Rank input on the machine edit form

Two faults on the Rank field of the machine edit form.

The label sat on top of the "Not ranked yet" placeholder, because MUI only floats a label over a filled value and the empty rank does not count. Fixed by shrinking the label explicitly.

A box with no applications could not be ranked at all: the select was disabled and the backend refused, because a box held its rank only through the applications on it. The fix gives the box a rank of its own.

## Design

The machine row carries the box's rank (`machines.rank`), and it is the box's rank everywhere: the environment it serves, its incident target, its billing stage, the window coverage it falls under.
Every live application on the box carries the same rank, which the existing exclusion constraint on `applications` keeps to one value per box.
`Machine::set_rank` writes the box and re-evaluates incident membership; it works on an empty box too.
Triggers keep the two columns together whichever is written: an application inserted unranked onto a ranked box takes its rank, ranking a live application ranks its box, and ranking a box ranks its live applications. That follows the group-propagation precedent, and covers raw SQL in tests and seeds.

- Arrival takes the box's rank, so applications arriving on a pre-ranked box are ranked rather than pending.
- Restore takes the box's rank where it has one. A box with no rank and no live application takes the restored application's own rank, so restoring a fully archived box's applications one by one keeps the rank they shared.
- The migration backfills `machines.rank` from the live applications' shared rank. A box whose applications are all archived starts unranked, which restore then recovers as above.
- The group tree places a ranked box with nothing on it under its environment, its row still reading "Awaiting check-in."
- A group's environments and headline rank stay composed of applications: a box carrying nothing runs no workload, so it adds no environment to its group, but its own checks belong to the environment its rank names.

## Build steps

- [x] Float the Rank label above the placeholder, with e2e coverage
- [x] Specs: FLT environments and editing, GRP environments, APP billing stage
- [x] Migration adding `machines.rank`, backfilled, constrained to the canonical ranks
- [x] `Machine` model and `Machine::rank`/`ranks` read the stored rank
- [x] `Machine::set_rank` writes the box, empty box allowed; triggers carry it to the applications
- [x] `Application::adopt` and `restore` take the box's rank
- [x] Maintenance window expansion reads the box's rank
- [x] Machine billing stage reads the box's rank
- [x] Update endpoint drops the empty-box refusal; regenerate the private OpenAPI types
- [x] Edit form, machine page and group tree read the machine's rank
- [ ] Tests: database shared-rank, private-server update, e2e empty box
- [x] AGENTS.md shared-rank rule
