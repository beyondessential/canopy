# J6: notes history, note writes over MCP, incident memory

## Notes history (verifies spec: FLT)

- [x] Notes a group is created with start its log, attributed to the creator
- [x] A change is a new revision; a save of the same text records none; clearing is kept
- [x] A change made with a stale revision is refused with the current text and revision
- [x] A machine's and an application's edits go through the log
- [x] A revision belongs to exactly one record
- [x] A change by an operator, through an Ansible import or through an access token records how it was made
- [x] A change is audited with the record and revision, never the text
- [x] The group, machine and application pages show the history newest first
- [ ] Existing non-empty notes get revision 1 with no author when the migration runs

## Issue and incident notes (verifies spec: CHK, INC)

- [x] An agent's note is marked as an agent's
- [x] An edit keeps the replaced text and who edited it; a stale edit is refused
- [x] A deleted note leaves lists and counts, is kept, and cannot be edited
- [x] Only administrators are shown deleted notes, and only when they ask
- [x] Adding and deleting a note are audited without the note text
- [x] An agent cannot edit or delete an operator's note, naming the reason
- [x] An operator can delete a note from the incident timeline

## Fleet query interface (verifies spec: MCP)

- [x] Every write is refused with no effect to a token without the notes grant
- [x] A token minted with the notes grant is listed with it
- [x] Replace notes states a revision and is refused with the current notes when stale
- [x] Get issue and get incident return notes without the deleted ones
- [x] Memory entries are recorded, amended, retired and restored; limits are named in the refusal
- [x] The index counts incidents, shows checks and groups with no linked entries, and narrows to a group

## Incident memory (verifies spec: MEM)

- [x] An entry's history keeps every version, retirement and restoration, newest first
- [x] A retired entry is hidden unless asked for, and cannot be amended until restored
- [x] Search matches word prefixes in titles and bodies
- [x] Entries relevant to an incident are those linked to it, its group, or a check among its issues
- [x] Any tailnet user reads; only administrators write
- [x] The memory page holds its search and filters in the address
- [x] An incident's page links to the memory page filtered to relevant entries
