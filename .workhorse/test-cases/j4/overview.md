# Retarget maintenance from the declare dialog

Scenarios for choosing what a declaration covers, moving a window, and the page-head control.

## Choosing what to cover

- [x] From a group, the choices nest group over each environment over its machines over their applications, production first (verifies spec: MNT)
- [x] A group's pending machines are listed under it, apart from its environments (verifies spec: MNT)
- [x] From an application, the choices are its machine, the machine's environment, and its group, and nothing beside them (verifies spec: MNT)
- [x] A machine in no group offers its applications alone; a pending machine offers its group with no environment between (verifies spec: MNT)
- [x] Declaring from an incident starts at the incident's environment (verifies spec: MNT)
- [x] With a group check failing in the headline environment's incident, only the group carries no "not all failing checks" mark, and choosing it clears the warning (verifies spec: MNT)
- [x] Retargeting onto a grain with its own window turns the dialog into an amendment, shows that window's end and note, and changes only what the operator changed (verifies spec: MNT)
- [x] An end or note entered before retargeting onto another operator's window is dropped, so that window keeps its own (verifies spec: MNT)
- [x] From an incident, a failure no window can be declared over marks every choice as not covering all failures (verifies spec: MNT)
- [x] The upgrade plan's declaration cannot be retargeted, and the window it opens records the plan (verifies spec: MNT, UPG)
- [x] The configuration run's "Declare the work" cannot be retargeted, and declares over the environment it is shown for (verifies spec: MNT, INV)

## Moving a window

- [x] Opened over a target with its own window, the dialog amends it and choosing another grain moves it, keeping the same window (verifies spec: MNT)
- [x] The fleet maintenance view moves a window (verifies spec: MNT)
- [x] A move suspends the new target at once, and leaves the old one settling at the grain it left until the settle period passes (verifies spec: MNT)
- [x] A failure on what a move left uncovered alerts once its settle period has passed, not before (verifies spec: MNT)
- [x] Moving over a target takes its issues out of their incident (verifies spec: MNT)
- [x] A move is refused off the line of descent, onto a target with its own window, and for an ended window (verifies spec: MNT)
- [x] A window declared from an upgrade plan cannot move; one over the environment the plan did not open can (verifies spec: MNT)
- [x] A window a run lease is served against cannot move until the lease is released; another operator's lease does not hold it (verifies spec: MNT, INV)
- [x] A moved window is in both targets' histories, each over the span it covered there (verifies spec: MNT)
- [x] Neither a move nor the end of its settle period notifies (verifies spec: MNT)
- [x] A window past its expected end that the sweep has not yet ended can be extended, and cannot be moved without extending it (verifies spec: MNT)
- [x] A declaration offered over a target whose own window cannot move shows no picker, whether or not the caller named that window (verifies spec: MNT)
- [x] The group's history shows a moved-off span as "moved to …" (verifies spec: MNT)

## Page-head control

- [x] The machine page's "Maintenance" declares over the machine, then amends and lifts that window (verifies spec: MNT)
- [x] The application page's "Maintenance" declares over the application (verifies spec: MNT)
- [x] The group page's "Maintenance" opens as an amendment of the group's own window, with Lift (verifies spec: MNT)
