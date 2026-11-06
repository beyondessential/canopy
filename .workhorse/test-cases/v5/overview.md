# Vulnerability tracking improvements: test cases

## Sources and exposure

- [x] The same npm package version in two repositories makes two items, each triaged on its own (verifies spec: VUL)
- [x] A fix landing on the default branch closes an item and keeps its status, decision and history (verifies spec: VUL)
- [x] A repository that cannot be read keeps its items, and a read that has not succeeded for a day raises the self-alert (verifies spec: VUL)
- [x] One unparseable lockfile leaves what is held for the repository and a good read clears the alert (verifies spec: VUL)
- [x] A repository is read again only when its head has moved (verifies spec: VUL)
- [x] Archiving or re-ranking a machine closes or opens its Linux item without a rescan (verifies spec: VUL)
- [x] Each package version is asked of OSV once, however many sources carry it (verifies spec: VUL)
- [x] Scopes come from npm, Cargo and uv lockfiles as the spec lists them (verifies spec: VUL)

## Triage

- [x] A split decision is refused when it leaves a workspace package uncovered, names one twice or one that does not reach the advisory, has two catch-alls or a catch-all that is not last, or splits a Linux item (verifies spec: VTR)
- [x] A split's status is the shared outcome, or mixed (verifies spec: VTR)
- [x] Withdrawing leaves the item needing triage with its suggestion; editing keeps the earlier text (verifies spec: VTR)
- [x] A lapse discards the suggestion; a new workspace package lapses a split without a catch-all and not one with it (verifies spec: VTR)
- [x] A revision raising severity or widening a range to what the source carries lapses a decision (verifies spec: VTR)
- [x] Decisions the model made are withdrawn on start, audited as Canopy's, and become suggestions with insufficient information (verifies spec: VTR)
- [x] The model never decides; an answer with no basis or an invalid split gives no suggestion; a valid split is stored (verifies spec: VTR)
- [x] A Linux item goes to the model as package and advisory only (verifies spec: VTR)

## List, search and counts

- [x] Search ranks a summary hit above a details-only hit and tolerates a typo and partial words (verifies spec: VUL)
- [x] Each facet ignores its own filter; every column sorts; a cursor pages without overlap (verifies spec: VUL)
- [x] Group by scope counts a multi-scope item in each scope (verifies spec: MCP)
- [x] The sidebar count leaves out closed and decided items (verifies spec: VUL)

## Interfaces

- [x] Recording a split, withdrawing and editing are audited as the operator's, admin-only, and refused to a read-only session (verifies spec: SAFE)
- [x] A batch records valid items and refuses invalid ones in order, auditing only what was recorded (verifies spec: MCP)
- [x] Withdraw and edit through a granted token, and a token without the grant is refused (verifies spec: MCP)
- [x] `fields` limits the output, details are opt-in, and `group_by` returns counts only (verifies spec: MCP)
- [x] The triage prompt is listed and takes a repository (verifies spec: MCP)
- [x] The sidebar badge, search with marks, filter counts, the closed switch, sorting, a split decision, edit and withdraw, and a lapsed banner in the browser (verifies spec: VUL)
