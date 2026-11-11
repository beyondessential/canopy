# Design loop — P6, auth control plane

Rigour: Standard. Round cap 6. Stop rule: two clean rounds, the second by fresh eyes, no criticals and few suggestions.

## Where the run is

Step 2, direction. Round 1 closed with a direction chosen (see the taste ledger). Round 2 built and fixed the hybrid. Round 3's fresh-eyes critique found it not clean, and its fixes are round 4's to judge. The card has no specs, no plan and no commits, so there is no build to polish. Step 3 waits on an implementation.

## Step 0 — does the card need design

Yes. The card's third goal is a control plane over human and machine identity with least-privilege permissions, and its first is choosing at runtime which tagged devices may call which parts of the Canopy API. Neither has a pattern in the app: Settings › Access is five separate pages (Admins, MCP access, Certificate authority, Identity provider, Audit log), each covering one credential kind, and none of them grants a scoped permission to a principal. A new surface, a new layout and controls with no precedent — design.

## The method as run here

The Workhorse version in this repo ships `card-context`, `spec-format`, `test-quality` and `working-doc-format` under `.agents/docs/`, but not the `design-judgement`, `running-loops` or `browser-rig` documents the skill cites. The method is therefore taken from the skill body itself, and the rig built from what the repo has.

## The rig

The Playwright e2e fixture is the rig: it spawns a private-server and Vite pair against a throwaway Postgres, so a capture needs no running dev server. The capture harness lives at `capture/` beside this log rather than in `private-web/e2e/`, so it is never mistaken for a test, and writes to `shots/current/`.

    cd private-web && env -u PORT npx playwright test \
      --config ../.workhorse/plans/p6/design/capture/playwright.config.ts

`env -u PORT` is required under a Workhorse local agent: the fixture's clap parsing conflicts with an inherited `PORT`.

## Step 1 — references

The app's own shipped surfaces are the consistency reference, captured in `shots/current/`: the five Access pages, restore consumers, the device listing, search and detail. There is no `.workhorse/design/` in this repository, so there is no design system document or snapshot to start from; the MUI composition in `private-web/src/routes/` is the baseline.

The user named the references (2026-11-10), for their models rather than their screens unless noted:

- Tailscale's grants, as a data model. Its UI is not the reference.
- Tamanu's permission system: verb on noun with an optional object id, attached to roles, allow-only, edited as a matrix.
- Firewall rule tables, for expressiveness that stays human-parseable. iptables syntax is the cautionary tale.
- Tailscale's and crates.io's trust credentials UIs. These are UI references as well as model references.
- AWS IAM for the API and model. Its UI is the cautionary tale.

They are written into `direction-brief.md`, with what each is taken for.

## Step 2 — three directions

### Scope decided by the user

Design for the full unified control plane (rung 3), then judge what is realistic and useful from there. The first round's spread across the scope ladder is withdrawn. Of the old directions:

- A's rung-1 rule table is kept in `superseded/` and `superseded-direction-a-rung1.md` for the record, out of the critique.
- B left nothing.
- C's partial principals screen is handed to its successor.

### Fleet noise decided by the user

Default views and filters leave out the authentication of fleet machines, which are the boxes enrolled in groups. That traffic would drown everything else. It stays fully in the model, appears in any "who can do this" answer, and is one step away as a count or a filter. A fleet machine behaving unlike the fleet still surfaces by default. This is written into the brief as a rule binding every direction.

### The round as re-run

Three structurally different shapes of the same rung-3 control plane, each organised around a different primary entity:

- **P, principals first.** One list of every principal, and a detail page with its credentials and effective permissions. The model comes from IAM, the credential shapes from trust credentials.
- **G, grants first.** One ordered table of grants, each readable as a sentence, with hit counts and last match. The model comes from Tailscale grants and firewall tables, the evaluation from IAM. Principals are secondary.
- **R, roles first.** Named roles as bundles of verb-on-noun permissions, edited as a matrix. Principals are members of roles, or are trusted into them by class. The model comes from Tamanu and IAM assumption. The fleet is one built-in role, collapsed.

There are ten hard cases. The first six are carried over:

1. A policy-granted admin Canopy cannot edit.
2. A principal with no permissions.
3. A list long enough to test density.
4. A secret shown once.
5. Something revoked beside live entries.
6. A scoped permission.

The four new ones:

7. The fleet held back, with its drill-down.
8. A trust credential for a releaser.
9. Taking access away in a hurry, by deny or otherwise.
10. A "who can do this" answer that names each principal and its source.

## In flight

All three directions are built: five mockups each in `.workhorse/design/mockups/p6/`, write-ups in `direction-{p,g,r}.md`, renders in `shots/r1/` and downscaled copies in `shots/r1-small/`. None of the three agents could view images, so each checked its own work by DOM geometry only.

The fresh-eyes critic could not run: every spawn (Opus, Opus inherited, Fable) failed at start with "prompt too long", even with downscaled renders, so the cause is the subagent's starting context in this session, not the images. The user accepted the main session's read as round 1's critique (2026-11-11).

The brief is wrong about the chrome: at 1440px the app has a permanent left sidebar with the safety-mode band at its foot, not a top bar. G and R followed the React source. The brief should say so before any later round.

## Round 2 — the hybrid

Direction H, seven mockups in `h-*.html`, built by the main session with `build/build_h.py` after the build agent again failed to start. Write-up in `direction-h.md`, renders in `shots/r2/`.

The main session's critique found 2 criticals, 7 suggestions and 4 nits, filed on the card:

- Credential names drifted between screens ("Client certificate", "enrolled key", "Tailnet tag"). Now "Enrolled key" throughout, with the tag as the key's qualifier; only the refused unknown caller reads "Tailnet node".
- The Suspend dialog listed fewer permissions than the roles grant. It now lists all of them.
- Principal kinds that echoed role names (Releaser, Restore consumer, Relay) are now CI workflow and Service.
- Rows needing attention sort first, the new principal appears behind the minted-token dialog, both principal pages share one Credentials table, scope chips omit "all groups" everywhere, and the data mismatches (a nine-machine "exception", a suspended role with an unlisted member, the releaser's read scope) are reconciled.

Twelve fixed in one pass. The Restore consumers sidebar entry was skipped: it is the operational roster of registered intents, and how it points at the restore-consumer role is for the specs.

Round 2 is not clean, so it does not count towards the stop rule.

## Round 3 — fresh eyes

Five critics, none of which had seen the conversation, ran in parallel: four took one or two screens each with the renders in `shots/r3/`, and one took all seven as text to check that the screens describe one world. They started, where round 1's could not, because each was handed text extracts of the mockups rather than the CSS-heavy HTML.

They found 6 criticals, 16 suggestions and 6 nits, filed on the card with the trust decision below as 28 issues, plus one reopened from round 2:

- Suspended state does not show: the strike applies only to a chip's scope, so `admin` and `deploy-automation` look live. And suspending `deploy-automation` cuts nothing, since its only member holds `admin` directly.
- About twenty role members appear on no principal screen, while the fleet row claims the rest hold only `fleet-machine`.
- Role summaries use verbs, nouns and scopes the matrix cannot express (triage, identity tokens, status; software, cluster, application).
- A role held through a trust offers removal on the principal page, and who-can gives no reason for a suspension.
- The rest is drift: Last matched holding refusals and suspensions, column names, the fleet row reading three ways, sort order, members counting, the token form, date formats.

The critics' view on the open questions: nothing drawn needs two scopes on one noun, since a second role covers it as `operator` and `maintenance-planner` already do; and no one objected to a suspended policy admin reading Suspended first.

Round 3 is not clean, so it does not count towards the stop rule.

## Next

- Fix round 3's issues in `build/build_h.py`, re-render into `shots/r4/`, and bring `direction-h.md` up to date.
- Round 4: fresh eyes again, run the same way.
- Then the open questions below, what to build first, and specs.

## Open questions

- Carried from `direction-h.md`: whether a suspended policy admin reads Suspended or Policy first; whether list rows say what last matched; how a role says Read everywhere and Consume in its own group on the same noun.

- What is realistic to build. This is judged after the direction is refined, not before.
- Whether Canopy can resolve tailnet group membership. Role trusts by tailnet group depend on it; the inventory does not say it can.

## Taste ledger

| Call | Reason | Before / after | Verdict |
|---|---|---|---|
| Withdraw the scope-ladder spread (A rung 1, B rung 2, C rung 3) | The user fixed scope at rung 3, so the spread no longer answers a live question | `superseded/a-*.html` / `p-`, `g-`, `r-` | Withdrawn by user decision |
| Organise the three directions by primary entity (principal, grant, role) | Each reference implies a different home entity, and that choice drives every screen | n/a | Adopted for round 1 |
| Fleet machines out of default views, one step away | User decision: they would add most of the noise | n/a | Binding on all directions |
| Take P and R forward as a hybrid: principals list as home, roles as the permission unit, borrowing G's last-matched column and checker | P is strongest at "what can this principal do" and at replacing the five Access pages; R keeps permissions auditable as a few named shapes; G's table drifts towards precedence puzzles ("overrides g16") | `p-*`, `r-*`, `g-*` / round 2 | User decision, 2026-11-11 |
| Suspend, not an explicit deny | Allow-only keeps "who can" a lookup with no precedence to show | G's deny rows / Suspend on principal or role | User decision, 2026-11-11 |
| A Canopy Suspend overrides a tailnet-policy admin | An emergency cut-off must not wait on a policy-file edit | P and R as drawn | User decision, 2026-11-11 |
| restore-consumer and backup-verifier are trusted by tailnet tag | Restore operators are tailnet nodes with no key of their own; round 2's "Enrolled key throughout" had overreached onto them | Enrolled key tagged `tag:restore-consumer` / Tailnet tag trust | User decision, 2026-11-11 |
| A fourth scope: one named record of the noun | Tamanu's optional object id; keeps the releaser to publishing one piece of software, and makes a cluster or an application a scope | All groups, own group, a named group or environment / plus a named record | User decision, 2026-11-11 |

## Differences from the reference

_None recorded yet. The app's own surfaces are the reference and the directions are briefed to match them._

## Feature backlog set aside by the loop

_Empty._
