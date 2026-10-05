# Two concurrent open incidents on one group

## Diagnosis

The two open incidents on Fiji Prime are on different targets, so the per-target unique indexes held: one on the group's production environment, one on the group itself.

Every Postgres application in the group carries no rank and runs on a box that also hosts a production Tamanu application.
When the SRH Central box went dark, its machine reachability and its Tamanu application's reachability joined the production incident, while its Postgres application's reachability opened a second incident on the group target.
That follows INC as written ("an application carrying none is in no environment … their issues belong to the group itself", `member_target` in `crates/database/src/issues.rs`), but it splits one box's failure across two incidents, against INC's own "a machine's failure is not split across the applications it hosts".
Both incidents were also announced under the same name, since a notification names a production environment and the group's own incident alike by the group's name alone (`format_group_label`).

The unranked rule is also implemented four different ways:

- Incidents: always the group target, even in an all-unranked group.
- Environment listing (`ServerGroup::environments_inner`): no environment, unless the whole group is unranked, then production.
- Upgrade candidates and reporting schemas (`ServerGroup::environment_of`, `migration_tests.rs`, `upgrade_plans.rs`): the group's headline environment. GRP's "A group's headline rank" section still says this.
- Maintenance windows (`environment_of_machines`): through the machine's derived rank.

## Decision: every group member is in an environment

Each group has, beside its ranked environments, an **other** environment: a catch-all Canopy derives, not a rank an operator can set.
It reads as the group's name with "other" after it ("Fiji Prime other"), and as "other" on the group's own surface.

An application's environment is, in order:

1. its own rank;
2. failing that, its machine's derived rank (the highest rank among the applications on the box);
3. otherwise the group's other environment.

A machine's environment is steps 2 and 3.
Group-scoped checks (backups) belong to the other environment too, so every group incident has an environment and the group-itself incident target is gone.
An all-unranked group's one environment is its other environment, holding its upgrade plans, maintenance windows and incidents.
The rule is stated once in GRP and referenced from INC, MNT, UPG, RST (candidate versions) and wherever else an application's environment is read.

Consequences:

- A sidecar (Postgres and similar) on a ranked box joins that box's environment's incident, maintenance, and plan.
- An unranked application on a box hosting nothing ranked is in the other environment for incidents, maintenance, plans and upgrade candidates, rather than inheriting the headline environment's candidate.
- A rank change on one application re-evaluates the issues of every unranked application on its box, as well as its own and its machine's.
- A group's maintenance window keeps covering everything in the group; the other environment's window is a separate, narrower target beside it.
- Existing open incidents heal on deploy through the monitor's startup `reconcile_open_incidents`; the group-target incidents become other-environment incidents in the same migration.

Whether group-scoped checks should instead file against the environment they concern is a separate card (H4).

## Tech notes

- Storage: `incidents.rank` and `maintenance_windows.rank` use NULL for the group itself, while `upgrade_plans.rank` and `inventory_leases.rank` are NOT NULL with a rank CHECK.
  The other environment needs one representation across all four. Maintenance still needs NULL for the group-wide window, so a stored `'other'` value (added to each CHECK) is the likely shape, with incidents' NULL migrated to it and `incidents_open_by_group` folded into `incidents_open_by_environment`.
- Rust: an environment key type (`Ranked(ServerRank)` or `Other`) distinct from `ServerRank`, which stays an application's settable rank. `IncidentTarget` becomes `Environment(group, key)` and `Global`.

## Steps

- [ ] Specs: GRP "Environments", "A group's headline rank" and "Naming" define the other environment and the derivation; INC "Targets", "Notification" and the rank-change re-evaluation; MNT, UPG, RST wording that leans on the old rules
- [ ] Migration: `'other'` in the rank CHECKs of incidents, maintenance windows, upgrade plans and inventory leases; incidents' NULL rank moves to `'other'`; one open-incident index per environment
- [ ] One derivation in the database crate (application or machine → environment key, batched), replacing `environment_of` and the unranked branches of `environments_inner`, `member_target`, `candidates_for` and `upgrade_plans.rs`
- [ ] `ScopeTargets::load` loads machine ranks for application scopes; the group-scope arm of `incident_target` targets the other environment
- [ ] Naming: `environment_name` and `format_group_label` read the other environment as "{group} other"; the SPA reads it as "other" on the group's surface
- [ ] Re-evaluation: rank change and machine move enqueue co-hosted unranked applications
- [ ] Tests: unranked sidecar on a production box joins the production incident (the Fiji Prime shape); bare box in a ranked group and a group check both open on the other environment; all-unranked group's plans, windows and incidents sit on its other environment; rank change on the host app moves the sidecar's issues
