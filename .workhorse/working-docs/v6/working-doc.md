---
status: draft
---

# PM-focused interface for Canopy

A view of Canopy for project managers, giving each group with a production environment its project, PM in charge, on-the-ground contact, and access to that environment's Tamanu central and facilities, with the groups a PM is in charge of surfaced first.

## Behaviour

- The card's "deployment" is a group. The PM view covers production environments only; clone, demo, test and dev are left out.
- A group has at most one production environment (one environment per rank), so "a deployment" and "a group's production environment" are one-to-one here.
- Tailnet only for now. Public access (G) comes later.
- The PM view is a subpage of the fleet view, beside the groups list, archived and figures tabs.
- Each person chooses their homepage: what `/` opens, which today is always the status page. Someone who hasn't chosen gets a default, which is the page their policy grants name (F), the PM view for PMs and the status page for everyone else.

## Scope

- This card: A, B, E, F, H, and the homepage choice.
- D (token sign-in to Tamanu) becomes its own cards, one here and one in Tamanu.
- C (credentials) is dropped in favour of D, with a root password kept in Bitwarden for rare use.
- G (public internet access) becomes a later card.

## Parts

The card bundles several pieces of work that can move separately. Each is listed with what Canopy already has toward it.

### A. PM details on a group

Project, PM in charge, contact on the ground. Free text, no data model behind project or contact.

- Groups already carry free-form markdown notes with a revision log (FLT "Notes"). Project and contact could live there by convention, but the view can't then pull them out to show in a list or search.
- So: three named fields on the group, beside the notes. The PM in charge is the one with meaning: it has to match a person so "my groups" works.
- PMs edit them, as well as admins. Any PM, on any group, rather than only the PM in charge: a group with no PM in charge yet still needs someone to fill it in, and PMs cover for each other.
- This is the first write a non-admin can make in Canopy. Only these three fields: the rest of the group's edit form stays admin-only.
- Each edit is audited, recording the fields changed from and to, like other group edits.
- Safety mode: a PM raises to write through the same interface as an admin (the mode control, or one click from the blocked control). A raise authorises nothing by itself (SAFE), so it opens nothing else to a PM. Danger isn't offered to a PM, since nothing a PM can do needs it.

### B. Access to the production environment

The Tamanu central and facilities in the production environment, each with its public URL and its tailnet URL.

- Mostly derived, not entered: Canopy knows each application's type and rank, its configured URL, and its machine's tailnet name (SVC already builds links from these).
- Not every machine is on the tailnet, so a member can have a public URL only, a tailnet URL only, or both.

### C. Credentials

The passwords to log in to each Tamanu.

- Canopy's reads are open to anyone on the tailnet, so passwords stored in Canopy would be readable by everyone on it unless this part gets its own gate.
- A Bitwarden integration was raised and doubted in the card itself.
- Part D would make this unnecessary for day-to-day access, leaving a root password in Bitwarden for rare use.

### D. Sign in to Tamanu through Canopy

Canopy mints a short-lived token for the signed-in PM and links to `https://<tamanu>/login?token=…`; Tamanu verifies it against Canopy as an OIDC issuer and signs the PM in.

- Canopy is already an OIDC issuer (IDP, IDT), but its tokens name machines and applications. This needs tokens naming a person, issued from the operator UI rather than the device API.
- Tamanu needs the other half: accepting the token, and mapping the person to a Tamanu user (existing user by email, or created on first use).
- Every issuance is audited, which gives the per-person access log the card wants.
- Cross-repo: Tamanu's side is its own card.

### E. The PM view

The list of groups with production environments, the PM's own groups in a section at the top and highlighted, a search box across groups, and each group's details from A and B (and D's sign-in links once D lands).

- A tab of the fleet view at `/fleet/pm`, beside `archived` and `figures`.
- "My groups" are the groups naming the signed-in person as PM in charge, matched on their tailnet login.

### Homepage choice

Each person picks what `/` opens. Someone who hasn't picked gets the `home` their policy grants name (F), otherwise the status page.

- Canopy keeps no per-user preferences today (it caches display names in `tailscale_users`, and sessions hold only safety mode), so this is new storage keyed by login.
- Any page, since the policy default (F) is a path too.
- Where it's set: leading option is a "make this my homepage" toggle in each page's header, shown filled on the page that is your homepage. Fallback, if the toggle doesn't hold up in mockups: a per-person page under Settings. (There's no per-person menu in the shell today: the sidebar has the sections, Settings holds Canopy-wide settings, and the safety mode band is the only per-session control.)

### F. Who counts as a PM

Sets the default homepage, and decides who may edit the PM details (A), from the tailnet policy's app capability grants.

- Proposed grant (beyondessential/tailscale#55): the Tamanu PM team gets `home: "/fleet/pm"` in the `bes.au/cap/canopy` value on `tag:server-canopy`, beside on-call's `admin: true` grant. Resolved the same way admin is (ADM), so policy freshness and the outage rules carry over.
- `home` is a path, not a role: the policy can give any team any default page. So the PM view lives at `/fleet/pm`.
- Canopy takes `home` only as a path within Canopy (starts with a single `/`), so a policy value can never send someone off-site.
- `home` doesn't carry the right to edit A. That comes from the PM access level (H): `pm: true` in a grant, or the PM email list.
- One person can get several cap values from several grants, so two grants can name different `home`s.
- Being named PM in charge on a group doesn't count: it's per group, and the default has to work for a PM before anyone fills in A.
- Where Canopy gets a person's caps from:
  - Today: it reads the whole policy file through the Tailscale API (needs the policy-read scope), resolves grant sources itself (groups, bare users, `autogroup:member`; other autogroups resolve to nobody), and refreshes on a timer. It can list everyone each level resolves to, which the permissions page (H) wants.
  - Per request from Tailscale: Serve (v1.92+) forwards the caller's caps in a `Tailscale-App-Capabilities` header when started with `--accept-app-caps`, and strips any incoming copy. Tailscale does the resolving, so every source form works and there's no refresh lag. But Canopy is published through the Kubernetes operator's ingress, which (as of upstream main, checked locally) doesn't expose that setting, and the header only describes the caller, so the page couldn't list who holds a level.
  - P4's `whois` with `svc_name` gives the same per-caller answer, but needs a tailscaled Canopy can query, which it doesn't have.
- Either way a cap arrives as a list with one value per matching grant, in no guaranteed order, so the rule for two different `home`s is needed whichever source is used.

### H. Access levels and the permissions page

The Admins settings page (today an email allowlist) becomes a permissions page. Not a full permissions model: a fixed set of access levels, and for each, the ways a person gets it.

- Levels: admin, support, PM, all.
- Each level is reached two ways: a `bes.au/cap/canopy` grant in the tailnet policy carrying that level's key set true (`admin`, `support`, `pm`), or an email list of its own on the page. Either alone suffices, as for admin today (ADM). The page reads as a table: levels down the side, the two ways across.
- The page spells out how the tailnet side works: the capability name, the key for each level, the destination tag, and that the grant's sources are who gets it. Ideally also who the policy currently resolves to each level, so an operator can check a grant took effect.
- "All" is everyone with access to Canopy at all. On the tailnet that's whoever the policy lets reach Canopy (today `autogroup:member` on `tag:server-canopy`), so its email list first matters once G lets people in from outside.
- The email lists keep the admin allowlist's property: a control-plane outage never withdraws a level held through a list.
- Editing the lists stays admin-only, and each change is audited, as admin allowlist changes are.
- `home` (F) isn't a level: it's a default, and stays its own key.

### G. Public internet access

PMs not on Tailscale reach Canopy over the internet behind Google OAuth.

- The private server today trusts Tailscale's identity headers and rejects anything else on its UI routes, so this is a second front door with its own identity, and everything behind it has to decide what a non-tailnet person may see.

## Mockups to make

- The PM view at `/fleet/pm`: my groups at the top, highlighted; search; each group's PM details and production access links.
- Editing the PM details as a PM.
- The homepage toggle in a page header, on and off (lead).
- A per-person homepage page under Settings (fallback, for comparison).
- The permissions page: levels against grant and email list, with the tailnet explanation.

## Open questions

- [x] Which parts are this card, and which become cards of their own? (see Scope)
- [x] Tailnet first; G later.
- [x] C dropped in favour of D.
- [x] A: PMs edit the PM details, on any group.
- [x] A: PMs raise to write through the same safety mode interface.
- [x] Each access level has an email list beside its tailnet grant (H).
- [x] E is a subpage of the fleet view, with a per-person homepage choice.
- [x] F: default homepage from a `home` path in the Canopy capability (tailscale#55).
- [x] F: the PM access level grants editing A (H).
- [ ] H: what the support level grants.
- [ ] H: are levels nested (admin includes PM's rights, and so on) or independent?
- [ ] F: two grants naming different `home`s for one person.
- [ ] F: keep reading the policy file, or move to per-request caps from Tailscale?
- [x] Homepage: any page; set by a header toggle (lead) or a Settings page (fallback), to settle in mockups.
