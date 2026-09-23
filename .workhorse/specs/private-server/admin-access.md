---
id: ADM
---

# Administrative access

The private server's administrative surface is restricted to administrators.
Whether a caller is an administrator is decided at request time from the caller's authenticated tailnet identity.
What an administrator may do at a given moment is governed by their session's safety mode (see [SAFE](safety-modes.md)).
This concerns human operators of the administrative API and is distinct from the device administrator role (see [DTR](device-trust.md)).

## Who is an administrator

A caller is an administrator when either:

- their login is on the recorded administrator allowlist, or
- the tailnet policy grants them the Canopy administrator capability.

The two sources are independent, and either alone suffices.
An operator adds or removes an allowlist entry directly; the policy-granted set is authored in the tailnet policy and not editable through Canopy.

## The administrator grant

The tailnet policy confers administrative access through an application-capability grant.
A grant confers administrative access when both hold:

- its application capabilities include `bes.au/cap/canopy` with a value carrying `admin` set true, and
- its destinations include the tag under which the Canopy service is published on the tailnet, `tag:server-canopy`.

A capability value that does not carry `admin` set true confers no administrative access.
The grant's sources name the principals who thereby become administrators.

## Resolving grant sources

Each source of an administrator-conferring grant is resolved, against the same policy, to the callers it covers:

- A group resolves to its listed member logins.
- A bare user login resolves to itself.
- `autogroup:member` covers any caller bearing a tailnet user identity.
- `autogroup:tagged` covers any caller identified only by a device tag.
- Any other autogroup is not resolved and covers no caller.

A caller holds policy-granted administrative access when their identity matches any resolved source of an administrator-conferring grant.
The administrative surface admits only callers bearing a tailnet user identity, so a source resolving solely to tagged devices never yields administrative access in practice.

## Freshness and availability

Administrative status derived from the policy reflects the policy as of the most recent successful read, refreshed periodically.
Reading the policy requires read access to the tailnet policy file.
When the policy cannot be read, the recorded allowlist remains authoritative, so a control-plane outage never withdraws administrative access held through the allowlist.

Administrative status is resolved afresh for each request, so removing an administrator takes effect at once rather than when their current session ends.

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
