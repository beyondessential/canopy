---
id: ADM
---

# Administrative access

The private server's administrative surface is restricted to administrators.
Two permissions are carried on a caller's authenticated tailnet identity: administrator, which admits them to the surface, and danger, which admits them to its most consequential operations (see [SAFE](safety-modes.md)).
Both are decided at request time, from the same grant and the same allowlist, so there is one grant to author, one refresh, and one place to look.
This concerns human operators of the administrative API and is distinct from the device administrator role (see [DTR](device-trust.md)).

## Who holds a permission

A caller holds a permission when either:

- their allowlist entry records it, or
- the tailnet policy grants them the corresponding Canopy capability.

The two sources are independent, and either alone suffices.
An operator adds, amends, or removes an allowlist entry directly; the policy-granted set is authored in the tailnet policy and not editable through Canopy.
An allowlist entry records a login together with the permissions it carries, so granting danger to an operator amends their entry rather than adding them to a second list.

The two permissions are independent: administrator does not confer danger, and danger does not confer administrator.
Because almost the whole administrative surface is restricted to administrators, an operator holding danger alone reaches very little, so danger is granted alongside administrator rather than in place of it.

## The capability grant

The tailnet policy confers a permission through an application-capability grant.
A grant confers a permission when both hold:

- its application capabilities include `bes.au/cap/canopy` with a value carrying that permission's key set true, and
- its destinations include the tag under which the Canopy service is published on the tailnet, `tag:server-canopy`.

The key is `admin` for administrator and `danger` for danger.
A capability value carrying neither key set true confers nothing.
The grant's sources name the principals who thereby hold the permissions its value carries.

## Resolving grant sources

Each source of a conferring grant is resolved, against the same policy, to the callers it covers:

- A group resolves to its listed member logins.
- A bare user login resolves to itself.
- `autogroup:member` covers any caller bearing a tailnet user identity.
- `autogroup:tagged` covers any caller identified only by a device tag.
- Any other autogroup is not resolved and covers no caller.

A caller holds a permission by policy when their identity matches any resolved source of a grant conferring it.
The administrative surface admits only callers bearing a tailnet user identity, so a source resolving solely to tagged devices never yields a permission in practice.

## Freshness and availability

A permission derived from the policy reflects the policy as of the most recent successful read, refreshed periodically.
Reading the policy requires read access to the tailnet policy file.
When the policy cannot be read, the recorded allowlist remains authoritative, so a control-plane outage never withdraws a permission held through the allowlist.

A permission is resolved afresh for each request that depends on it, so withdrawing one takes effect at once rather than when the caller's current session ends.

## Reporting administrative status to a caller

A caller can ask whether it is an administrator without itself being one, so that a client can decide whether to offer administrative controls before attempting anything.
The answer is `true` for an administrator and `false` for a caller that definitely is not one, including a caller bearing no tailnet identity at all.
A failure that is not an authorization outcome — the allowlist being unreadable, say — is reported as an error rather than as `false`, because a caller cannot tell a wrongly-negative answer from a real one.

## Presenting administrative controls

An operator client decides the whole session's administrative controls from a single answer, so that every part of a page agrees about whether the operator is an administrator.
Until an answer arrives, administrative controls are withheld.
Once an answer arrives it is retained for the session and refreshed periodically; a later failed refresh leaves the retained answer in place rather than withdrawing controls mid-session.
While no answer has yet arrived and the request is failing, the client retries and tells the operator that administrative status is undetermined, rather than presenting an unexplained view with no controls.

A control withheld because the operator is not an administrator is absent.
A control the operator could use in a higher safety mode is present and blocked instead (see [SAFE](safety-modes.md)).
