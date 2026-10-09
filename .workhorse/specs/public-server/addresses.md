---
id: ADR
---

# Application DNS addresses

An application asks Canopy to publish the address records that make one of its DNS names resolve, because Canopy is the only holder of write access to the zone.
The request is refused unless an operator has granted the application permission to manage its own DNS records, and is confined to DNS names within the domains the application's group controls (see [DOM](../servers/domains.md)).
Which application a request is about, what is recorded when that cannot be settled, and how an operator declares and denies DNS names for addresses are common to certificates and are in [DNS](dns-names.md).
Addresses are declared, requested, recorded, denied, and presented without reference to certificates: an application may register addresses for a DNS name it holds no certificate for, and the reverse.

## Registering

An application registers the DNS name it should be reachable at together with the external addresses it is reachable at, and Canopy publishes the address records: the IPv4 addresses as A records, the IPv6 addresses as AAAA records, at that DNS name, in the managed zone it resolves to.

Registering replaces the addresses previously registered for the DNS name, so an application announces a change of address by registering again, and a registration naming no addresses withdraws the DNS name.
Canopy publishes what it is told: it does not verify that an address is really the application's, the grant being the trust boundary rather than any proof of possession.

Canopy changes only records it created itself.
Because zones are shared, a DNS name may be served by records Canopy knows nothing about, and Canopy neither rewrites nor removes those; it records what it has published so it can tell its own records from everyone else's.

A DNS name's addresses are the addresses of the one application that holds it for addresses, so two applications cannot fight over where one DNS name points.

## Presentation

The DNS names section of an application, a machine, and a group is described in [DNS](dns-names.md), "Presentation"; this is what it carries for addresses.

An application presents the DNS names it declares for addresses, with the addresses published for each and whether the zone has caught up with what it asked for.
A declared DNS name with no addresses registered presents as declared without addresses, distinct from one whose addresses are being withdrawn.
A DNS name outside every domain the group controls presents flagged as such, since nothing can be published for it until a covering domain is claimed.

A group presents, under each domain it controls, the DNS names declared for addresses beneath it and whether each has its records published.

A machine's DNS names section carries the DNS names declared for addresses, the undeclared address requests, and the DNS names denied for addresses, and nothing about certificates.
