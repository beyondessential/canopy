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

## Decision: a box's applications share one rank

An earlier draft of this card gave each group a derived **other** environment for unranked members and group-scoped checks.
It is replaced by making unranked members in an environment impossible in the first place.

- A box's applications share one rank, and a machine is in the environment its applications' rank names.
  Ranking any application on a box ranks every application on it, so a box never carries two ranks.
- An application arriving on a box that carries a rank takes it.
  An application arriving on a box with nothing ranked is **pending**: it is in no environment until an operator ranks it, and ranking it ranks the box.
- A rank can be changed but not cleared, so the only way a group loses its last ranked application is to lose its last application.
- A cluster-hosted application has no box, so it ranks on its own; it is pending until ranked.
- Group-scoped issues (backups) belong to the group's headline environment.
  Whether a group check should file against a narrower environment is #H4.
  A window declared from such an incident covers the headline environment's machines and not the group's own checks; letting the declare dialog retarget the grain is #J4.

So every environment holds ranked members only, every group incident has an environment, and the group-itself incident target is gone.

Live data on 2026-10-06 (read-only, through the private API): 105 live boxes, none carrying two ranks; 68 unranked applications (67 `postgres`, 1 `tamanu-central`), every one with a ranked sibling, so all of them inherit on migration and none go pending.
The one all-unranked group (a demo) was ranked by hand the same day.

Consequences:

- A sidecar (Postgres and similar) on a ranked box is ranked with it, so it joins that box's environment's incidents, maintenance windows, plans and upgrade candidates through the ordinary rank rule.
- A pending application's or box's issues belong to no target, as for a machine in no group, and the group presents them as awaiting a rank beside "awaiting check-in".
- A group with nothing ranked yet (new, or holding only pending boxes) has no environments and no headline environment, so its group-scoped issues belong to no target until something in it is ranked.
- A rank change on a box moves the issues of every application on it and of its machine together; ranking a pending box brings its issues into incident membership.
- The production-upgrade lease guard (INV "Planned upgrades") is unchanged: a group with nothing ranked has no environment to lease.
- Existing group-target incidents move to the headline environment in the migration; open ones heal on deploy through the monitor's startup `reconcile_open_incidents`.

## Tech notes

- Migration: set each unranked application's rank to its box's, then hold the rule in the schema.
  An exclusion constraint over live applications, `machine_id WITH =` and `COALESCE(rank, '') WITH <>` (needs `btree_gist`), refuses both two ranks on one box and an unranked application beside a ranked one.
  Arrival code sets the sibling's rank on insert; the rank update writes every live application on the box in one statement.
- No environment key type is needed: `ServerRank` names every environment. `IncidentTarget` becomes `Environment(group, rank)` and `Global`.
- Storage: `incidents.rank` becomes required wherever `server_group_id` is set, and its NULL rows move to the group's headline rank; `incidents_open_by_group` goes. `maintenance_windows.rank` keeps NULL for a group-wide window. `upgrade_plans`, `inventory_leases` and `inventory_variables` are unaffected.
- The rank-clearing path goes from the private API and the SPA (`update` on applications, the machine form).

## Steps

- [x] Specs: rework this branch's GRP, INC, CHK, FLT, MNT, UPG, RST and MCP edits from the other-environment model to the shared-rank model; FLT "Environments" and "Editing" (rank per box, not clearable), APP "Billing attribution" (a box's stage is its rank), K8S (a cluster application ranks on its own; namespace checks follow group checks), INC re-evaluation (box rank change, ranking a pending box)
- [x] Migration `shared_rank`: canonicalise rank spellings, give each live application its box's highest rank, hold it with a deferrable exclusion constraint, move group-target incidents to the headline rank (closing one that has nowhere to go, or whose headline already has an open incident), `incidents.rank` required with a group. The exclusion constraint is deferrable so a whole-box rank write is checked at the end of the statement, which makes `INSERT INTO applications … ON CONFLICT` need an explicit target
- [x] Arrival: `Application::adopt` takes the rank the box's other applications share, and `restore` returns an application to the box's current rank, pending where none is ranked. The cluster relay creates no applications yet, so a namespace's shared rank has no code to carry it
- [x] Rank change: `Machine::set_rank` writes the whole box in one statement and refuses a box with no application; `Application::set_rank` delegates to it (a cluster application ranks alone); the private API refuses `rank: null` on both the application and machine updates; re-evaluation covers the box and, when the group's headline moved, the group's own checks. Archiving, restoring, and moving a box between groups re-evaluate the group's own checks when its headline moved
- [x] One derivation: `environment_of`, `UNRANKED_ENVIRONMENT`, and the unranked branches of `environments_inner`, `candidates_for`, `upgrade_plans.rs`, `environment_of_machines`, and inventory are gone; a pending member maps to no target
- [x] `incident_target`: the group-scope arm targets the headline environment (none while nothing is ranked); `IncidentTarget::Group` and `format_group_label`'s group-itself case are gone
- [x] SPA: rank edited once on the machine form (no empty choice, "Not ranked yet" while pending, disabled with nothing on the box); the application form offers none; pending boxes listed under "awaiting a rank" and silent ones under "awaiting check-in" after the environments; the status dot strip's trailing row is "pending"; `GroupInventorySection` offers only ranks
- [x] Tests: database (`shared_rank.rs`, `shared_rank_migration.rs`, rewritten `incident_environment.rs`, `maintenance_windows.rs`, `migration_test_candidates.rs`, `upgrade_plans.rs`), private-server (`update_server.rs`, `upgrade_plans.rs`, `application_types.rs`, `inventory.rs`, `issues.rs`, `mcp.rs`), and Playwright (`group-tree`, `machines`, `inventory`, `incident-environment`). `incident_environment_migration.rs` is removed: it replayed a long-shipped migration against the group-itself target this card deletes
