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
