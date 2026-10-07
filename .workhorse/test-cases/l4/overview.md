# Raise from a blocked control

Unless a case says otherwise, start each one from a fresh page load as an administrator, which means read-only.

## Raising from a blocked write control

- [ ] On a group's page, click the blocked Edit link: a dialog titled "Edit group {name}" says "This action needs write mode", and the mode is still read-only (verifies spec: SAFE)
- [ ] In that dialog, choose "Continue in write mode": the mode control shows write with a fresh ten-minute countdown, and the group edit form opens (verifies spec: SAFE)
- [ ] Click the blocked Edit link again and choose Cancel: the mode stays read-only, the page doesn't change, and the link is still blocked (verifies spec: SAFE)
- [ ] A raise made from a blocked control counts down from ten minutes, as one made from the mode control does, and the session returns to read-only when it runs out (verifies spec: SAFE)

## Raising from a blocked danger control

- [ ] On the Admins page, click the blocked "Add admin": the dialog is titled with the action, says it needs danger mode, gives its reason ("it issues credentials"), and its confirm reads "Continue in danger mode" (verifies spec: SAFE)
- [ ] Raise to write from the mode control, then click a blocked danger control: the dialog asks for danger, not write, and confirming lands on danger (verifies spec: SAFE)
- [ ] On a server's certificates section, click the blocked revoke: the reasons read "it acts directly on servers and invalidates credentials", joined with "and" (verifies spec: SAFE)
- [ ] Confirm the raise for a control with a confirmation of its own, such as revoking a certificate: the action's own confirmation still appears after the raise, and cancelling it leaves the session in danger with nothing revoked (verifies spec: SAFE)
- [ ] On a server's certificates section, click the blocked Pause, which opens a dialog: confirming raises to danger and opens the pause dialog, and nothing is paused until that dialog is submitted (verifies spec: SAFE)
- [ ] The danger dialog's title has the same warning icon as the mode control's "Enter danger mode?" dialog

## Failures and repeats

- [ ] Make the raise request fail (block `/api/safety/raise` or return a 500): after confirming, "Mode unchanged" is shown, the mode is still read-only, and the action isn't carried out (verifies spec: SAFE)
- [ ] Double-click a blocked "Add admin" with an email filled in, then confirm: one dialog appears, and exactly one admin entry is added (verifies spec: SAFE)
- [ ] Press Enter twice quickly on a focused blocked control: one dialog appears, and the action runs once after confirming (verifies spec: SAFE)

## Forms

- [ ] On the Admins page, type an email, click the blocked "Add admin" and cancel: the email is still in the field (verifies spec: SAFE)
- [ ] Type an email, click the blocked "Add admin" and confirm: the email is added and appears in the list (verifies spec: SAFE)
- [ ] Type an email, make the raise fail, and dismiss "Mode unchanged": the email is still in the field (verifies spec: SAFE)
- [ ] On a form whose save needs write for some inputs and danger for others, fill it so the submission needs danger, then click the blocked save: the dialog asks for danger mode and gives the danger reasons (verifies spec: SAFE)
- [ ] Fill the same form so the submission needs only write, then click the blocked save: the dialog asks for write mode (verifies spec: SAFE)

## Keyboard

- [ ] Tab through the Admins page in read-only: focus reaches the blocked "Add admin" (verifies spec: SAFE)
- [ ] With the blocked "Add admin" focused, press Enter: the raise dialog opens, just as a click opens it (verifies spec: SAFE)
- [ ] With the blocked "Add admin" focused, press Space: the raise dialog opens, and the Space keyup doesn't confirm it (verifies spec: SAFE)
- [ ] Type an email in the Admins form and press Enter in the field: the raise dialog opens, and confirming adds the email (verifies spec: SAFE)
- [ ] When the raise dialog opens from the keyboard, focus is inside the dialog but not on "Continue in … mode" (verifies spec: SAFE)
- [ ] Cancel the raise dialog with Escape: focus returns to the blocked control (verifies spec: SAFE)
- [ ] Confirm a raise for an opener such as Pause on a server's certificates section: focus ends up in the opened dialog, not back on the control behind it (verifies spec: SAFE)

## Controls disabled for their own reason

- [ ] In read-only, on a form whose save is disabled because it is incomplete: the save has no stripe, clicking it does nothing, and no raise dialog appears (verifies spec: SAFE)
- [ ] Complete that form, still in read-only: the save gets its grade's stripe, and clicking it offers the raise (verifies spec: SAFE)
- [ ] While a request from a usable control is in flight, its disabled button carries no stripe and offers no raise (verifies spec: SAFE)

## Menus and hand-wired controls

- [ ] On a group's maintenance section, open the declare menu and choose a blocked environment: the menu closes first, then the raise dialog appears, and confirming opens the declare dialog for that environment (verifies spec: SAFE)
- [ ] On a group's inventory section, click a blocked remove chip: the raise dialog names the variable being removed, and confirming removes it (verifies spec: SAFE)
- [ ] On the upgrades calendar in read-only, an open plan entry wears the write stripe, and clicking it offers the raise, then opens that plan's amend form (verifies spec: SAFE, UPG)
- [ ] On the upgrades calendar, an entry for a met plan still links to its group in every mode
- [ ] In machine setup in read-only, the ticket isn't minted on load, the re-mint button wears the danger stripe, and clicking it raises to danger and then mints (verifies spec: SAFE)

## Action names

- [ ] Each blocked control's raise dialog title names its action and its object, such as "Revoke certificate for host-3", not just the button's visible verb (verifies spec: SAFE)
- [ ] The blocked control's tooltip still says "Requires {mode} mode" (verifies spec: SAFE)

## Mode control

- [ ] Raising to write from the mode control takes effect with no dialog (verifies spec: SAFE)
- [ ] Raising to danger from the mode control still shows "Enter danger mode?", with its existing copy and no action name (verifies spec: SAFE)
- [ ] Lowering from the mode control still works straight after a raise made from a blocked control (verifies spec: SAFE)

## Server grading

- [ ] Every danger operation in the private server's OpenAPI document declares at least one known reason, and no other operation declares any (verifies spec: SAFE)
- [ ] A `routes!(danger: handler)` entry with no reasons fails to compile, and so does one naming an unknown reason (verifies spec: SAFE)
- [ ] `just check-generated` passes, and the public server's `openapi.json` and `crates/canopy-api` are unchanged by the reason declarations

## Regression

- [ ] A control withheld from a non-administrator is still absent, not blocked or raisable (verifies spec: SAFE, ADM)
- [ ] A raise that lapses on the server mid-action: the next refused request returns the indicator to read-only and says the raise has lapsed (verifies spec: SAFE)
- [ ] Spot-check one blocked control on each top-level page (fleet, groups, backups, upgrades, settings) for the raise dialog with a sensible action name
