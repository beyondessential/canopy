# Test cases: a check reported under an application is its type's

## Namespace derivation

- [x] A structured source's check filed at an application is that type's namespace, and at a machine the machine's, whatever its name (verifies spec: CHK)
- [x] A reserved source is flat wherever it files (verifies spec: CHK)

## Ingest

- [x] A split push carrying `disk_free` under the machine and under an application catalogues two entries, the box's and `tamanu-central.disk_free` (verifies spec: CHK, STA)
- [x] Raising the box's ceiling or silencing the box's check changes the machine's answer on a split push and not the application's (verifies spec: CHK)
- [x] A unified push's machine-subject name still files at the machine and catalogues once, however many types report it (verifies spec: STA)

## Readers

- [x] The device-facing ceiling map and silence set are per target: the box's `memory` and an application's `memory` are told apart (verifies spec: CHK)
- [x] The catalogue page lists an application's own `disk_free` as its type's entry beside the box's (verifies spec: CHK)
- [x] The point-in-time view of a split push presents a machine-named check reported under the application as the application's, graded through the application's entry (verifies spec: STA)
- [x] The point-in-time view still reads an application's report as a split push after the box's newer pushes stop describing that application (verifies spec: STA)
- [x] A unified push's answer takes a box check's silences from the box's group when the application on it is in another group (verifies spec: STA)

## Migration

- [x] An application-scoped silence in the machine namespace moves to the application's machine, keeping who set it
- [x] A box already holding the silence keeps its own, and two workloads' copies become the earliest one
