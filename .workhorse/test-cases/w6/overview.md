# Triage keeps what it learns in incident memory

## Automated

- [x] A token with the notes grant amends and retires an entry an operator recorded (verifies spec: MEM)

## Manual, in Slack after deploying

The guidance shapes an agent's behaviour, so these are checked by watching triage run.

- [ ] An alert whose cause the agent works out records or amends a memory entry linked to the check, group or incident, in its own words and naming where it came from (verifies spec: TRI)
- [ ] A close alert's report says which entries it recorded or amended, or that there was nothing to keep (verifies spec: TRI)
- [ ] An operator saying in an escalation thread that a pattern is harmless, then the incident closing, leaves a memory entry naming the operator (verifies spec: TRI)
- [ ] Someone not named as an operator saying a pattern is harmless leaves an incident note naming them, and no memory entry saying it is harmless (verifies spec: TRI)
- [ ] A triage reads fleet-wide entries as well as the incident's own (verifies spec: TRI)
- [ ] A reply in a thread that mentions Claude fetches the guidance before answering
