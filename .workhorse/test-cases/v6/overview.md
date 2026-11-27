# V6 test cases

## Access levels

- [x] A login on the PM list holds PM; one on the admin list holds admin; one on neither holds all (verifies spec: ADM)
- [x] A grant whose value carries `pm: true` on `tag:server-canopy` gives its group's members PM; the same value on another tag gives nothing (verifies spec: ADM)
- [x] A value carrying both `admin` and `pm` gives admin; a value carrying neither gives no level (verifies spec: ADM)
- [x] A login on the PM list and granted admin by the policy holds admin (verifies spec: ADM)
- [x] A login on a list matches whatever its case, as does one named in the policy (verifies spec: ADM)
- [x] A grant with `*` as a source gives its level to every tailnet user (verifies spec: ADM)
- [x] With no tailnet API configured, a listed PM holds PM and nothing else confers a level (verifies spec: ADM)
- [x] With the policy unreadable after a good read, a policy-granted PM keeps PM and a listed PM keeps PM (verifies spec: ADM)
- [x] Removing someone from the PM list refuses their next project details edit (verifies spec: ADM)
- [ ] The level endpoint answers all for a plain tailnet user, and the client shows no PM or admin controls to them (verifies spec: ADM)
- [x] Existing admins are still admins after the migration (verifies spec: ADM)

## Permissions page

- [ ] Shows admin, PM and all, highest first, each with what it may do (verifies spec: ADM)
- [ ] Admin and PM each name their key and the sources the policy grants them to: a group with its member count, a login as one person, `autogroup:member` and `*` as everyone, and an undefined group or other autogroup as no one (verifies spec: ADM)
- [ ] All shows no key and no list, only that everyone on the tailnet who can reach Canopy holds it (verifies spec: ADM)
- [ ] Shows when the policy was last read, or that Canopy isn't configured to read it (verifies spec: ADM)
- [x] An admin adds and removes a login on the PM list, each needing danger mode and each leaving an audit entry naming level and login (verifies spec: ADM)
- [ ] `/settings/admins` leads to the permissions page
- [ ] A PM or all-level caller has no Permissions entry in the sidebar, and the server refuses them the lists and the policy's grants (verifies spec: ADM)

## Project details

- [ ] A PM in write mode edits a group's project, PM in charge and contact from its page (verifies spec: GRP)
- [x] A PM edits a group whose PM in charge is someone else, and one with none (verifies spec: GRP)
- [ ] A PM in read-only mode sees the edit control blocked, and raising from it opens the form (verifies spec: GRP, SAFE)
- [ ] A PM sees no Add machine, Edit or Archive on the group page, and the server refuses a PM's group update (verifies spec: GRP)
- [ ] An all-level caller gets no edit control, and the server refuses their edit (verifies spec: GRP)
- [x] A PM in charge with no `@`, or nothing either side of it, is refused (verifies spec: GRP)
- [x] An edit leaves an audit entry holding each changed field before and after; saving unchanged details leaves none (verifies spec: GRP)
- [ ] The PM in charge shows by display name for a login Canopy knows, and as entered otherwise (verifies spec: GRP)
- [ ] Empty details read as none (verifies spec: GRP)

## Projects view

- [x] Lists groups with a live production environment, and leaves out a group with only test and demo (verifies spec: PRJ)
- [x] Leaves out an archived group (verifies spec: PRJ)
- [ ] The viewer's groups come first in their own highlighted section, matched on login ignoring case (verifies spec: PRJ)
- [ ] A viewer in charge of nothing sees one section without an empty "yours" heading (verifies spec: PRJ)
- [ ] Each group shows its production central, then facilities by name, with public and tailnet links opening in a new tab, the tailnet link over HTTPS (verifies spec: PRJ, SVC)
- [ ] Two facilities sharing a name are told apart by their machine (verifies spec: PRJ, FLT)
- [ ] A group whose production environment holds no Tamanu shows its details and reads as having no Tamanu (verifies spec: PRJ)
- [ ] A facility with no URL shows no public address; a machine with no tailnet name, or a cluster-hosted application, shows no tailnet address (verifies spec: PRJ)
- [x] Test and pending Tamanu in the same group are not listed (verifies spec: PRJ)
- [ ] Search narrows by group name, project, contact, and PM by display name or login, ignoring case (verifies spec: PRJ)
- [ ] "Projects" sits under Fleet in the sidebar and opens `/fleet/pm` (verifies spec: PRJ, FLT)

## Homepage

- [ ] With nothing chosen and no `home` grant, `/` opens the status page (verifies spec: HOME)
- [ ] A grant with `home: "/fleet/pm"` makes `/` open the projects view (verifies spec: HOME)
- [x] A `home` that isn't a single-`/` path (`//evil.example`, `https://…`, `fleet`), is `/` alone, or carries `?` or `#` is ignored (verifies spec: HOME)
- [x] A `home` on a grant whose destinations leave out `tag:server-canopy` is ignored (verifies spec: HOME)
- [x] Two `home`s: higher priority wins; equal priority goes to more segments, a trailing `/` adding none; then alphabetical (verifies spec: HOME)
- [x] A `priority` that isn't a number ranks at zero (verifies spec: HOME)
- [ ] A chosen homepage naming a path that matches no page opens as a missing page, and the toggle there clears it (verifies spec: HOME)
- [ ] The toggle on a page that isn't home makes it home, and `/` opens it in a new session (verifies spec: HOME)
- [ ] The toggle on the chosen homepage clears the choice, and `/` opens the default again (verifies spec: HOME)
- [ ] On a page that is home only by default, the toggle shows on and clicking it changes nothing (verifies spec: HOME)
- [ ] A query string on the page isn't kept in the chosen homepage (verifies spec: HOME)
- [ ] The toggle works in read-only mode (verifies spec: HOME, SAFE)

## Bestool snippets

- [x] An all-level caller lists and opens snippets but cannot save or delete one; an admin can (verifies spec: ADM)
- [x] Deleting a snippet without a tailnet identity is refused (verifies spec: ADM)

## Safety modes

- [ ] The mode control offers write but not danger to a PM, both to an admin, and shows read-only alone to an all-level caller (verifies spec: SAFE)

## Fleet query interface

- [x] Get group returns the project details; Find groups matches a term in the project, PM in charge or contact (verifies spec: MCP)
