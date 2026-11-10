# Design loop — P6, auth control plane

Rigour: Standard. Round cap 6. Stop rule: two clean rounds, the second by fresh eyes, no criticals and few suggestions.

## Where the run is

Step 1, references. The card has no specs, no plan and no commits, so there is no build to polish: the run starts at direction, not at polish. Step 3 waits on an implementation.

## Step 0 — does the card need design

Yes. The card's third goal is a control plane over human and machine identity with least-privilege permissions, and its first is choosing at runtime which tagged devices may call which parts of the Canopy API. Neither has a pattern in the app: Settings › Access is five separate pages (Admins, MCP access, Certificate authority, Identity provider, Audit log), each covering one credential kind, and none of them grants a scoped permission to a principal. A new surface, a new layout and controls with no precedent — design.

## In flight

- Inventory of every authentication and authorisation mechanism in the codebase. It is the input to all three sides of the design space, so it runs first.
- The rig: `private-server` + `migrate` built, `private-web` dependencies installed, so the existing Access pages can be captured through the Playwright fixture.

## Next

- Capture the app's own neighbouring surfaces as the consistency reference: the five Access pages, a device's detail page, and the audit log.
- Ask the user for a product reference and for which rung of the card's ladder the design is aiming at.
- Three structurally different directions for the control plane, critiqued side by side.

## Open questions

- Which rung: dynamic tagged-device authorisation only, something between, or the unified control plane. The surface differs completely between them.
- Whether a product does this well and should be the reference.

## Taste ledger

_Empty. Each entry: the call, its reason, before and after paths, the verdict._

## Differences from the reference

_None recorded yet._

## Feature backlog set aside by the loop

_Empty._
