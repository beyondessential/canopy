# Direction A: the rule table

## What it is

One new page, Settings › Access › Access rules, placed second in the group after Admins. It is a flat table: type, matcher, scope, reach, where the rule is set, status. The add form sits above the table, as on Admins. The five existing pages are untouched.

A scope is the whole API or named parts: Status, Backups, Restore, Artifacts, Names, Identity tokens, plus MCP's Triage and Notes. Rules only grant; a principal holds the union of every matching rule. No deny, no ordering.

## The bet

The pain is that the tagged-device gate is one environment variable and the grant catalogue is compiled in. A table where an operator names a tag and picks what it may call fixes both, in one handler and one page.

## What it gives up

- A sixth page in an already fragmented section.
- Nothing about people. Admins stays a separate email list. The tailnet policy admin grant appears here read-only, as a row.
- Token grants now live in two places: legacy tokens are read-only here and edited on MCP access, while tokens added here are not on that page's mint form.
- No group or environment limit, and no suspend. "Scoped" means a named part of the API only.

## Files (in `.workhorse/design/mockups/p6/`)

- `a-access-rules.html`: 18 rows. Tailnet policy grant and environment tag gate read-only under "Set in" (1). `tag:canopy-test` matches nothing and says so. A revoked token beside live ones (5). Scoped rows throughout (6). "Not matched by any rule" lists `backup-runner-mel`, which has arrived with no permissions (2).
- `a-access-rule-add.html`: the add form with scope chosen, and a preview line showing what the new rule adds to devices already covered (the overlap case, at entry time).
- `a-access-rule-overlap.html`: `tag:backup-runner` expanded. Each device lists its other matching rules and its effective access, so overlap is visible as state.
- `a-access-rule-token-created.html`: a token added as a rule, secret in a one-time dialog (4).

## Unsure

- Placing the form above the table follows the real screens; the brief said below.
- Token rows: adding a token here duplicates minting on MCP access. A cleaner cut might drop the token matcher entirely.
- Built-in role rows summarise the route table; Names still needs per-application flags the table does not show.
- The environment row grants Whole API, which is my reading of how the tag gate behaves today, not something the inventory states.
- "Reaches" counts and the refused list need new queries (tag membership, refused-call log).
