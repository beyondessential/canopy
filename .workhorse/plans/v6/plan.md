# V6: PM interface

Specs: [ADM](../../specs/private-server/access-levels.md), [PRJ](../../specs/servers/projects.md), [HOME](../../specs/private-server/homepage.md), [GRP](../../specs/servers/groups.md) "Project details", [SAFE](../../specs/private-server/safety-modes.md), [FLT](../../specs/servers/overview.md) "Navigating the two grains".
Reasoning is in the working doc at `.workhorse/working-docs/v6/working-doc.md`.
Token sign-in to Tamanu, the support level and public access are cards of their own (`.workhorse/breakdowns/v6/breakdown.md`).

## Tech design

### Where levels and `home` come from

Canopy keeps reading the whole policy file (`TailnetDirectory::refresh_policy`), and `resolve_admins` grows into resolving every level key and every `home`:

- `AdminGrants` becomes per-level grants: for each of `admin` and `pm`, the lowercased logins and whether `autogroup:member` is a source. Same source resolution as today.
- Each `bes.au/cap/canopy` value carrying a `home` (a string starting with a single `/`) contributes `(home, priority)` to every login its grant's sources resolve to, and to every member where the source is `autogroup:member`. Resolving one login's default picks by priority, then segment count, then alphabetical order. That rule is a pure function, unit-tested on its own.
- The cache also keeps, per level, the sources that conferred it with their resolved member count, and the time of the last successful read, for the permissions page.
- A failed read keeps the previous grants, homes and time, as `admin` is kept today.

The per-request caller capabilities header (`Tailscale-App-Capabilities`) would replace this once the Kubernetes operator's ingress can forward caps. It isn't used here (see "Explicitly rejected").

### Levels on the server

- An `AccessLevel` enum (`All < Pm < Admin`), resolved per request from the lists and the policy, highest wins.
- `TailscaleAdmin` keeps its name and meaning (level admin). A new `TailscalePm` extractor admits PM and admin. The debug-only shortcut keeps acting as `admin@localhost` at the admin level, and `CANOPY_TRUST_TAILSCALE_HEADERS=1` exercises the real resolution.
- `commons::is_current_user_admin` becomes `commons::current_user_level`, returning the level. A caller with no tailnet identity gets `null` where today it gets `false`. The SPA gets `useAccessLevel()`. `useIsAdmin` stays as the derived predicate its 33 call sites actually ask (`level === "admin"`), built on the same single fetch.

### Lists

The `admins` table generalises to one table of list entries keyed by `(level, login)`, with level a Postgres enum (`admin`, `pm`, `all`). A migration (`just migration access_lists`) moves every `admins` row in as `admin` and drops `admins`. `database::admins` becomes `database::access_lists`, every caller updated.

Handlers move from `fns/admins.rs` to `fns/access.rs`:

- `list`: read_only, silent. Each level's entries, plus the policy's conferring sources with member counts and the last read time.
- `add`: `danger(issues)`, audited `AccessListEntryAdded` (level, login).
- `remove`: `danger(invalidates)`, audited `AccessListEntryRemoved` (level, login).

Today's admin allowlist handlers are audit-silent; these are audited, as the spec now says.

### Project details

Three nullable text columns on `server_groups`: `project`, `pm_in_charge`, `ground_contact`. Empty strings are stored as null.

- `server_groups::update_project_details`: `TailscalePm`, `write`, `audit(GroupProjectDetailsChanged)`, `audit.unchanged()` when nothing differs. The action records the group, and its detail holds each changed field with `from` and `to`.
- `get` and the projects listing return the PM in charge with the display name from `tailscale_users` where the login is known there.
- The group's admin edit form (`GroupEdit`) doesn't carry the three fields, so it can't be used to change them.

### Projects listing

`server_groups::projects` (read_only, silent) returns every live group with a live production environment, its details, and its production `tamanu-central` and `tamanu-facility` applications with:

- the application's configured URL as the public address;
- the machine's bound tailnet name as the tailnet address (none for a cluster-hosted application or a machine without one).

The groups are ordered by name. Splitting them into the viewer's and everyone else's, and searching, happen in the SPA, since the whole list is small and searching it needs no round trip.

### Homepage

- Table `operator_homepages(login text primary key, path text not null)`, keyed by lowercased login.
- `homepage::get` returns `{ chosen, default }`: `chosen` from the table, `default` from the policy's resolution for the caller. `homepage::set { path }` and `homepage::clear`: read_only (own preference, per SAFE), silent. `set` refuses a path that isn't a single-`/` path.
- The SPA's `/` route becomes a resolver: `chosen ?? default ?? "/status"`, then `Navigate` with `replace`.
- The toggle compares the current pathname (no query string) with the effective homepage.

### Safety modes

`SafetyModeControl` offers write from PM up and danger only to admins. Nothing changes server-side: a raise authorises nothing, and handlers are gated by level.

### Settings

`/settings/admins` becomes `/settings/permissions` ("Permissions" under Access), with a redirect from the old path. `Admins.tsx` becomes `Permissions.tsx`.

## Cross-repo

Assumes beyondessential/tailscale#55 lands, granting the Tamanu PM team `{ "pm": true, "home": "/fleet/pm" }` on `tag:server-canopy`. Until it does, PMs are added through the PM list.

## Explicitly rejected

- Reading caps from the `Tailscale-App-Capabilities` header, or from `whois`: the operator's ingress can't forward the header, Canopy has no tailscaled to query, and neither can list who holds a level.
- Keeping project and contact in the group's notes by convention: the view couldn't show or search them.
- Storing Tamanu credentials in Canopy.

## Checklist

### Database

- [ ] Migration: `access_list_entries` with the level enum, `admins` rows moved in, `admins` dropped
- [ ] `database::access_lists` replacing `database::admins`, every caller moved
- [ ] Migration: `server_groups.project`, `pm_in_charge`, `ground_contact`
- [ ] `ServerGroup::update_project_details` returning what changed
- [ ] Projects query: live groups with live production Tamanu, with machine tailnet names
- [ ] Migration and model: `operator_homepages`
- [ ] Audit actions `GroupProjectDetailsChanged`, `AccessListEntryAdded`, `AccessListEntryRemoved`

### Policy and levels

- [ ] Per-level grant resolution and `home`/`priority` collection in `tailnet_directory`
- [ ] Default homepage rule as a pure function, with unit tests
- [ ] Conferring sources with member counts, and last read time, kept in the cache
- [ ] `AccessLevel` resolution and the `TailscalePm` extractor
- [ ] `commons::current_user_level`, replacing `is_current_user_admin`

### Handlers

- [ ] `fns/access.rs` replacing `fns/admins.rs`
- [ ] `server_groups::update_project_details` and the details on `get`
- [ ] `server_groups::projects`
- [ ] `fns/homepage.rs`
- [ ] `just gen-openapi`

### SPA

- [ ] `useAccessLevel`, with `useIsAdmin` derived from it
- [ ] Sidebar: home toggle in the brand row; "Projects" under Fleet
- [ ] `/` resolves the homepage
- [ ] Projects view at `/fleet/pm`: sections, highlight, search, address links
- [ ] Group detail: Project panel and its edit dialog, shown from PM up
- [ ] Permissions page at `/settings/permissions`, with a redirect from `/settings/admins`
- [ ] `SafetyModeControl` offers modes by level

### Tests

- [ ] Rust endpoint tests per the test cases
- [ ] Playwright coverage per the test cases, with seed helpers for project details, list entries and homepages
