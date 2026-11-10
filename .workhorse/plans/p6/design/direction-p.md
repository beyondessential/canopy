# Direction P: principals first

## What it is

One list of every principal: operators, MCP agents, the relay, releasers, restore consumers, newly arrived callers and the fleet. Each row shows kind, how it authenticates, what it can do, last seen and status. A principal's page holds its credentials and effective permissions, editable there, each permission labelled with its source.

## The bet

To an authorisation system, humans and machines are the same thing. The five-page split under Settings › Access follows how the features were built, not how operators think. "Who can reach Canopy and what can they do" becomes one screen.

## What it gives up

Cross-cutting questions. The list is organised by who, so "who can consume restores for this group" needs its own answer page. That is the weak point, drawn as such.

## Files and hard cases

- `p-principals.html`: 18 rows (3). The fleet is one class row, "410 more, in 14 groups", with a Show button, plus two fleet machines that surface for a new key and a permission beyond the fleet default (7). A policy-granted admin reads "Tailscale policy" (1). A revoked agent sits among live ones (5). Scoped grants are chips (6). A not-authorised caller is first (2).
- `p-principal-detail.html`: a releaser with all four credential shapes: GitHub Actions trust credential (repository, workflow, environment), certificate, bearer token, tailnet identity (8). Role and tag permissions are muted, with where to edit them.
- `p-who-can.html`: who can consume restores for one group, each principal with its grant or role and when it last used it (10). The fleet is a foot row.
- `p-token-minted.html`: the one-time token dialog (4).
- `p-newcomer.html`: no permissions, with Authorise as fleet machine and a grant form (2).

## Deny (9)

One: Suspend, a Canopy-side deny overriding every allow, including a tailnet-policy admin. It shows as a struck-through Admin chip and a "suspended" status. The Tailscale policy is slow to edit, so an emergency cut-off must work from Canopy.

## References

- Principal, effective permissions, explicit deny, "who can" page: AWS IAM.
- Credential shapes and claim fields: Tailscale and crates.io trust credentials.
- Permission as scoped verb on noun: Tamanu.
- Last-used column: firewall hit counts.

## Unsure

- Role and tag permissions are muted with a link out rather than editable, so one table reads two ways.
- The fleet class has no page of its own, only the Show button.
- Suspend overriding a policy admin may surprise someone reading the policy file.

## Verification

Rendered all five files at 1440px in headless Chromium. The image viewer gave no readable pixels, so I checked DOM geometry (overflow, clipping, row heights), not by eye.
