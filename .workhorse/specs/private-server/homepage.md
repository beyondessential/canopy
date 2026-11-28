---
id: HOME
---

# Homepage

Applies to what the operator interface opens at its root, how a person chooses that page, and the default given to someone who has not chosen.

## What the root opens

The operator interface's root opens the person's homepage, which is the page they have chosen where they have chosen one, and their default homepage otherwise (see "The default homepage").

A homepage is a page's path within the operator interface.
What follows the path, such as a search or a filter, is not part of it.

A homepage that no longer names a page, such as one since removed or one the person's level does not show them, opens as any page that does not exist does.
Where it is their chosen homepage, the toggle there clears it.

A person's choice belongs to their tailnet login, so it holds across their sessions and the clients they use.

## Choosing a homepage

A home toggle sits in the top-right corner of the navigation, beside the Canopy mark, on every page.
It is faint at rest, stronger and filled on the page that is the person's homepage, and at full strength under the pointer.

Activating it on a page that is not the person's homepage makes that page their chosen homepage.
Activating it on the page they chose clears their choice, so the root opens their default again.
On a page that is their homepage only by default, it shows as on and activating it does nothing.
Its label says what activating it does, or that the page is their default homepage where it does nothing.

Choosing or clearing a homepage changes only the person's own preference, so it needs no raised safety mode (see [SAFE](safety-modes.md), "Grading the operator surface").

## The default homepage

A person's default homepage is the `home` their tailnet grants give them, and the status page where they give none.

A Canopy capability value in the tailnet policy may carry `home`, a path within the operator interface, alongside or instead of a level key (see [ADM](access-levels.md), "The Canopy grant").
The grant's sources, resolved as for a level, receive that path, where the grant's destinations include `tag:server-canopy` as a level-conferring grant's do.
A value may also carry `priority`, a number ranking its `home` against others, and a value carrying none, or one that is not a number, ranks at zero.

A `home` is taken only where it is a path within the operator interface, beginning with a single `/`, with more than `/` alone, with no `.` or `..` segment and with no query or fragment, so a policy value never sends anyone elsewhere or back to the root.
Any other `home` is ignored.

A person covered by several grants may be given several `home`s, and one is their default, chosen by, in order:

- the highest `priority`;
- then the path with the most segments, so `/fleet/pm` ranks above `/status`, a trailing `/` adding none;
- then the path first in alphabetical order.

> [!NOTE]
> People commonly sit in several overlapping policy groups, so differing `home`s are expected and settled by rule rather than reported as a conflict.

A default derived from the policy is as fresh as the levels derived from it, and when the policy cannot be read the last default read stands (see [ADM](access-levels.md), "Freshness and availability").
