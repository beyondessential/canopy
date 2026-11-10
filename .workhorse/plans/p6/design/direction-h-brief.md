# Round 2 brief: the principals-and-roles hybrid (H)

One agent builds one refined set of mockups for the direction the user chose after round 1. Read `direction-brief.md` first: its sections on the references, fleet machines, fidelity, visual language, copy and the ten hard cases all still bind. Read `inventory.md` for the facts. This file only adds what round 2 settles.

## The direction, settled

- **Home is the principals list** (from P). Every caller in one table: operators, MCP agents, the relay, releasers, restore consumers, newcomers, and the fleet held back as one row.
- **Permissions live in roles** (from R). A role is a matrix of verbs on nouns, sectioned by noun, with a scope per row where the noun accepts one: all groups, the member's own group, or one named group or environment. Built-in roles: `admin`, `operator`, `fleet-machine`. The five device roles and two MCP grants of today become ordinary roles.
- **A principal holds roles** directly, or because a trust on the role matches it: a tailnet tag, a tailnet login, an enrolled machine key, or a GitHub Actions claim (repository, workflow file, environment). Do not draw trusts by tailnet group: whether Canopy can resolve group membership is an open question.
- **A one-off is an exception**: a role with one member, a reason and an end date. Exceptions sit under one collapsed row on the roles list.
- **Allow-only.** There is no deny row anywhere. Taking access away in a hurry is **Suspend**, on a principal or on a role. A suspended principal holds no permission at all, including admin granted by the Tailscale policy. Revoke stays for a token.
- **Borrowed from G:** a "Last matched" column (when the principal or role last authorised a request, and what) and the "Who can…" checker.

## Where things sit

The Settings › Access group in the sidebar becomes: Principals, Roles, Certificate authority, Identity provider, Audit log. Admins and MCP access are gone from it, folded into Principals. "Who can…" is not a sidebar entry: it is an outlined button in the page header of both Principals and Roles, and its own page.

## Files to produce

Write into `.workhorse/design/mockups/p6/`, prefixed `h-`. Seven files:

1. `h-principals.html`: the home list. Columns: principal (name, with its kind beneath), authenticates by, roles (chips; a scoped role shows its scope in the chip), last matched, status. At least eighteen rows. Sorted with anything needing attention first. Shows hard cases 1, 2, 3, 5, 7.
2. `h-principal.html`: the releaser `release-publisher`. Sections: credentials (its GitHub Actions trust, matched through the `releaser` role, plus a disabled certificate), roles held (direct or through which trust), effective permissions (each with the role it comes through and when it was last used). Suspend in the page header. Shows hard cases 5, 6, 8.
3. `h-suspend.html`: the same page with the Suspend confirmation dialog open on `incident-triage-bot`. The dialog names what stops working, plainly and briefly. Shows hard case 9.
4. `h-roles.html`: the roles list. Columns: role, members (count, with how they join), permissions summary, last matched, status. A suspended role among live ones, the built-ins marked as such, `fleet-machine` with its member count and Show, the exceptions collapsed. Shows hard cases 3, 5, 7.
5. `h-role.html`: the role `restore-consumer`. The permission matrix with own-group scope; trusts (enrolled machine key with tag `tag:restore-consumer`); members. Shows hard case 6.
6. `h-who-can.html`: "Who can consume restores for group harbour?". The question as editable fields at the top, the answer as a table: principal, through which role and trust, scope that matched, last matched. Include the suspended principal struck through with why, the Tailscale-policy admin, and the fleet as a count at the foot. Shows hard cases 1, 7, 9, 10.
7. `h-token-minted.html`: creating the MCP agent principal `grafana-sidecar` with role `triage`; the bearer token shown once. Shows hard case 4.

## Start from round 1, do not redraw

Round 1 already matched the app's chrome. Copy the nearest round-1 file and edit it, rather than writing from scratch:

- `h-principals` from `p-principals.html`
- `h-principal` and `h-suspend` from `p-principal-detail.html`
- `h-roles` from `r-roles.html`
- `h-role` from `r-role-releaser.html`
- `h-who-can` from `p-who-can.html`
- `h-token-minted` from `p-token-minted.html`

Update the sidebar's Access group in every file to the list above, with the right entry selected.

## What round 1's critique asks of this round

- One vocabulary across all seven files: "principal", "role", "trust", "exception", "Suspend", "Revoke", "Last matched". No "grant" as a noun on screen.
- A role chip reads the same everywhere it appears (list, detail, who-can).
- The fleet row reads the same on every page that holds it back: a count of machines and groups, a Show button, and any outlier machines listed above it with the reason they surfaced.
- No hint text that narrates what the system does. State shows as state (a Suspended chip, a Revoked chip, a struck-through role).

## Write-up

Write `.workhorse/plans/p6/design/direction-h.md`, under 350 words: which file shows which hard case, every place you departed from this brief and why, and anything you were unsure of.

Render every file with headless Chromium at 1440px wide, full page, into `.workhorse/plans/p6/design/shots/r2/<file>.png`. Check the DOM for overflow, clipping and wrapped chips and fix what you find. Work only in your own files.
