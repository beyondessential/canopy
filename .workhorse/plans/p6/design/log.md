# Design loop — P6, auth control plane

Rigour: Standard. Round cap 6. Stop rule: two clean rounds, the second by fresh eyes, no criticals and few suggestions.

## Where the run is

Step 2, direction. The card has no specs, no plan and no commits, so there is no build to polish: the run starts at direction, not at polish. Step 3 waits on an implementation.

## Step 0 — does the card need design

Yes. The card's third goal is a control plane over human and machine identity with least-privilege permissions, and its first is choosing at runtime which tagged devices may call which parts of the Canopy API. Neither has a pattern in the app: Settings › Access is five separate pages (Admins, MCP access, Certificate authority, Identity provider, Audit log), each covering one credential kind, and none of them grants a scoped permission to a principal. A new surface, a new layout and controls with no precedent — design.

## The method as run here

The Workhorse version in this repo ships `card-context`, `spec-format`, `test-quality` and `working-doc-format` under `.agents/docs/`, but not the `design-judgement`, `running-loops` or `browser-rig` documents the skill cites. The method is therefore taken from the skill body itself, and the rig built from what the repo has.

## The rig

The Playwright e2e fixture is the rig: it spawns a private-server and Vite pair against a throwaway Postgres, so a capture needs no running dev server. The capture harness lives at `capture/` beside this log rather than in `private-web/e2e/`, so it is never mistaken for a test, and writes to `shots/current/`.

    cd private-web && env -u PORT npx playwright test \
      --config ../.workhorse/plans/p6/design/capture/playwright.config.ts

`env -u PORT` is required under a Workhorse local agent: the fixture's clap parsing conflicts with an inherited `PORT`.

## Step 1 — references

The app's own shipped surfaces are the consistency reference, captured in `shots/current/`: the five Access pages, restore consumers, the device listing, search and detail. There is no `.workhorse/design/` in this repository, so there is no design system document or snapshot to start from; the MUI composition in `private-web/src/routes/` is the baseline.

A product reference has been asked for in prose and not yet answered. The directions are briefed to the app's own language in the meantime, so an answer can be folded in without rework.

## Step 2 — three directions

The card asks whether the work can reach rung 1, something between, or the unified control plane. Rather than park the run on that question, the three directions span the ladder, so the side-by-side critique answers the scope question with evidence instead of a guess:

- **A, the rule table.** One new page, a flat table of matcher to API scope, in the shape of the existing Admins page. The five Access pages are untouched.
- **B, the grants layer.** The credential pages stay; a shared named-capability concept threads through all of them, with one grant-centric index.
- **C, one principal list.** The Access section is replaced by a single list of every principal, human and machine, each with its credentials and permissions on one detail page.

Each was briefed from `direction-brief.md` and `inventory.md`, with six hard cases every direction must show: a policy-granted admin Canopy cannot edit, a principal with no permissions, a list long enough to test density, a secret shown once, something revoked beside live entries, and a permission scoped to a group rather than global.

## In flight

Three direction agents, writing into `.workhorse/design/mockups/p6/` and `direction-{a,b,c}.md`.

## Next

- View all three at full size, critique side by side with a fresh critic, choose one and record why in the taste ledger.
- Refine the chosen mockup through a few UI-loop iterations before anything is built.

## Open questions

- Whether a product does this well and should be the reference.
- Which rung to build to. The critique is meant to answer this; the user's own call overrides it.

## Taste ledger

_Empty. Each entry: the call, its reason, before and after paths, the verdict._

## Differences from the reference

_None recorded yet. The app's own surfaces are the reference and the directions are briefed to match them._

## Feature backlog set aside by the loop

_Empty._
