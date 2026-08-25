# Safety modes for the administrative surface

Canopy's administrative surface is live by default: any operator who reaches it can act, and the only distinction is admin or not.
That has cost us data more than once, through operators clicking controls they had access to but no business using.

The fix has two halves.
Every session starts read-only and an operator opts briefly into a higher mode that expires on its own, so the interface is safe by default even for the most privileged operator.
Alongside that, a new permission tier reserves the genuinely destructive actions for the personnel trusted with them, enforced on the server rather than in the client.

## Make administrative identity testable end to end

Both tailnet auth extractors short-circuit under `cfg!(debug_assertions)` and return a fixed `admin@localhost` identity, so every debug build is unconditionally an administrator.
Nothing in the suite currently exercises a denial: the admin-gating Playwright specs intercept the status probe in the browser to fake a failure, and the server underneath always answers yes.
Building a permission tier on that harness would leave every grading decision unverified, so this comes first.

Replace the compile-time bypass with a runtime opt-in, keeping `cfg!(debug_assertions)` as the outer guard and requiring an environment variable inside it, so release builds cannot bypass authentication even by misconfiguration.
The opt-in should switch on trusting the real `Tailscale-User-*` request headers rather than naming a fixed identity, because the e2e stack is shared across the tests in a worker and a fixed identity could not vary between them.
Each test then chooses its own login per request, and administrative status resolves through the real allowlist and policy path rather than a stub.
With the variable unset the behaviour is exactly as it is today, so the existing integration tests are untouched and this lands as pure addition.

## Gate the administrative surface behind safety modes

An operator's session is read-only until they raise it to write or to danger, and it returns to read-only ten minutes later on its own.
Raising to danger asks for confirmation; raising to write does not.
The current mode is visible at all times with its remaining time, and controls the operator cannot presently use are visibly blocked rather than absent, so the interface reads the same whatever mode they are in.

Danger becomes a real permission alongside the existing administrator one, carried on the same tailnet capability grant and the same allowlist table so there is one grant, one refresh, and one place to look.
An operator without it cannot enter danger mode, and the server refuses the underlying requests regardless of what the client sends.
Every handler on the private server's administrative surface is graded to a mode as part of this work; there are around 130 of them and that grading is the bulk of the card.
The existing administrator boundary is a poor guide to it, since creating a backup configuration and deleting one sit at the same level today and should not.

The concept is borrowed from seedling's safety modes, but none of the implementation is: this is designed for canopy against canopy's own components and naming.

## Record an audit log of administrative actions

Canopy has no systematic record of who did what.
What exists is per-feature provenance on a handful of handlers that happen to bind the calling administrator, plus a cached user table so resolutions and notes can show an avatar.
An events table existed once and was dropped.

This card adds a real audit log covering every administrative action, captured at the middleware or extractor level rather than by editing each handler in turn.
It is deliberately separate from the safety-mode gate, which stands on its own and should not wait on it.
Replicating write and danger entries to the backups as an append-only offsite copy belongs to this card and should be raised as an entry in its own breakdown once it is created.
