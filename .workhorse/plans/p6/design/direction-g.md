# Direction G: grants first

## What it is

The operator manages the policy; principals are what it applies to. Settings > Access gets a Grants page: one table where each row reads as a sentence (these principals may, or may not, do these actions on this resource), with 30-day matches and last match. No policy-file editor.

Evaluation follows IAM: default deny, allows add up, a deny beats any allow. Row order is presentation only (denies, tailnet policy, then broad to narrow), so order cannot hide a precedence bug. Rows flag interactions ("Overrides g16", "Partly denied", "Covered by g10").

## Bet and cost

Bet: operators ask "who can do this" more than "what can this person do", so a flat table plus a simulator beats nested roles. Fleet machines are three quiet rows and a counted band.

Cost: "what can this principal do" is computed, not a page. Principal detail, credential lifecycle and last-seen are thin. No Tamanu-style matrix, so one role's full reach is harder to review.

## Files and hard cases

- `g-grants.html`: 20 rows. Read-only tailnet admin row (1). Newcomer with no grants (2). Revoked and expired beside live (5). Scoped restore rows (6). Fleet band (7). GitHub Actions credential (8). Pinned denies (9).
- `g-fleet.html`: three fleet grants, 212 machines, three unlike the fleet first (7).
- `g-new-grant.html`: trust credential by repository, workflow file and environment, with a "Read as" sentence and overlap check (8).
- `g-token-minted.html`: secret shown once (4).
- `g-evaluate.html`: who can consume restores for harbour, each principal with its deciding row (10).

## References

Row shape, hit counts: firewall tables. Sources and capabilities as data: Tailscale grants. Deny wins, simulator, "decided by": AWS IAM. Scoping a verb to one object: Tamanu. Trust credential form: crates.io trusted publishing.

## Cutting someone off

There is an explicit deny. One row cuts a token or group off at once and shows what it overrides (g22 over g16). Allow-only would mean deleting every row that reaches the principal.

## Unsure

- Whether a Canopy deny may override the read-only tailnet admin grant. Shown as not.
- Group rows need Canopy to resolve tailnet group membership; the inventory does not say it can.
- Fleet hit counts in the millions are probably noise.
- The brief describes a top bar; at 1440px the app has a sidebar with the mode band at its foot, so I followed the code.

## Verification

Rendered all five at 1440px in headless Chromium and ran DOM checks for overflow, clipping and wrapped chips: none found. I could not inspect the screenshots pixel by pixel, so fine spacing against the real app is unchecked.
