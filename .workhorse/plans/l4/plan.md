# Raise from a blocked control

## Tech notes

- The danger confirmation lives inside `SafetyModeControl` today. Raising from a blocked control needs the same dialog (plus the write variant and the action name) reachable from any graded control, so it moves into `SafetyModeProvider` behind something like `requestRaise(mode, actionName) => Promise<boolean>`. The mode control then uses it too, passing no action name.
- The action name becomes a required field of `Grading` in `GradedAction.tsx`, so the type checker finds every call site (178 `GradedAction` and 1 `GradedMenuItem` across 48 files). The hand-rolled `blockedSx` controls (`GroupInventorySection.tsx`) need the same wiring by hand.
- A blocked control is currently `disabled` and `pointer-events: none` inside a wrapper `<span>`. To be focusable and activatable it stops being `disabled`; the wrapper (or the control's own click/submit) intercepts activation, asks for the raise, then replays it. For a `type="submit"` save that has no `onClick`, replay means `form.requestSubmit()`, and the form's `onSubmit` path also has to intercept Enter in a field while the save is blocked. Simpler: intercept `click` in the capture phase on the wrapper and replay it once the raise has re-rendered the control as usable. Raising swaps the wrapper for the bare control, which is a new element, so the replay finds the control again by a `data-graded-action` marker the wrapper puts on it in both states rather than holding on to the old node. Implicit form submission (Enter in a field) fires a click at the form's default button when that button isn't `disabled`, so the same interception covers submit buttons, Enter in a field, links, and toggles without touching any form's `onSubmit`.
- `e2e/safety-modes.spec.ts` currently asserts that a blocked control does nothing and is out of keyboard reach; those tests invert.
- Danger reasons are declared on each danger handler's `routes!` entry next to its grade, carried through `openapi.json`, and generated into `safety-modes.ts` by `scripts/gen-safety-modes.mjs`, so the build check that rejects an ungraded handler also rejects a danger handler with no reason. The reasons are: cannot be undone, acts directly on servers, removes a protection, issues credentials, invalidates credentials. A control calling several danger endpoints shows the union of their reasons, joined with commas and a final "and".
- A control `disabled` for its own reason stays `disabled` and unstriped even when also below its grade. `GradedAction` today striped-and-disables everything blocked; it now has to tell the two apart (child's own `disabled` prop wins), and only intercept activation when the child isn't disabled.
- Calendar entries in `Upgrades.tsx` (~line 439) drop the "link to the group when amending is blocked" fallback and become blocked openers of the amend form like any other, so in read-only every open entry wears the write stripe.
- `MachineSetupInstructions` passes `disabled={mint.pending || minting.blocked}` to its re-mint button, naming the blocked state itself because the button sits inside a `Tooltip`. Under the own-reason rule that reads as "disabled for its own reason", so it has to drop `minting.blocked` and leave blocking to `GradedAction`. Auto-mint still waits while minting is blocked: the spec gives a raise only to an operator's activation, never to a page load.
- `GradedMenuItem`'s single caller (`MaintenanceSection.tsx`) closes the menu inside its own `onClick`. The menu has to close before the raise is asked for, so the item gets an `onCloseMenu` prop that it calls first, then asks for the raise, then calls `onClick`.

## Proposed danger reasons

Check each one against its handler when you declare it. Short names: `irreversible` (cannot be undone), `fleet` (acts directly on servers), `unprotects` (removes a protection), `issues` / `invalidates` (credentials).

| Handler | Reasons |
| --- | --- |
| `admins/add` | issues |
| `admins/delete` | invalidates |
| `kubernetes_clusters/register` | issues |
| `kubernetes_clusters/reissue` | issues, invalidates |
| `kubernetes_clusters/remove` | irreversible, invalidates |
| `domains/release` | unprotects (check: may be irreversible if another group claims it) |
| `mcp_tokens/mint` | issues |
| `mcp_tokens/revoke` | irreversible, invalidates |
| `healthchecks/set_source_ingest` | unprotects |
| `healthchecks/decommission` | unprotects |
| `backups/upsert` | issues, unprotects (retention floor opt-out) |
| `backups/clear_schedule` | unprotects |
| `backups/request_now` | fleet |
| `backups/disallow_restore` | unprotects |
| `backups/set_capability` | unprotects |
| `backups/delete` | irreversible, invalidates |
| `versions/delete_artifact` | irreversible |
| `silenced_refs/silence_{server,machine,cluster,group}` | unprotects |
| `restore_replicas/delete` | invalidates |
| `devices/disable_all_keys` | invalidates |
| `devices/update_role` | issues, invalidates |
| `devices/provision_credential` | issues |
| `devices/add_key` | issues |
| `devices/deactivate_key` | invalidates |
| `devices/reactivate_key` | issues |
| `devices/attach_tailscale` | issues |
| `devices/detach_tailscale` | invalidates |
| `devices/merge_into` | irreversible |
| `certificates/pause` | unprotects |
| `certificates/revoke` | fleet, invalidates |
| `machines/archive` | check: the doc comment doesn't say why it's danger |
| `machines/attach_tailscale_device` | issues |
| `machines/mint_enrollment` | issues |
| `machines/revoke_enrollment` | invalidates |

## Build steps

### Danger reasons on the server

- [x] `vendor/canopy-utoipa-axum/src/lib.rs`: add a `danger(reason, ...): handler` arm to `routes!`, with each reason ident mapped to its wire string by a helper arm, so an unknown reason fails to compile. Turn bare `danger: handler` into a `compile_error!` that names the reasons. Add `__set_danger_reasons`, which writes an `x-canopy-danger-reasons` string array next to `x-canopy-safety-mode`
- [x] Declare reasons on all 37 `routes!(danger: …)` entries under `crates/private-server/src/fns/`, per the table above
- [x] `crates/private-server/tests/it/openapi_spec.rs`: next to `every_operation_declares_a_safety_mode`, assert that every danger operation carries a non-empty list of known reasons and that no other operation carries any
- [x] `private-web/scripts/gen-safety-modes.mjs`: read the reasons, reject unknown ones and danger operations with none, and emit `DANGER_REASONS` (keyed like `SAFETY_MODES`) plus a `DangerReason` type
- [x] `just gen-openapi`, then commit `private-web/openapi.json` and `private-web/src/safety-modes.ts`. Check that the public server's `openapi.json` and `crates/canopy-api` are unchanged (`just check-generated`)

### Raise request in the provider

- [x] `private-web/src/safety.ts`: add each reason's operator wording, in the SAFE list order, and a joiner ("a, b and c")
- [x] New `RaiseDialog` component with two variants. The mode-control variant keeps the existing "Enter danger mode?" copy. The blocked-control variant is titled with the action name and shows "This action needs {mode} mode", plus ": {reasons}" for danger, with "Continue in {mode} mode" as its confirm. The danger variant keeps the `WarningAmberIcon` title. No `autoFocus` on the confirm, so focus lands on the dialog itself
- [x] `private-web/src/hooks/useSafetyMode.tsx`: the provider owns the dialog and exposes `requestRaise({ mode, action?, reasons? }): Promise<boolean>`. It resolves `true` once the raise has landed and the new mode has rendered, and `false` on cancel or failure. A second request while one is pending resolves `false` straight away. The "Mode unchanged" failure dialog moves here from `SafetyModeControl`
- [x] `SafetyModeControl.tsx`: drop its own dialogs and raise to danger through `requestRaise({ mode: "danger" })`. Raising to write still calls `raise` directly

### `GradedAction` and `GradedMenuItem`

- [x] Make `action: string` required on `Grading`
- [x] Add a `dangerReasons(grading)` helper: the union, in list order, of `DANGER_REASONS` over the endpoints whose grade equals the required mode
- [x] Blocked `GradedAction`: stop cloning `disabled: true` and drop `pointer-events: none` on the child. Keep the stripe and the "Requires … mode" tooltip, and use a pointer cursor instead of `not-allowed`. `aria-disabled` goes: the control is operable, and Playwright (like a screen reader) reads an `aria-disabled` ancestor as disabled. Intercept `onClickCapture` on the wrapper (`preventDefault` + `stopPropagation`), then `requestRaise`, and on `true` click the marked control again, and focus it if nothing took focus
- [x] Own-reason disabled: when the child, or the control inside a `Tooltip` child, has `disabled`, render it the way a usable control renders, with no stripe and no interception, whatever the mode
- [x] Blocked `GradedMenuItem`: call `onCloseMenu`, then `requestRaise`, then `onClick` on `true`. Update `MaintenanceSection.tsx`
- [x] Add a `useGradedActivation(grading)` hook returning `{ required, blocked, activate(run, action?) }` for the controls that can't take the wrapper; `action` overrides the grading's for a control standing for one of many
- [x] Rewrite the `GradedAction` and `GradedMenuItem` doc comments that say raising is never a by-product of a blocked control

### Hand-wired controls

- [x] `GroupInventorySection.tsx`: wire the remove chip through `useGradedActivation`
- [x] `Upgrades.tsx` calendar: drop `!amending.blocked` from `editor`. Entries that aren't done get the blocked treatment and open the amend form through `useGradedActivation`. Done entries keep their link to the group. Update the comment above `editor`
- [x] `MachineSetupInstructions.tsx`: remove `minting.blocked` from the re-mint button's `disabled`. Leave the auto-mint effect gated on `minting.blocked`

### Action names at every graded control

Name each action with its object taken from the data in scope, such as "Revoke certificate for {hostname}".

- [x] components: AddPublicKeyDialog (1), ChecksTable (3), DeclareMaintenanceDialog (2), GroupDomainsSection (2), GroupInventorySection (2), IssueRow (10), MachineBackupSection (3), MachineDnsNamesSection (4), MachineIdentitySection (2), MachineSetupInstructions (4), MaintenanceSection (8), ManualEventButton (2), ManualEventForm (1), NotesList (3), ProvisionCredentialDialog (1), ReportingSchemasSection (1), RestoreReplicasSection (6), ServerCertificatesSection (8), SilencedRefsSection (1), TailnetIdentitySection (6)
- [x] routes, A to G: Admins (2), ArchivedList (2), BackupConfig (3), BackupDefaults (1), BackupPanel (14), BestoolSnippetDetail (4), BestoolSnippets (2), ClusterEdit (1), DeviceDetail (8), DevicesList (1), GroupDetail (5), GroupEdit (3), GroupsList (1)
- [x] routes, H to V: HealthcheckSettings (11), Healthchecks (2), IncidentDetail (4), KubernetesClusters (6), MachineCreate (1), MachineDetail (1), MachineEdit (1), Maintenance (2), McpTokens (3), RecoveryVault (2), SelfAlerts (1), ServerDetail (4), SourcesSettings (3), Upgrades (7), VersionDetail (14)

### Playwright coverage (`private-web/e2e/safety-modes.spec.ts`)

- [x] Invert "a control above the mode is present and does not act": clicking it offers the raise, cancelling leaves it as it was, and confirming raises to write and carries the action out
- [x] Invert "a blocked control is out of reach of the keyboard too": Tab reaches it, Enter offers the raise, and Enter in a field of a form whose save is blocked offers the raise too
- [x] A blocked danger control in read-only and in write raises straight to danger, its title names the action, and its reasons are shown
- [x] A blocked opener raises and then opens its form, and a control with its own confirmation shows it after the raise
- [x] The form keeps what was typed through cancel and through confirm
- [x] A failed raise (route the raise request to a 500) shows "Mode unchanged" and doesn't carry the action out
- [x] A double click on a blocked control carries the action out once
- [x] A blocked menu item closes the menu, then asks for the raise
- [x] Extend "a control disabled for a reason of its own carries no stripe": it offers no raise below its grade either
- [x] Calendar: in read-only an open entry wears the write stripe, and clicking it raises and opens the amend form
- [x] Update "machine setup below danger offers its ticket blocked" for the raise flow

- [x] `private-openapi-dump` writes through a `serde_json::Value`, so `openapi.json` comes out in one order: utoipa keeps an operation's extensions in a `HashMap`, and with two per danger operation the direct dump reordered them every run

### Verify

- [x] `just typecheck`, `just lint`, `cargo fmt`
- [x] `openapi_spec` and `safety_modes` tests in private-server, the canopy-utoipa-axum tests, and `just check-generated`
- [x] `e2e/safety-modes.spec.ts` and `e2e/upgrades.spec.ts` locally; the full e2e suite is left to CI
