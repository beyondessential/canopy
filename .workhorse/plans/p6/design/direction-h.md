# Direction H: principals and roles

Round 2 refines the hybrid the user chose. The principals list is home, permissions live in roles, and Suspend is the cut-off. Seven files are in `.workhorse/design/mockups/p6/h-*.html`, with renders in `shots/r2/`.

## Hard cases, by file

| File | Shows |
|---|---|
| `h-principals` | 1 (admin chips marked Tailscale policy), 2 (an unknown tailnet node refused with no roles), 3 (twenty rows), 5 (a revoked token and a suspended admin among live rows), 7 (two outliers above the fleet row and its Show) |
| `h-principal` | 5 (a disabled certificate beside the live trust), 6 (scoped permissions), 8 (GitHub Actions trust on repository, workflow and environment) |
| `h-suspend` | 9 (the dialog lists what stops at once, takes a reason, offers to revoke the token too) |
| `h-roles` | 3, 5 (a suspended role), 7 (`fleet-machine` with its count and Show); built-ins marked, exceptions collapsed |
| `h-role` | 6 (Consume on restore replicas and Read on backups, own group) |
| `h-who-can` | 1, 7 (fleet as a count at the foot), 9 (the suspended admin struck through), 10 |
| `h-token-minted` | 4 |

## Departures from the brief

- The build agent failed to start (prompt too long, as in round 1). The main session built the files with `build/build_h.py`, reusing round 1's CSS and sidebar.
- `h-role` trusts `restore-consumer` by tailnet tag rather than by an enrolled machine key with a tag. The restore operators are tailnet nodes today.
- In `h-who-can` the fleet row answers "none of 412": `fleet-machine` does not hold Consume, and saying so is part of the answer.

## Unsure

- Whether a suspended Tailscale-policy admin should read as Suspended or as Policy first. It reads Suspended here, since that is what decides access.
- Whether Last matched should say what matched (`write note`) on lists, or only on detail pages. Lists show the time alone.
- The scope column on `h-role` holds one scope per noun row. A role that wants Read everywhere and Consume in its own group on the same noun has no way to say it.
