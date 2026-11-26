---
status: draft
---

# PM-focused interface for Canopy

A view of Canopy for project managers, giving each group with a production environment its project, PM in charge, on-the-ground contact, and access to that environment's Tamanu central and facilities, with the groups a PM is in charge of surfaced first.

## Behaviour

- The card's "deployment" is a group. The PM view covers production environments only; clone, demo, test and dev are left out.
- A group has at most one production environment (one environment per rank), so "a deployment" and "a group's production environment" are one-to-one here.

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

The list of groups with production environments, the PM's own groups in a section at the top and highlighted, a search box across groups, and each group's details from A and B (and D's sign-in links).

- Whether this is a separate page, a mode of the existing fleet view, or a per-user preference that changes the landing page is open.
- Canopy keeps no per-user preferences today (it caches display names in `tailscale_users`, and sessions hold only safety mode), so a preference would be new storage keyed by login.

### F. Who counts as a PM

Needed if the view defaults on for PMs, or if PMs get rights others don't.

- Option: a `bes.au/cap/canopy` capability value (`pm: true`), resolved from the tailnet policy the same way admin is (ADM).
- Option: anyone named as PM in charge on some group is a PM. No policy change; falls out of A.
- bestool P4 relies on Tailscale's `whois` returning a peer's caps for a Tailscale Service. Canopy could read caps that way per request instead of resolving the policy file itself. That would simplify how admin is resolved too, and is a change of its own.

### G. Public internet access

PMs not on Tailscale reach Canopy over the internet behind Google OAuth.

- The private server today trusts Tailscale's identity headers and rejects anything else on its UI routes, so this is a second front door with its own identity, and everything behind it has to decide what a non-tailnet person may see.

## Open questions

- [ ] Which parts are this card, and which become cards of their own?
- [ ] The card says "let's just start with testing": was that "start with Tailscale", i.e. leave G for later?
- [ ] C: drop credentials in favour of D, or store some in Canopy meanwhile?
- [ ] A: can a PM edit their own groups' details, or only admins?
- [ ] E: separate page, view mode, or per-user preference?
- [ ] F: capability, implied by being named PM in charge, or both?
