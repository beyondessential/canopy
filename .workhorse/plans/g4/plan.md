# Two concurrent open incidents on one group

## Diagnosis

The two open incidents on Fiji Prime are on different targets, so the per-target unique indexes held: one on the group's production environment, one on the group itself.

Every Postgres application in the group carries no rank and runs on a box that also hosts a production Tamanu application.
When the SRH Central box went dark, its machine reachability and its Tamanu application's reachability joined the production incident, while its Postgres application's reachability opened a second incident on the group target.
That follows INC as written ("an application carrying none is in no environment … their issues belong to the group itself", `member_target` in `crates/database/src/issues.rs`), but it splits one box's failure across two incidents, against INC's own "a machine's failure is not split across the applications it hosts".

The unranked rule is also implemented four different ways:

- Incidents: always the group target, even in an all-unranked group.
- Environment listing (`ServerGroup::environments_inner`): no environment, unless the whole group is unranked, then production.
- Upgrade candidates and reporting schemas (`ServerGroup::environment_of`, `migration_tests.rs`, `upgrade_plans.rs`): the group's headline environment. GRP's "A group's headline rank" section still says this.
- Maintenance windows (`environment_of_machines`): through the machine's derived rank, which is the rule below.

## Decision: one rule for an application's environment

An application's environment is, in order:

1. its own rank;
2. failing that, its machine's derived rank (the highest rank among the applications on the box);
3. failing that, production where its group has nothing ranked at all (GRP's one environment);
4. otherwise none: it belongs to the group itself.

A machine's environment is steps 2 to 4.
The rule is stated once in GRP and referenced from INC, MNT, RST (candidate versions) and wherever else an unranked application's environment is read.

Consequences:

- A sidecar (Postgres and similar) on a ranked box joins that box's environment's incident, maintenance, and plan.
- In an all-unranked group, member failures open on the production environment, and only group checks (backups) target the group.
- An unranked application on a box hosting nothing ranked, in a group with ranked applications elsewhere, has no upgrade candidate. Today it inherits the headline environment's.
- A rank change on one application re-evaluates the issues of every unranked application on its box, as well as its own and its machine's.
- A group's first ranked application, or its last one going, moves its bare-box members between production and the group target.
- Existing open incidents heal on deploy through the monitor's startup `reconcile_open_incidents`.

## Steps

- [ ] Specs: state the rule in GRP "Environments" and "A group's headline rank"; update INC "Targets" and the rank-change re-evaluation; check MNT, RST, UPG for wording that leans on the old rules
- [ ] One derivation in the database crate (application → `Option<ServerRank>`, batched), replacing `environment_of` and the unranked branches of `environments_inner`, `member_target` and `candidates_for`
- [ ] `ScopeTargets::load` loads machine ranks for application scopes, and group ranked-ness for the all-unranked fallback
- [ ] Re-evaluation: rank change and machine move enqueue co-hosted unranked applications; a group gaining or losing its only ranked application re-evaluates its bare-box members
- [ ] Tests: unranked sidecar on a production box joins the production incident (the Fiji Prime shape); bare box in a ranked group targets the group; all-unranked group opens on production; rank change on the host app moves the sidecar's issues
