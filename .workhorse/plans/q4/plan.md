# Machine row icon shows the machine only

## Decisions

The group tree's machine row (group, machine and application pages, all via `GroupTree`) draws the machine alone, not an enclosure of its applications' dots, since the rows beneath already list them.
The Status page cards keep the enclosure holding application dots.

Standalone machine mark: a solid round dot, the size of a one-dot enclosure, filled in the machine's state colour with an edge a shade darker.
Fine is green (`success.main`), degraded orange (`warning.main`), unreachable red (`error.main`).
A window over the box stripes it and pulses as the pill does; a wider window fades it.

Never reported, in both forms: background fill with a 2px dotted edge in the main text colour.
The fill is the card background, so it reads as white in light mode and as the card in dark mode.
The enclosure's padding drops by 1px to absorb the thicker edge, so it stays the size of its neighbours.

The legend block (version, status, machine, maintenance) is removed from the machine and application pages.
Legends stay on the Status and cluster pages.

The CHK "One subject per mark" section needs rewording to cover both forms and the never-reported mark.
