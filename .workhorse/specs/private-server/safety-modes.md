---
id: SAFE
---

# Safety modes

Every operator session on the private server's administrative surface runs in one of three safety modes, and every handler on that surface is graded to the mode it requires.
A session is read-only until the operator raises it, and returns to read-only on its own.
This governs what an operator may do at a given moment, and is distinct from whether they may reach the surface at all (see [ADM](admin-access.md)).
A safety mode guards against mistakes rather than authorising anyone: every administrator may raise to any mode.

## The modes

The modes are read-only, write, and danger.
They are a ladder: danger permits everything write permits, and write permits everything read-only permits, so an operator in danger mode does ordinary write work without dropping back down.

A session begins read-only.
A raise lasts ten minutes from the moment it is made, and the session returns to read-only when that elapses, whatever the operator has been doing in the meantime.

The server accepts requests at a raised grade for one minute longer than the raise lasts.
That margin is not offered to the operator and does not appear anywhere: it exists so that a request already in flight as the raise ends is not refused for an expiry the operator had no way to anticipate.

## Raising and lowering

An operator raises from the mode control, or by reaching for a blocked control (see "Raising from a blocked control").
From the mode control, raising to write takes effect without confirmation, and raising to danger asks the operator to confirm first.

An operator lowers their mode from the mode control at any time, without waiting for the remaining time to run out.

## Raising from a blocked control

Activating a blocked control asks the operator to raise to the mode that control requires.
This holds for write as well as danger, so a stray click on a blocked control raises nothing by itself.
The confirmation is titled with the action the control takes, and says the action needs that mode.
Its confirming choice says the action continues in that mode, since confirming both raises and carries the action out.
The confirmation for danger also says what danger mode unlocks, as the one reached from the mode control does.
A control requiring danger raises straight to danger, from read-only as from write.

Every graded control names its action and what it acts on, such as "Revoke certificate for host-3", rather than relying on its visible label, which is often a bare verb.

Confirming raises the session to that mode and then carries out the activation, whether the control makes a change itself or opens a form, dialog, or confirmation for one.
A control with a confirmation of its own still shows it after the raise: one confirms the mode, the other the action.
A control that is also unavailable for a reason of its own, such as an incomplete form, is raised for but not activated, and stays in the state that reason puts it in.
A raise made this way lasts as long as one made from the mode control.

Cancelling leaves the mode and the control as they were.
If the raise fails, the operator is told their mode is unchanged and the action is not carried out.

A blocked item in a menu closes the menu when chosen, as any other choice does, before asking for the raise.

A blocked control is reachable from the keyboard, and activating it there does what clicking it does.
Pressing Enter in a field of a form whose save is blocked does what activating the save does.

## The session

A mode belongs to a single client session rather than to the operator's identity.
One operator working in two clients has two sessions, and those sessions can be in different modes.

The server holds each session's mode and decides every graded request against it, so a client reaches no further than its session's mode however it describes itself.
A session's mode holds however the surface is served, and survives a restart of it.

A client that has lost its record of its own mode is read-only, so doubt resolves downwards.
A request presenting a session identifier that is unknown or has expired is treated as read-only rather than refused outright.
A session is usable only by the login it belongs to.
A read-only-graded request needs no session at all, so a client can read before it has one.

A session that has not been seen for some time is retired.

## Grading the administrative surface

Every handler on the administrative surface declares the mode it requires, and a handler that declares none fails the build, so none can be added ungraded.
Each handler's grade is available to the operator interface, so that it can present a control according to the mode that control needs.

A handler that changes nothing is read-only.
This holds where the read is a sensitive one: the SQL playground runs operator-supplied queries inside a read-only transaction that is always rolled back, so it is graded read-only.

A handler that changes something is danger when any of the following holds, and write otherwise.

- It cannot be undone from the interface. Deleting a backup configuration qualifies; creating one does not.
- It acts on the fleet rather than amending Canopy's own records. Revoking a machine's certificate qualifies; renaming a group does not.
- It removes a protection without destroying anything at the time. Closing a machine's restore window and pausing certificate renewal both qualify.
- It issues or invalidates credentials or trust material. Minting a fleet-query access token and changing the certificate authority both qualify.

Changing a backup schedule at any layer is graded write, including setting it to manual-only or clearing it: a schedule is routine configuration, set back from the same editor, and every change to it is recorded (see [BKO](backup.md)).

A handler that lets an already-trusted machine obtain credentials is graded write, because making a machine trusted is itself danger and the decision to trust it has already been made there.
Opening a machine's restore window is graded on this basis.

The fleet query interface (see [MCP](mcp.md)) changes nothing, so every one of its tools is read-only.
The handlers that mint, revoke, and list its access tokens belong to the administrative surface rather than to that interface: minting and revoking are danger, and listing is read-only.

The handlers by which an operator reads and changes their own safety mode are read-only, because a read-only session has to be able to reach them: what they change is that operator's own session rather than the fleet or Canopy's records.

A handler's grade is independent of who may reach it.
A handler reachable by a caller who is not an administrator is graded on the same basis as any other, and every operator has a safety mode.

## Refusals

A graded request from a session below the mode it requires is refused, and the refusal names the mode required.
A client refused for its mode returns its indicator to read-only and tells the operator that their raise has lapsed, so client and server agree again without a reload.

## Presenting the mode

The current mode and the time remaining on it are visible at all times, counting down from ten minutes when a raise is made.
While raised, the mode control is filled in that mode's colour and wears its stripe (see "Presenting graded controls").
In the raise control, each mode above read-only wears its stripe in its colour, muted at rest and in full colour under the pointer and for the current mode, so the operator learns the treatment from the control that grants it.

A reloaded page is read-only.

## Presenting graded controls

A control the operator could use in a higher mode is present and blocked rather than removed, so the surface has the same shape whatever mode the operator is in.
A blocked control names the mode it requires, and is presented as clickable, since activating it offers the raise (see "Raising from a blocked control").

A control that opens a form, dialog, or confirmation for making a change carries the grade of that change, so the operator is never led through filling in something they cannot submit.
Where what the form saves needs a higher mode for some inputs than for others, its opener carries the lowest of them, and the save carries the grade of the submission it would make.
A control that opens something also worth reading without making a change, such as a detail view with an edit inside it, is not graded; the change inside it is.

A control requiring write or danger carries the grade it requires as a diagonal stripe in that grade's colour, muted while at rest and coming to full colour under the pointer.
Write and danger stripes differ in angle as well as colour, so the two read apart from the pattern alone.
The same treatment is used throughout the surface and on the mode control itself, so an operator learns it once.

A control the operator can use is drawn in the colour of the grade it requires, the colour of its stripe when blocked and of the mode that permits it, so what a control takes is readable from the control itself.
The grade's colour takes the place of any colour the control would otherwise have, so a colour means the same grade wherever it appears.
A control that requires no mode keeps its own colour.

A control disabled for a reason unrelated to its grade, such as a request in flight or an incomplete form, carries no stripe, so the treatment never misreports why a control is unavailable.

A control the operator can never use, because they are not an administrator, is absent rather than blocked (see [ADM](admin-access.md)).
