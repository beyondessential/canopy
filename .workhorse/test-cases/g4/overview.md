# One box's failure is one incident

Scenarios for the shared-rank model: a box's applications share one rank, an application arriving where nothing is ranked is pending, and a group's own checks join its headline environment's incident.
Most are automated at the database layer (`crates/database/tests/it/`), with the operator surfaces in Playwright (`private-web/e2e/`).

## Shared rank

- [ ] An application reported for the first time on a machine whose applications are production is created at production (verifies spec: GRP, FLT)
- [ ] An application reported for the first time on a machine with nothing ranked is created unranked, and the machine and application both read as pending (verifies spec: GRP)
- [ ] Ranking one application on a box with three applications sets all three to that rank in one save (verifies spec: GRP, FLT)
- [ ] Changing a ranked box from test to demo moves every application on it to demo (verifies spec: GRP)
- [ ] Clearing the rank of a ranked application through the private API is refused, and its rank and its siblings' are unchanged (verifies spec: GRP, FLT)
- [ ] Inserting a live application at a rank different from its siblings' fails on the database constraint (verifies spec: GRP)
- [ ] Inserting an unranked live application beside a ranked sibling fails on the database constraint (verifies spec: GRP)
- [ ] An archived application at another rank on the same machine does not trip the constraint (verifies spec: GRP)
- [ ] Restoring an archived test application onto a box now ranked production brings it back at production, and onto a box with nothing ranked brings it back pending (verifies spec: GRP)
- [ ] A machine with no applications yet is pending, and becomes ranked only once an application on it is ranked (verifies spec: FLT)
- [ ] A group whose machines are all pending has no environments and no headline rank, and is left out of the fleet listing's rank buckets (verifies spec: GRP)

## Incidents

- [ ] A production box hosting a Tamanu central and a Postgres goes unreachable: its machine reachability, the central's reachability and Postgres's reachability all join one production incident, and no second incident opens (verifies spec: INC)
- [ ] A failing backup check on a group whose highest rank is production joins the production incident rather than opening one of its own (verifies spec: INC, GRP)
- [ ] A failing backup check on a group whose highest rank is demo joins the demo incident (verifies spec: INC)
- [ ] A failing check on a pending application opens no incident and joins none (verifies spec: INC)
- [ ] A failing machine check on a pending box opens no incident (verifies spec: INC)
- [ ] A failing backup check on a group with nothing ranked opens no incident (verifies spec: INC)
- [ ] Ranking a pending box whose application is failing opens an incident on the new environment after the membership delay (verifies spec: INC)
- [ ] Moving a box from test to production while its application is failing moves the issue into the production incident and closes the test incident it leaves with no failure (verifies spec: INC)
- [ ] Ranking the group's only production box down to clone moves an open backup issue from the production incident to the clone incident (verifies spec: INC)
- [ ] A production incident's notification names the group alone, and a clone incident's names the group with "clone" after it (verifies spec: INC)
- [ ] A group's incidents list, detail page and Slack notices never name a group-level target with no environment (verifies spec: INC, CHK)

## Maintenance windows

- [ ] A window over a group's production covers the Postgres on a production box, so its failure joins no incident while the window holds (verifies spec: MNT)
- [ ] A window over a group's production leaves a failing backup check in the production incident (verifies spec: MNT)
- [ ] A group's window suspends its backup check (verifies spec: MNT)
- [ ] A pending box is covered by its group's window and by a window over itself, and by no environment's window (verifies spec: MNT, GRP)

## Plans and testing

- [ ] A plan can be recorded for a group's production, and its environment's version reads from the production central (verifies spec: UPG, APP)
- [ ] Recording a plan for a group with nothing ranked is refused (verifies spec: UPG)
- [ ] A Postgres on a production box takes the production plan's target as its migration-test candidate (verifies spec: RST)
- [ ] A pending application has no migration-test candidate while its group has a production plan open (verifies spec: RST)
- [ ] The planned upgrades dashboard lists the headline environment of every group with something ranked, and no group with nothing ranked (verifies spec: UPG)

## Billing

- [ ] A production box's effective labels carry stage production, and every application on it carries the same stage (verifies spec: APP)
- [ ] A pending box carries no stage label (verifies spec: APP)

## Kubernetes

- [ ] A second application reported in a demo namespace is created at demo (verifies spec: K8S, GRP)
- [ ] The first application reported in a fresh namespace is pending, and ranking it ranks every later application in that namespace (verifies spec: K8S, GRP)
- [ ] A failing namespace check joins the group's headline environment's incident (verifies spec: K8S, INC)

## Migration

- [ ] Applying the migration to a box holding a ranked Tamanu central and an unranked Postgres sets the Postgres to the central's rank (verifies spec: GRP)
- [ ] An open group-target incident migrates to an incident on the group's headline environment, keeping its timeline, and the monitor's startup reconcile leaves it open while it still has a failure (verifies spec: INC)
- [ ] An open group-target incident on a group with nothing ranked closes in the migration (verifies spec: INC)
- [ ] The migration runs cleanly against a copy of the live database, and the constraint holds afterwards

## Operator interface

- [ ] The machine edit form offers Rank on the machine's own section, and no application section offers a rank (verifies spec: FLT)
- [ ] A ranked machine's Rank menu lists the five ranks and no empty choice (verifies spec: FLT)
- [ ] A pending machine's Rank field reads "Not ranked yet", and saving it at demo shows every application on the box at demo
- [ ] The group tree lists a pending box under "awaiting a rank", after every rank's environment, with its applications' health dots shown as for any other box (verifies spec: FLT)
- [ ] A box with nothing on it still reads as awaiting check-in, not as awaiting a rank (verifies spec: FLT)
- [ ] The group page's environment picker for inventory offers only the group's ranks, never an unranked or pending bucket
- [ ] The group's status card marks the production row for a backup incident, not the card alone (verifies spec: CHK)

## Tests asserting the replaced rules

These existing tests assert the group-itself target or the unranked fallbacks, so they are rewritten or removed as their behaviour goes:

- `incident_environment.rs`: `a_groups_own_check_targets_the_group_beside_its_environments`, `an_unranked_application_targets_the_group`, `a_box_hosting_nothing_ranked_targets_the_group`, `a_group_window_suspends_an_unranked_members_incident`, `a_box_takes_the_rank_of_its_highest_ranked_workload`
- `maintenance_windows.rs`: `an_environment_window_does_not_suspend_an_unranked_member`
- `migration_test_candidates.rs`: `an_unranked_group_takes_its_plan_s_environment`
- `incident-environment.spec.ts`: "a group's own incident marks no environment row" and "a group's own incident is presented beside its environments'"
