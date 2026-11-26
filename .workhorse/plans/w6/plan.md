# Triage keeps what it learns in incident memory

Triage agents never write to incident memory. The guidance's only write path is the "Learning" section: record an entry when an operator confirms a cause in an escalation or write-up thread. Nothing reliably wakes the agent at that moment (a thread reply only reaches it if it mentions Claude), the session it wakes in has no reason to fetch the guidance, and the step sits outside the per-alert order of work.

## Decisions

- **Canopy memory is the agent's working memory.** What the agent learns in any session (triage, a thread reply, the morning routine) goes in incident memory rather than Claude Tag's own channel memory, so operators can see it, it is linked to what it concerns, and it outlasts the channel.
- **It records as it learns, and reviews at close.** A published incident always notifies on close, and that notification mentions the agent, so the close session is a guaranteed review: it reads the incident's notes, its escalation thread and its reply in the write-up thread (replies under that reply only, not the whole shared thread), records what is not yet in memory, and says in its close reply what it recorded or that there was nothing to keep.
- **Remarks after close** are handled by the same steps when they mention the agent; there is no separate morning sweep.
- **Wider scope.** Causes and harmless patterns, plus gotchas (a check that misreads during a site's backup window, a tool that misleads), how a kind of failure was told apart or explained, where a site's logs or fix live, and who answered for a site or check.
- **Narrowest link.** An entry links to the check, group or incident it concerns; only fleet-wide process gotchas are unlinked.
- **Fleet-wide entries are read every session**, alongside the incident's own checks, group and incident, so gotchas recorded unlinked are seen again.
- **Provenance in the body.** Each entry says where its content came from: the agent's own finding on an incident, or what a named operator said and when, so a later session weighs it rather than trusts it.
- **The operator gate covers only what quiets an alert.** An entry saying a pattern is harmless still needs an operator's word (a named operator in Slack, or an operator-authored note in Canopy), and still counts only where the check's stability record bears it out. Everything else is recorded on the agent's own evidence.
- **The agent may amend and retire any entry**, operator-written ones included, when its evidence contradicts it. Memory exists for the agent; the entry's history keeps every change attributed and reversible. This is deliberately unlike notes, where an agent cannot edit an operator's note.

## Build

- [x] Alert triage spec (TRI): fleet-wide reads, "What triage keeps" with the operator gate and the closed-incident review
- [x] Incident memory spec (MEM): wider entry purpose, link to triage, an agent may change any entry
- [x] `crates/canopy-mcp/src/alert_triage_guidance.md`: read fleet-wide entries; replace "Learning" with what to keep, linking, provenance, amending any entry, the operator gate; add the closed-incident review to the order of work, with its line in the close report
- [x] `crates/canopy-mcp/src/memory.rs`: widen `record_memory_entry`'s description to match MEM's list of what an entry is for
- [x] `plugins/canopy-mcp/README.md`: the triage-channel instructions keep the operator list for the harmless-pattern gate, and tell Claude to keep what it learns in Canopy memory rather than its own
