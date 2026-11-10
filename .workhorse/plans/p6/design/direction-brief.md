# Shared brief for the three direction mockups

Round 1 of the design loop. Three agents each build one direction, independently, without seeing the others. This file holds everything common to all three; each agent's own prompt names its direction.

## What the work is

Canopy has three ways a caller proves who it is and no single place to say what any of them may do. Read `inventory.md` beside this file first: it is the factual account of what exists, and nothing in a mockup may contradict it.

The scope is settled: **the full unified control plane** over human and machine identity, with least-privilege permissions. Design for all of it; what is realistic to build gets judged afterwards. The three directions are three structurally different shapes of that same control plane, each organised around a different primary entity (the principal, the grant, the role). They are not three sizes of it.

## References

Take the model from these, not their screens, unless a note says otherwise.

- **Tailscale grants** (the policy file's `grants` section). A grant names sources (users, groups, tags, autogroups), destinations, and capabilities carried as named keys with structured parameters. Additive, allow-only, all of it data. Its UI is a policy-file editor: do not copy it.
- **Tamanu's permissions.** A permission is a verb on a noun, optionally narrowed to one object id, attached to a role. A user holds several roles. Each noun declares which verbs it accepts, and that one table drives validation, the editor and import. Scoping to an object is allowed only for an explicit short list of nouns, which keeps the editor tractable. Allow-only: revoking means removing the row. The editor is a matrix: roles as columns, nouns grouped in sections, a checkbox per verb.
- **Firewall rule tables.** Ordered rows, each readable as a sentence (from this source, to this destination, for this service, allow or deny), with hit counts and when it last matched. Expressive while staying human-parseable. iptables syntax is the cautionary tale.
- **Tailscale's and crates.io's trust credentials (trusted publishing).** A machine is trusted by a federated identity claim instead of a long-lived secret: an OIDC issuer plus the claims it must present (for crates.io, a GitHub repository, workflow file and optional environment). Configured per target, it shows plainly what the claim must match, and nothing secret is ever shown.
- **AWS IAM, for the API and the model only (its UI is the cautionary tale).** Principals, policies made of effect, action, resource and condition, an explicit deny that overrides any allow, roles that trusted principals assume, and a simulator that answers "can this principal do this action on this resource, and why".

## Fleet machines stay out of the way

Every enrolled machine in every group authenticates to Canopy all the time. That is the bulk of all principals and all activity, and it is not what an operator opens this section to manage.

- **Default views and default filters leave fleet-machine authentication out.** The headline lists, counts and activity are about operators, agents, tokens, the relay, releasers, restore consumers and anything newly arrived.
- **It is never absent from the model.** Fleet machines are covered by the same permission model as everything else, appear in any answer to "who can do X", and are one step away: a count the operator can open, or a filter they can switch on. Show that drill-down.
- A fleet machine that is *not* behaving like one (a new key, a permission beyond the fleet default, a failed authentication) is not noise and does surface by default.

## Fidelity is the point

These mockups stand in for screens in a shipped operator UI. A critic compares them against the app's own screenshots. A mockup that invents its own visual language loses on contact, however good the idea.

**Look at the real screens before you write any markup.** `shots/current/` beside this file holds full-page captures at 1440px of every surface this work touches: `admins.png`, `mcp-access.png`, `identity-provider.png`, `certificate-authority.png`, `audit-log.png`, `restore-consumers.png`, `devices-all.png`, `devices-search.png`, `device-detail.png`. View them at full size. Then read the React that produced them: `private-web/src/routes/Admins.tsx`, `McpTokens.tsx`, `IdentityProvider.tsx`, and `private-web/src/components/AppSidebar.tsx` for the navigation.

Carry the chrome through unchanged: the top bar with the Canopy mark and the safety-mode control on the right, the left sidebar with its sections and the expanded Settings group. Your screen is what changes; everything around it is as captured.

## Visual language

Material UI, as the app ships it. From the captures: outlined `Paper` panels with a heading above the content, dense tables with a plain header row, inline add-forms sitting below the table they add to, `Chip` for small enumerated values such as grants, `Alert` for an unconfigured or error state, monospace for keys, hashes and identifiers. Default MUI light palette and spacing. Do not introduce a new accent colour, a gradient, an icon set the app does not use, or rounded-card styling the app does not have.

## Copy

- Australian and New Zealand English (authorise, organisation, finalise).
- **No em-dashes anywhere.**
- Microcopy stays micro. No sentence that tells the operator what the system will do behind the scenes, and no reassurance. If behaviour matters, show it as state when it happens rather than promising it beside a control.
- Every element earns its place. Before adding a label, badge, icon, divider or hint, ask whether it lets the reader do or understand something they could not already. If not, cut it.
- Use the vocabulary the inventory uses. Say group, environment and rank, never "deployment". Say Ansible, not "inventory", for the configuration feature.

## Real data, and the hard cases

Populate every screen with plausible fleet data, not `foo` and `lorem`. Use the principals the inventory describes: tailnet operators by email, machines by node name (`tamanu-prod-db-1`, `pgro-restore-operator`, `backup-runner-syd`), MCP tokens by purpose (`incident-triage-bot`, `grafana-sidecar`), the relay, a releaser.

Your direction must show, not dodge, these:

1. **An admin granted by tailnet policy, not by Canopy.** Canopy can read it but cannot edit it. The screen has to make that legible without making it look broken.
2. **A principal with no permissions at all**, newly arrived and not yet authorised.
3. **A long list.** At least fifteen rows, so density and scanning are tested rather than imagined.
4. **A secret shown exactly once** at creation, never again.
5. **Something revoked or disabled** sitting alongside live entries.
6. **A permission that is scoped, not global**: restricted to one group, one environment, or one kind of resource.
7. **The fleet, held back.** Fleet machines out of the default view, with the count and the one-step drill-down visible, and one fleet machine that does surface because it is behaving unlike the fleet.
8. **A trust credential.** A releaser trusted by a federated claim (a GitHub Actions workflow on a named repository and environment), not by a key Canopy minted.
9. **Taking access away in a hurry.** Decide whether your model has an explicit deny. If it does, show a deny overriding an allow. If it does not, show how an operator cuts a principal off at once, and say in the write-up why you chose that.
10. **"Who can do this?"** An operator asking which principals can perform one action on one resource (for example, consume restores for one group), and getting an answer that names each principal and the grant or role responsible.

## What to produce

Standalone HTML with inline CSS, self-contained, opening with `<!-- spec: platform/auth-control-plane.md -->` and carrying a `<title>` naming the screen and its state in sentence case. No JavaScript, no external fetches, no web fonts.

Write **three to five files** into `.workhorse/design/mockups/p6/`, prefixed with your direction letter, covering the main screen and whichever supporting states your direction needs (a detail view, the creation moment, the "who can do this" answer). Name them `<letter>-<slug>.html`, for example `g-grants.html`.

Then write `.workhorse/plans/p6/design/direction-<letter>.md`: what the direction is, what it is betting on, what it gives up, which hard cases each file shows, which reference each main idea came from, and where you were unsure. Keep it under 450 words.

Before you finish, render every file with headless Chromium at 1440px wide and look at each render at full size. Fix anything that overflows, clips or misaligns. Say in the write-up which files you viewed.

## Scope discipline

Do not design features the card has not asked for. No dashboards, no analytics, no activity charts, no onboarding wizards. This is a control plane for identity and permission, nothing else.

Work only inside your own files. Do not edit the shared files, the React source, or another direction's output.
