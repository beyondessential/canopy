---
id: ADM
---

# Access levels

What a caller may do on the private server's operator surface is decided by their access level, at request time, from the caller's authenticated tailnet identity.
What they may do at a given moment within that level is governed by their session's safety mode (see [SAFE](safety-modes.md)).
This concerns human operators of the operator surface and is distinct from the device administrator role (see [DTR](device-trust.md)).

## The levels

The levels are admin, PM, and all.
They are a ladder: each includes everything the levels below it may do, and a caller's level is the highest any source gives them.

- All reads the operator surface.
- PM also edits any group's project details (see [GRP](../servers/groups.md), "Project details").
- Admin does everything on the operator surface, Canopy's own settings included.

Every caller reaching the operator surface with a tailnet user identity holds all.
Wherever this spec set says a thing is done by an administrator, it is done by a caller at the admin level.

## How a level is held

A caller holds a level when either:

- their login is on that level's list, or
- the tailnet policy grants them that level.

The two sources are independent, and either alone suffices.
Each level has a list of its own, which an administrator edits in Canopy; the policy-granted set is authored in the tailnet policy and not editable through Canopy.

## The Canopy grant

The tailnet policy confers levels through an application-capability grant.
A grant confers a level when both hold:

- its application capabilities include `bes.au/cap/canopy` with a value carrying that level's key set true, `admin` for admin and `pm` for PM, and
- its destinations include the tag under which the Canopy service is published on the tailnet, `tag:server-canopy`.

A capability value carrying no level key set true confers no level, and a value may carry several.
A value may also carry a default homepage for the grant's sources (see [HOME](homepage.md), "The default homepage").
The grant's sources name the principals who thereby hold the level.
A caller covered by several grants receives a value from each.

## Resolving grant sources

Each source of a level-conferring grant is resolved, against the same policy, to the callers it covers:

- A group resolves to its listed member logins.
- A bare user login resolves to itself.
- `autogroup:member` covers any caller bearing a tailnet user identity.
- `autogroup:tagged` covers any caller identified only by a device tag.
- Any other autogroup is not resolved and covers no caller.

A caller holds a policy-granted level when their identity matches any resolved source of a grant conferring it.
The operator surface admits only callers bearing a tailnet user identity, so a source resolving solely to tagged devices never yields a level in practice.

## Freshness and availability

A level derived from the policy reflects the policy as of the most recent successful read, refreshed periodically.
Reading the policy requires read access to the tailnet policy file.
When the policy cannot be read, the lists remain authoritative, so a control-plane outage never withdraws a level held through a list.

A caller's level is resolved afresh for each request, so taking a level from someone takes effect at once rather than when their current session ends.

## The permissions page

The operator interface presents the levels on a permissions page under the settings' access section, one row per level, highest first, each saying what the level may do.
Only an administrator sees the page, and only an administrator reads the lists or what the policy grants each level.
Beside each level are its two sources:

- its tailnet grant, naming the capability key that confers it and the sources the policy currently grants it to, each with how many people it resolves to;
- its list, with the logins on it.

All has no key: its tailnet side is who the tailnet policy lets reach Canopy at all.

The page explains how to grant a level on the tailnet: the capability's name, the destination tag, that a grant's sources receive every level whose key is true, and how a grant sets a default homepage.
It says when the policy was last read.

Only an administrator adds a login to a list or removes one.
Adding a login issues access and removing one invalidates it, so both are graded accordingly (see [SAFE](safety-modes.md)).
Both are audited actions, naming the level and the login (see [AUD](../platform/audit-log.md)).

## Reporting a level to a caller

A caller can ask for its own level without holding any particular one, so that a client can decide which controls to offer before attempting anything.
The answer is the caller's level, and all for a caller holding nothing higher.
A caller bearing no tailnet identity at all holds no level and is told so.
A failure that is not an authorization outcome, such as a list being unreadable, is reported as an error rather than as a lower level, because a caller cannot tell a wrongly-low answer from a real one.

## Presenting controls by level

An operator client decides the whole session's controls from a single answer, so that every part of a page agrees about the operator's level.
Until an answer arrives, controls beyond reading are withheld.
Once an answer arrives it is retained for the session and refreshed periodically; a later failed refresh leaves the retained answer in place rather than withdrawing controls mid-session.
While no answer has yet arrived and the request is failing, the client retries and tells the operator that their access level is undetermined, rather than presenting an unexplained view with no controls.

A control withheld because the operator's level does not reach it is absent.
A control the operator could use in a higher safety mode is present and blocked instead (see [SAFE](safety-modes.md)).
