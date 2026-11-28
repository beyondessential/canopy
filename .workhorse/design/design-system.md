---
title: Canopy Design System
---
# Canopy Design System

Canopy's admin UI is where an operator, often on call, finds out what in the fleet is wrong and acts on it. It is read at a glance and worked in under pressure, so it is dense, quiet and consistent: colour is spent on state, surfaces are flat and outlined, and the same thing is drawn the same way on every page. The product's records are the content; the chrome stays grey and out of the way.

The UI is MUI (`@mui/material`) with a deliberately thin theme (`private-web/src/theme.ts`). Almost everything below is MUI's defaults plus conventions held in the components, so the rule for new UI is: use the MUI component and the palette key, and copy the nearest existing component's treatment before inventing one.

## Colour

### Theme

Light and dark follow the operating system (`prefers-color-scheme`, `private-web/src/main.tsx`). Only `primary` and `secondary` are set; every other colour is MUI's default for the mode.

| Palette key | Used for |
|---|---|
| `primary` | The brand wordmark, primary (contained) buttons, the active sub-page, links in rendered Markdown |
| `secondary` | The current sidebar section, a version 2 to 4 behind latest, the Senaite application type |
| `success` | Passing, healthy, current |
| `warning` | Needs attention soon; write mode |
| `error` | Down, failing, production; danger mode |
| `info` | Neutral notice; lingering incidents; demo and test ranks |
| `text.secondary` | Labels, metadata, empty states, muted trail segments |
| `text.disabled` | Never reported, unknown |
| `divider` | Every hairline: outlined surfaces, rows, gridlines |
| `action.hover` | Current row, operator-present group cards, code backgrounds |

Read colours through palette keys (`color="warning"`, `sx={{ color: "text.secondary" }}`, `theme.palette.error.main`), never a hex, so both modes work without a second value. The values below are what those keys resolve to, for reference.

### Light

| Token | Value |
|---|---|
| `primary` | `#388e3c` |
| `secondary` | `#32669a` |
| `success` | `#2e7d32` |
| `warning` | `#ed6c02` |
| `error` | `#d32f2f` |
| `info` | `#0288d1` |
| `text.primary` | `rgba(0, 0, 0, 0.87)` |
| `text.secondary` | `rgba(0, 0, 0, 0.6)` |
| `text.disabled` | `rgba(0, 0, 0, 0.38)` |
| `divider` | `rgba(0, 0, 0, 0.12)` |
| `action.hover` | `rgba(0, 0, 0, 0.04)` |
| `background.default` | `#ffffff` |
| `grey[100]` | `#f5f5f5` |

### Dark

| Token | Value |
|---|---|
| `primary` | `#66bb6a` |
| `secondary` | `#5b8cbe` |
| `success` | `#66bb6a` |
| `warning` | `#ffa726` |
| `error` | `#f44336` |
| `info` | `#29b6f6` |
| `text.primary` | `#ffffff` |
| `text.secondary` | `rgba(255, 255, 255, 0.7)` |
| `text.disabled` | `rgba(255, 255, 255, 0.5)` |
| `divider` | `rgba(255, 255, 255, 0.12)` |
| `action.hover` | `rgba(255, 255, 255, 0.08)` |
| `background.default` | `#121212` |
| `grey[900]` | `#212121` |

### Chrome

The sidebar (and the app bar on narrow screens) is drawn in `grey[100]` in light mode and `grey[900]` in dark, so the frame reads as one piece of chrome against the white or `#121212` page. Content never takes this grey.

### What colour means

Colour carries state and nothing else. Each meaning has one colour everywhere it appears.

| Subject | Mapping | Where |
|---|---|---|
| Check result | failed `error`, warning `warning`, passed `success`, skipped `default` | `CheckResultChip` |
| Broken check | hazard tape: `warning.main` and `grey[900]` stripes at -45°, white bold label | `CheckResultChip` |
| Application dot | healthy `success.main`, warning `rgb(132, 223, 91)`, down or unhealthy `error.main`, never reported `text.disabled` | `StatusDot` |
| Machine enclosure | fine `divider` outline, degraded `warning.main`, down `error.main` (each with a faint wash), never reported dotted `text.primary` | `MachineEnclosure`, `MachineMark` |
| Health chip | filled `success` / `warning` / `error`; stale is outlined `warning`; unmonitored or maintained fades to half opacity | `HealthChip` |
| Incident loudness | loud `error`, held `warning`, lingering `info` | `IncidentCard`, `IncidentDetail`, `Status` |
| Safety mode | read-only neutral, write `warning`, danger `error` | `GradedAction.modePalette`, `SafetyModeControl` |
| Rank | production `error`, clone `warning`, demo and test `info`, dev `success` | `ServerRankChip` |
| Version distance | under 2 behind `success`, 2 to 4 `secondary`, 5 to 9 `warning`, 10 or more `error`, unknown `text.disabled` | `VersionSquare` |
| Version status | published `success`, draft `warning`, yanked `error` | `VersionStatusChip` |
| Vulnerability severity | critical `error`, high `warning`, medium `info`, low and unrated `default` | `VulnerabilityChips` |
| Certificate | revoked, expiring or failed `error`; due for renewal `warning`; valid outlined `success` | `CertificateStateChip`, `TimeLeft` |
| Application type | Tamanu central and facility `success`, Senaite `secondary`, others `default` | `ApplicationTypeChip` |
| Device role | machine and backup-restore `primary`, releaser `warning`, admin and relay `info` | `DeviceShorty` |
| Sidebar count | open incidents `error.main`, vulnerabilities needing triage `warning.main` | `IncidentCount` in `AppSidebar` |

> [!NOTE]
> A warning dot is a near neighbour of the healthy green on purpose: a warning is "could be better", so it reads as a shade of fine and never competes with a red. Yellow stays out of the dot ramp because the striped broken state owns it.

> [!NOTE]
> Red on both grains is deliberate: a red dot in a neutral pill is one application failing on a healthy machine; a red pill is the machine.

### Patterns that carry meaning

- **Mode stripe.** A raised safety mode is a repeating diagonal stripe of its palette's `light` colour (24% stripe, 7% gap, 6px bands): 135° for write, 45° for danger, so the two read apart by angle alone (`modeStripe` in `GradedAction.tsx`). The mode control wears it while raised, each raised mode wears it in the mode menu, and a control blocked by the current mode wears it greyed (`grayscale(.8)`), regaining colour on hover.
- **Maintenance.** Work under way moves and a target settling after a window is still. A maintained application dot hollows to a 2px ring and pulses (opacity 1 to 0.4 over 2s); a maintained machine enclosure is muted and carries a slow wave of `alpha(text.primary, .08)`; a group under maintenance is hatched at 45°.
- **Unmonitored.** A diagonal band is masked out of the dot, so whatever surface is behind shows through.

### Tints

A tint is `alpha(palette.X.main or .light, small)`, never a hand-mixed colour. Tints mark state (a draft or yanked version row, a degraded enclosure, an operator-present group card) and are never decoration.

## Typography

- **Font.** No web font is loaded. The stack is MUI's default, `"Roboto", "Helvetica", "Arial", sans-serif`, so most operators see Helvetica or Arial. Do not add a font.
- **Monospace.** `fontFamily: "monospace"` for anything an operator might copy or compare character by character: ids, hashes, keys, snapshot ids, check names, version strings in titles, `key=value` tags, and SQL results.
- **Numbers that tick** (countdowns, live figures) use `fontVariantNumeric: "tabular-nums"`.

| Element | Variant | Size | Weight | Used for |
|---|---|---|---|---|
| Page title | `h4` (as `h1`) | 2.125rem | 400 | Page title on list and detail pages |
| Form page title | `h5` (as `h1` or `h2`) | 1.5rem | 400 | Page title on form and configuration pages; a free-standing section heading |
| Section heading | `h6` (as `h2`) | 1.25rem | 500 | Section heading inside a surface; the brand wordmark |
| Sub-heading | `subtitle2` | 0.875rem | 500 | Sub-heading within a section |
| Body | `body2` | 0.875rem | 400 | The default for content: rows, facts, table cells, legends |
| Caption | `caption` | 0.75rem | 400 | Fact labels, timestamps, secondary lines |
| Sidebar group | `overline` | 0.6875rem | 500 | Settings group headings in the sidebar, at a size and weight set there |

`body2` is the working size; `body1` is rare. Secondary text is `color="text.secondary"`, not a smaller size.

## Spacing and shape

MUI defaults: an 8px spacing unit and a 4px radius (`borderRadius: 1`). There are no theme overrides.

### Spacing

| Token | Value | Usage |
|---|---|---|
| `spacing(0.5)` | 4px | Tight inline items |
| `spacing(1)` | 8px | Rows and inline items |
| `spacing(1.5)` | 12px | A compact row's padding (`p: 1.5`); a detail page's header stack |
| `spacing(2)` | 16px | Groups within a section; a surface's padding (`p: 2`); grid gaps |
| `spacing(3)` | 24px | Page sections (`Stack spacing={3}`); the content's vertical padding |
| `spacing(4)` | 32px | Between facts in a facts row |

### Radius

Radii beyond the default are for shapes that mean something.

| Token | Value | Usage |
|---|---|---|
| default radius | 4px | Surfaces, shorties, buttons, chips |
| count badge radius | 10px | Count badges |
| pill radius | 999px | Machine enclosures |
| round radius | 50% | Dots and avatars |
| square radius | 0 | The mode band |

## Surfaces

- Content sits on `Paper variant="outlined"`: a 1px `divider` border, the default radius, no shadow. Elevation is not used for content; the only shadow is the narrow-screen app bar's.
- One level of surface. A section is an outlined Paper on the page; rows inside it are separated by space or a hairline, not by another box. Rows that are themselves links (the shorties below) are the one place a bordered row sits in a list.
- An open incident's header surface takes a 2px border in its loudness colour; once closed it returns to the 1px divider.

## Layout

### Shell

- At `md` and above: a permanent sidebar 240px wide, in the chrome grey, and no app bar. The brand (favicon at 24px and "Canopy" in `h6` `primary.main`) sits at its top, the navigation scrolls, and the safety-mode band sits at its foot.
- Below `md`: a dense app bar (`color="default"`) with the menu button, the brand, an Incidents link with its count (kept out of the drawer so an open incident is never hidden), and the mode control; the sidebar becomes a temporary drawer 256px wide.
- Banners (`AdminProbeBanner`, `SelfAlertsBanner`) sit above the content, `px: 3, pt: 2`, as MUI `Alert`s.
- Content is `Container maxWidth="lg"` with `py: 3`.

### Navigation

- Top-level sections are rows in `text.secondary`, weight 500; the current section turns `secondary.main`.
- Sections with sub-pages expand and collapse; sub-pages are a dense, indented list whose active item takes MUI's `selected` background and `primary.main` text.
- Settings groups its sub-pages under overline headings (Access, Monitoring, Backups).
- External links end in an `OpenInNew` icon.

### Page anatomy

- **List page**: `h4` title, then a filter bar in an outlined Paper (small switches and selects, `minWidth: 200`, refresh pushed right), then the records. Records are shorties in a `Stack spacing={1}`, cards in a responsive grid (1 to 4 columns, `gap: 2`), or a small table.
- **Detail page**: a header `Stack spacing={1.5}`: chips (rank, type) then the `h4` title as a trail, and below it a wrapping row of action buttons. Then outlined sections. Machine and application detail pages are laid out identically.
- **Trails** name a record by its ancestors: muted `text.secondary` segments joined by ` · `, the record itself in full colour (`TargetName`).
- **Facts** are a row of label-over-value pairs: a `caption` label in `text.secondary` over a `body2` value, monospace where it is an identifier, spaced `spacing={4}`.
- Settings pages often have no page title and start with their sections.

## Components

### Chips

- Always `size="small"`.
- Outlined for a category (rank, type, role, version status, severity); filled for a live state (health, check result).
- Labels are lower case where they are a state ("broken", "unmonitored", "pending"); rank and kind chips are capitalised with CSS from their stored value.

### Shorties

A shorty is the one-line link to a record used in lists (`ServerShorty`, `GroupShorty`, `DeviceShorty`): a row with `p: 1.5`, a 1px `divider` border and the default radius; the name as a weight-500 link with `underline="hover"`, its chips after it, and secondary facts pushed right with `ml: "auto"`. The row for the record already on screen takes `action.hover`.

### Status marks

- `StatusDot` is one application's state; `MachineEnclosure` is a pill around a machine's dots carrying the machine's own reachability; `MachineMark` is a lone solid circle of the same size for a machine with no applications.
- `VersionSquare` is a small rounded square beside a version, coloured by distance from latest, with the distance in a tooltip.

### Tables

`Table size="small"`, inside an outlined Paper where it is the section's whole content. `hover` only where the row is a link. Many lists are shorties rather than tables; use a table when the columns are compared down the page.

### Buttons and actions

- `contained` for the one primary action of a page or dialog; `outlined` for secondary; `text` for Cancel and tertiary actions. `size="small"` in headers, rows and sections.
- Detail-page actions are `ActionButton`s: outlined small buttons showing only their icon until hovered or focused, when the label expands. The label is always the accessible name.
- An action that needs a safety mode is wrapped in `GradedAction`: drawn in its mode's palette when usable, greyed with the mode stripe and the tooltip "Requires write mode" / "Requires danger mode" when blocked.
- A label that opens a form ends in an ellipsis ("Resolve…"); an action in progress says so with one ("Saving…").

### Dialogs

- `maxWidth="sm" fullWidth` by default; `md` only for wide content.
- `DialogActions`: text "Cancel" first, the contained primary action last, on the right.
- A destructive confirmation's title is the question ("Archive machine?"), with the record's name in bold in the content. Its confirm button takes the action's mode colour, or `error`.
- An error from the action appears as an `Alert` (`mt: 2`) inside the content.

### Loading, error and empty

- Loading is `LinearProgress`; a spinner only inside a button or a small inline slot.
- An error is `<Alert severity="error">` with the server's message.
- Empty is a short line: an `info` Alert ("No devices found."), a `success` Alert where empty is good news ("No open incidents."), or inline `body2` in `text.secondary` inside a section. No illustrations.

### Time

Relative and compact ("5m ago", "in 2h", "now") via `TimeAgo`, refreshed as it ages, with the absolute time and zone in a tooltip.

### People

`UserAvatar` is 24px initials on a colour hashed from the login (`hsl(hash, 45%, 38%)`, white text) or their profile picture; automation gets an icon on `action.selected`. Several operators are an `AvatarGroup` of 28px avatars, at most eight.

### Icons

`@mui/icons-material`, the default (filled) set. An icon marks a type or replaces a word (the action row, health chips, the danger title); it does not repeat a label beside it.

### Rich content

- Markdown (`Markdown.tsx`): code and `pre` on `action.hover`, links `primary.main` underlined, blockquotes with a 3px `divider` rule.
- SQL and Ansible scopes are edited in CodeMirror with line numbers, in a light or dark theme to match the mode. SQL results colour values by type: null `text.secondary`, boolean `info`, number `primary`, string `success`, array `warning`.
- Charts are hand-drawn SVG, not a charting library (`RunThroughputChart`): one measure per chart, a 2px series line with a 10% area fill, `divider` gridlines, 10px `text.secondary` ticks, the unit stated once, and the readout on hover in an outlined Paper. The series green is stepped per mode (`#388e3c` light, `#43a047` dark) because the dark primary is too light to read as a mark.

## Motion

Nothing moves unless something is under way. The maintenance pulse and wave (2s cycles) are the only continuous motion; everything else is MUI's default transitions.

## Copy

- Sentence case for page titles, section headings, buttons and navigation.
- Name things in the fleet's terms: group, environment, rank, machine, application. An installation of Canopy is "the Canopy instance". Ansible's variables and runs are "Ansible", not "inventory".
- A trail separator is ` · `.
- Microcopy is a few words. A problem is shown as a failed check or an issue, not as a reassuring banner when all is well.

## Drift

These exist in the shipped UI and are not to be copied into new work.

- Title case in a few labels: "Server Versions", "Delete Snippet", "Extra Data", "PSQL Snippets", "SQL Playground".
- "server" where the subject is an application or a machine ("N servers" in `GroupShorty`, "Server reports OK" in `HealthChip`).
- A handful of `*Outlined` icons among the filled set.
- A hard-coded colour where a palette key would do. The warning green in `StatusDot` and the chart series green in `RunThroughputChart` are the deliberate exceptions, and each says why.
