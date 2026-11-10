# Shared brief for the three direction mockups

Round 1 of the design loop. Three agents each build one direction, independently, without seeing the others. This file holds everything common to all three; each agent's own prompt names its direction.

## What the work is

Canopy has three ways a caller proves who it is and no single place to say what any of them may do. Read `inventory.md` beside this file first: it is the factual account of what exists, and nothing in a mockup may contradict it.

The card asks for a control plane over human and machine identity. The three directions sit at three different scopes, so the critique that picks one also answers how far the work should go.

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

## What to produce

Standalone HTML with inline CSS, self-contained, opening with `<!-- spec: platform/auth-control-plane.md -->` and carrying a `<title>` naming the screen and its state in sentence case. No JavaScript, no external fetches, no web fonts.

Write **two to four files** into `.workhorse/design/mockups/p6/`, prefixed with your direction letter, covering the main screen and whichever supporting states your direction needs (a detail view, the creation moment, the empty state). Name them `<letter>-<slug>.html`, for example `a-tag-rules.html`.

Then write `.workhorse/plans/p6/design/direction-<letter>.md`: what the direction is, what it is betting on, what it gives up, which hard cases each file shows, and where you were unsure. Keep it under 400 words.

## Scope discipline

Do not design features the card has not asked for. No dashboards, no analytics, no activity charts, no onboarding wizards. This is a control plane for identity and permission, nothing else.

Work only inside your own files. Do not edit the shared files, the React source, or another direction's output.
