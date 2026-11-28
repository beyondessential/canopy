---
id: SVC
---

# Service links

An application's detail view links an operator out to the web service it runs, and a machine's to the services running on the box, so an operator can jump from Canopy straight to those interfaces.
How metadata is reported is the status contract (see [STA](../public-server/statuses.md)); the tailnet identity these links resolve against is the trust model (see [DTR](device-trust.md)).

## Application link

When an application has a known address — its configured URL, or its machine's tailnet name when it has no configured URL — the detail view offers a link that opens it in a new tab.
An application with no known address offers no application link.
Beside the link to a Tamanu central or facility, sign-in to it is offered where it can be (see [TSI](tamanu-sign-in.md)).

## Munin link

Munin monitors the box rather than anything running on it, so whether Munin is running is a machine figure and the link belongs to the machine.

A machine reports whether it runs Munin as a field in its pushed status, and the flag is one of the machine's reported figures (see [FIG](figures.md)) — read from the most recent source to report it, and held for as long as the machine exists.
So a machine that has reported running Munin keeps the link however long it has been quiet since, a status that omits the flag leaves the value unchanged, and a machine that reports it has stopped running Munin loses the link.

The machine's detail view offers a link to its Munin only when the machine is known to run Munin and has a bound tailnet name.
The link opens Munin in a new tab, over HTTPS at the machine's tailnet MagicDNS name on port 4950.
A machine not known to run Munin, or without a tailnet name, offers no Munin link.
