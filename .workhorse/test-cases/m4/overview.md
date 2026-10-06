# Rank input on the machine edit form

## Rank field

- [x] An unranked box's Rank label floats above "Not ranked yet" rather than over it
- [x] A box with nothing on it offers the Rank select, and saving a rank ranks the box (verifies spec: FLT)
- [x] Ranking a pending box ranks every application on it (verifies spec: FLT)

## The box's rank

- [x] An empty box can be ranked, and an application arriving on it takes that rank (verifies spec: GRP)
- [x] An unranked application written onto a ranked box takes the box's rank (verifies spec: GRP)
- [x] Ranking an application by any writer ranks its box (verifies spec: GRP)
- [x] Ranking a box by any writer ranks its live applications, and archived ones keep theirs (verifies spec: GRP)
- [x] A box created in the same statement as its ranked application is ranked
- [x] Restoring onto an unranked empty box ranks the box with the application's rank (verifies spec: GRP)
- [x] The update endpoint ranks a box with no application and returns the new rank (verifies spec: FLT)
- [x] A ranked box with nothing on it sits under its rank in the group tree, still awaiting check-in (verifies spec: FLT)
- [x] The migration backfills a box's rank from its live applications and leaves pending, empty and fully archived boxes unranked

## Archived boxes and reassignment

- [x] An archived machine with nothing live on it is refused a new rank, keeps the rank it was archived at, and serves no environment (verifies spec: FLT)
- [x] An archived machine is never refused the rank it already carries, so an edit naming it saves (verifies spec: FLT)
- [x] A live application restored onto an archived box can be ranked, and ranks the box (verifies spec: FLT)
- [x] The update endpoint refuses a rank for an archived machine and leaves the rest of the edit unapplied (verifies spec: FLT)
- [x] An application restored onto an archived box comes back at the rank the box carries (verifies spec: FLT)
- [x] An application moved onto a ranked box takes the box's rank rather than re-ranking it (verifies spec: GRP)
- [x] An application un-archived onto a ranked box takes the box's rank (verifies spec: GRP)
- [x] The schema reads every rank spelling the way `ServerRank` parses it
- [x] An application inserted onto a ranked box at another rank takes the box's rank (verifies spec: GRP)
- [x] A box's rank column refuses a spelling that is not canonical, including one no one recognises
- [x] With the triggers bypassed, the schema still refuses two ranks on one box
- [x] An archived box with nothing on it offers no rank to change on the edit form (verifies spec: FLT)
- [ ] A push adopting an application while the box is re-ranked takes the rank the locked row holds
