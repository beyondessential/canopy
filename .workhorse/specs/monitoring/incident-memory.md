---
id: MEM
---

# Incident memory

A store of short titled entries that carry what was learned from past incidents to the next investigation, written and read by AI agents and by operators.

## What an entry holds

An entry holds a title, a body of free-form text, and optionally one link.
A link names a group, a check, or an incident, and an entry with no link applies to the fleet as a whole.
A check is named by its source and its presented name (see [CHK](checks.md), "Names"), so an entry about a check applies wherever that check is filed.

An entry records who recorded it and when, who last amended it and when, and whether it is retired.
A title is short and a body is bounded, and an entry over either bound is refused with the limit named.

An entry is for what a later investigator would otherwise rediscover: a recurring cause, how a check's failure was told apart from another, a standing quirk of a site, where a fix is kept.
It is not a log of what happened during one incident, which is the incident's own timeline and notes (see [INC](incidents.md)).

## Changing entries

An entry can be amended: its title, body, and link are each replaceable.
An amendment states the revision of the entry it was made against and is refused if the entry has changed since, so two writers cannot silently overwrite each other.

An entry can be retired, recording who and why.
A retired entry is no longer offered by default and is kept in full, with its history, and can be found by asking for retired entries.

The history of an entry keeps every version it has had and every retirement, each with who made it, how, and when: an operator in the operator interface, or a fleet query access token through the fleet query interface (see [MCP](../private-server/mcp.md)).
It is shown newest first.

Recording, amending, and retiring an entry are audited actions (see [AUD](../platform/audit-log.md)).
An entry's text is held in its history, and the audit entry names the entry and the revision it concerns rather than repeating the text.

## Who can read and write

Any tailnet user can read entries in the operator interface.
Recording, amending, and retiring there are for administrators.
Through the fleet query interface, an agent reads entries freely and writes them where its access token carries the notes grant.

## The memory page

The operator interface offers a page under incidents that lists the entries, newest amended first.
The list can be searched by free text matched against titles and bodies, narrowed to one link kind or to one group, check, or incident, and widened to include retired entries.
Each entry opens to its body, its link resolved to the group, check, or incident it names, and its history.
Administrators record, amend, and retire from the same page.

## Reading for context across incidents

The fleet query interface offers the index described in [MCP](../private-server/mcp.md), "Incident memory", so an agent starting an investigation can see what is already known, where it applies, and where recurring trouble has no memory yet.
