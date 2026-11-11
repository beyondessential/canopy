# Direction H: principals and roles

The principals list is home, permissions live in roles, and Suspend is the cut-off. Seven files are in `.workhorse/design/mockups/p6/h-*.html`, built by `build/build_h.py`, with renders in `shots/r4/`.

## Hard cases, by file

| File | Shows |
|---|---|
| `h-principals` | 1 (admins marked Tailscale policy), 2 (an unknown tailnet node, not authorised), 3 (twenty-three rows), 5 (a revoked token, a suspended admin), 7 (six fleet machines surfaced with their reason above the fleet row) |
| `h-principal` | 5 (a revoked enrolled key beside the live trust), 6 (`releaser` scoped to `bestool` by the role), 8 (GitHub Actions trust) |
| `h-suspend` | 9 (what stops at once, an optional reason, revoking the token too with its cost) |
| `h-roles` | 3, 5 (a suspended role), 7 (`fleet-machine` with its count and Show) |
| `h-role` | 6 (own-group scope, trusted by tailnet tag) |
| `h-who-can` | 1, 7 (harbour's fleet machines as a count, and why they do not match), 9 (the suspended admin with who, when and why), 10 |
| `h-token-minted` | 4 |

## Decided since round 2

- `restore-consumer` and `backup-verifier` are trusted by tailnet tag; their members authenticate as tailnet nodes.
- A scope can be one named record of the noun: `bestool`, `syd-1`, `grafana-sso`.

## Departures from the brief

- The release-publisher's old credential is a revoked enrolled key, not a disabled certificate: Revoke is the word for credentials.
- The matrix gains Triage and Request verbs and Status and Identity tokens nouns, so every role summary is expressible in it.
- Vulnerability reporting is part of `fleet-machine`'s default, so the fleet row's claim holds without listing dozens of machines.

## Unsure

- Whether a direct credential's Roles column should list the principal's direct roles, as drawn, or stay empty.
- The principals add form sits above its long table; the roles add forms sit under theirs, as the shipped Admins page does.
- Whether Canopy can resolve tailnet group membership. No trust here relies on it.
