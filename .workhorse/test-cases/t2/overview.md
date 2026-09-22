# Gate the administrative surface behind safety modes

Scenarios verifying [SAFE](../../specs/private-server/safety-modes.md) and the danger permission in [ADM](../../specs/private-server/admin-access.md).

Server-side cases exercising a boundary need the real tailnet header path rather than the debug identity, which is an administrator in danger mode by construction.

## Modes and enforcement

- [x] A read-only session is refused a write-graded request (verifies spec: SAFE)
- [x] A write session is refused a danger-graded request (verifies spec: SAFE)
- [x] A danger session is permitted a write-graded request, the ladder holding downwards (verifies spec: SAFE)
- [x] A read-only-graded request is answered when the caller has no session at all (verifies spec: SAFE)
- [x] A request carrying an unknown or expired session identifier is treated as read-only rather than refused outright (verifies spec: SAFE)
- [x] A session identifier is not usable by a login other than the one it belongs to (verifies spec: SAFE)

## The danger permission

- [x] An operator without the danger permission cannot raise to danger (verifies spec: SAFE)
- [x] An operator without the danger permission is refused a danger-graded request even when their session is in danger mode (verifies spec: SAFE)
- [x] The danger permission is held through the allowlist entry alone, with no policy grant (verifies spec: ADM)
- [ ] The danger permission is held through the policy capability alone, with no allowlist entry (verifies spec: ADM)
- [x] A capability value carrying neither key confers neither permission (verifies spec: ADM)
- [x] Administrator does not confer danger (verifies spec: ADM)
- [x] Withdrawing the danger permission takes effect during an operator's existing danger raise, not at the end of it (verifies spec: ADM)

## Refusals

- [x] The two refusals are distinguishable: lacking the permission, and being below the required mode (verifies spec: SAFE)
- [x] A client refused for its mode returns its indicator to read-only without a reload (verifies spec: SAFE)

## Expiry and lifetime

- [x] A raise returns to read-only ten minutes after it is made, regardless of activity in between (verifies spec: SAFE)
- [x] The server accepts a request at the raised grade in the minute after the raise has lapsed (verifies spec: SAFE)
- [x] A raise holds when the request is served by a different private-server process (verifies spec: SAFE)
- [x] A raise survives a restart (verifies spec: SAFE)
- [x] A session not seen for some time is retired, and a retired session is no longer live (verifies spec: SAFE)
- [x] An operator lowers their mode from the mode control without waiting for the countdown (verifies spec: SAFE)

## Grading the surface

- [x] Every handler on the administrative surface carries a grade, and one added without a grade fails the build (verifies spec: SAFE)
- [ ] Every tool on the fleet query interface is read-only (verifies spec: SAFE)
- [x] Minting a fleet-query access token is danger, and listing tokens is read-only (verifies spec: SAFE)
- [x] The SQL playground is graded read-only despite running operator-supplied queries (verifies spec: SAFE)

## Interface

- [x] A reloaded page comes back read-only (verifies spec: SAFE)
- [x] A raise made before the page has its session is not undone when the session arrives (verifies spec: SAFE)
- [x] Raising to danger asks for confirmation; raising to write does not (verifies spec: SAFE)
- [x] The current mode and its remaining time are visible at all times, counting down from ten minutes (verifies spec: SAFE)
- [x] A blocked control does not act when clicked, and names the mode it requires (verifies spec: SAFE)
- [x] A blocked control cannot be reached from the keyboard, and Enter in a field of its form does not submit it (verifies spec: SAFE)
- [ ] A popover of individually graded rows opens at the lowest grade it offers, so un-silencing is reachable in write mode (verifies spec: SAFE)
- [ ] Machine setup below danger offers the enrollment ticket as a blocked control, and mints it once the operator raises (verifies spec: SAFE)
- [x] A control requiring write or danger carries a stripe in that grade's colour, muted at rest and full colour under the pointer (verifies spec: SAFE)
- [x] A control disabled for a reason unrelated to its grade carries no stripe (verifies spec: SAFE)
- [x] A control withheld because the operator is not an administrator is absent rather than blocked (verifies spec: ADM)
- [x] Danger is granted and withdrawn from the administrators screen (verifies spec: ADM)
