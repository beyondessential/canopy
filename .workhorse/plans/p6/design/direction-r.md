# Direction R: roles first

**What it is.** Settings, Access opens on the set of roles. A role is a matrix: nouns in sections, verbs as columns. Each noun accepts only some verbs and offers a scope only where it makes sense (group, environment, software, cluster). Every principal is a member of roles, directly or because a trust matches it (tailnet tag, group or policy, enrolled key, GitHub claim). The five device roles and two MCP grants become ordinary roles; `admin`, `operator` and `fleet-machine` are built in. Admins and MCP access become members of roles.

**The bet.** A few named shapes stay auditable. Scope can be "own group", so `restore-consumer` is one role, not one per group.

**What it gives up.** Role sprawl, and "what can this principal do" being a union. Handled in view:
- A one-off is an exception: a one-principal role with a reason and an end date, kept under one collapsed row.
- Every role shows members and last used. Needs attention flags a role whose permissions another role already holds.
- The principal page lists effective permissions, each with the role it came through. Who can inverts that.

**No explicit deny.** Roles are allow-only, as in Tamanu, so the union is monotone and "who can" is a lookup. Cutting off is Suspend on a principal or a role: it switches off every membership at once, including admin granted by tailnet policy, which Canopy cannot edit. Revoke stays for tokens.

**Files, and the hard cases they carry**
- `r-roles.html`: 1 (lock chip), 2 (`staging-jump-1`), 3, 5 (suspended role), 7 (fleet row: count, Show all, two outliers).
- `r-role-releaser.html`: 6 (scope chips), 8 (GitHub Actions claim), 5 (disabled key).
- `r-token-minted.html`: 4, 5 (revoked token), in Write mode.
- `r-principal.html`: exception, effective permissions, 9 (Suspend).
- `r-who-can.html`: 10, 1, 7 (1 of 412), 5 and 9 (suspended row), 3.

**Where each idea comes from.** Matrix and noun-declares-verbs: Tamanu. Trust by claim: crates.io and Tailscale. Assumed roles and the who-can query: IAM. Match counts and last matched: firewall tables.

**Unsure**
- Suspend overriding tailnet-granted admin is new behaviour. Otherwise it waits on a policy edit.
- The fleet drill-down (Show all 412) is drawn as a control, not as the list it opens.
- At 1440px the app shows a sidebar with the mode band at its foot, no top bar. I followed the source.

**Verification.** All five rendered in headless Chromium at 1440px, with geometry checked for overflow, clipping and wrapped chips. The screenshots reached me as unreadable image data, so I did not inspect them by eye, and worked from the React source, not the reference captures.
