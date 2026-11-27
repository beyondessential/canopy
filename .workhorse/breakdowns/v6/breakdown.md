# PM interface: split-off cards

Parts of the PM interface that move separately from this card, which builds the Projects view, the PM details on a group, the homepage choice and the permissions page.

## Sign in to Tamanu through Canopy

Canopy issues a short-lived token naming the signed-in person, from the operator UI, and the Projects view links to each Tamanu's login with it, so a PM reaches Tamanu without a password. Canopy is already an OIDC issuer, but its tokens name machines and applications; this adds tokens that name a person. Every issuance is audited, giving a per-person access log.

Tamanu's half, accepting the token and mapping the person to a Tamanu user, is a card of its own in Tamanu. A root password kept in Bitwarden covers the rare access that needs one.

## Add the support access level

Adds support between admin and PM on the permissions page's ladder, reached by its own grant key or email list like the other levels. Support does the day-to-day fleet work on machines, applications and groups, and saves and deletes bestool snippets; admin is support plus Canopy's own settings, some of which only admins may see.

That splits today's single admin gate into two grades: every write handler is placed as support or admin, and the admin-only settings pages gain a read gate. Where the support/admin line falls is this card's to decide, along with the tailnet policy change that gives people support.

## Reach Canopy from the public internet

People not on the tailnet, PMs first, reach Canopy over the internet by signing in with Google. This is a second front door beside the tailnet's identity headers, so everything behind it decides what someone from outside the tailnet may see. The "all" level's email list on the permissions page is who may come in this way.
