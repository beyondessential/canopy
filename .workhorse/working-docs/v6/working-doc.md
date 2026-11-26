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
- Each person chooses their homepage: what `/` opens, which today is always the status page. Someone who hasn't chosen gets a default, which is the PM view for people in certain groups (F) and the status page for everyone else.

## Scope

- This card: A, B, E, F, and the homepage choice.
- D (token sign-in to Tamanu) becomes its own cards, one here and one in Tamanu.
- C (credentials) is dropped in favour of D, with a root password kept in Bitwarden for rare use.
- G (public internet access) becomes a later card.

## Parts

The card bundles several pieces of work that can move separately. Each is listed with what Canopy already has toward it.

### A. PM details on a group

Project, PM in charge, contact on the ground. Free text, no data model behind project or contact.

- Groups already carry free-form markdown notes with a revision log (FLT "Notes"). Project and contact could live there by convention, but the view can't then pull them out to show in a list or search.
- So: three named fields on the group, beside the notes. The PM in charge is the one with meaning: it has to match a person so "my groups" works.
- Editing them is a write, so today it needs an admin. Whether a PM can edit their own groups' details is open.

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

- A tab of the fleet view (`/fleet/…`, beside `archived` and `figures`).
- "My groups" are the groups naming the signed-in person as PM in charge, matched on their tailnet login.

### Homepage choice

Each person picks what `/` opens. Someone who hasn't picked gets the PM view if F says they're a PM, otherwise the status page.

- Canopy keeps no per-user preferences today (it caches display names in `tailscale_users`, and sessions hold only safety mode), so this is new storage keyed by login.
- Is the choice between any page, or a short fixed list (status, PM view, fleet, incidents…)?
- Where it's set: a menu on the user's avatar, or a "make this my homepage" action on the page itself.

### F. Who counts as a PM

Decides the default homepage, from the person's groups in the tailnet policy.

- Option: a `bes.au/cap/canopy` capability value naming a default homepage (`home: "pm"`), granted to a policy group and resolved the same way admin is (ADM). Keeps the policy in the grant model Canopy already reads, and generalises if other groups want another default.
- Option: Canopy configured with the name of a policy group (`group:pm`) whose members default to the PM view. Simpler to read, but ties Canopy to a group name in the policy.
- Being named PM in charge on a group doesn't count: it's per group, and the default has to work for a PM before anyone fills in A.
- bestool P4 relies on Tailscale's `whois` returning a peer's caps for a Tailscale Service. Canopy could read caps that way per request instead of resolving the policy file itself. That would simplify how admin is resolved too, and is a change of its own.

### G. Public internet access

PMs not on Tailscale reach Canopy over the internet behind Google OAuth.

- The private server today trusts Tailscale's identity headers and rejects anything else on its UI routes, so this is a second front door with its own identity, and everything behind it has to decide what a non-tailnet person may see.

## Open questions

- [x] Which parts are this card, and which become cards of their own? (see Scope)
- [x] Tailnet first; G later.
- [x] C dropped in favour of D.
- [ ] A: can a PM edit their own groups' details, or only admins?
- [x] E is a subpage of the fleet view, with a per-person homepage choice.
- [ ] F: capability value or a configured policy group name?
- [ ] Homepage: any page or a fixed list, and where is it set?
