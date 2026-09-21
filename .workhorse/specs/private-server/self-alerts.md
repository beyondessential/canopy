---
id: SELF
---

# Self-alerts

A self-alert is Canopy reporting a problem with its own operation, as distinct from an issue observed on a fleet member.
Self-alerts are Canopy-wide checks (see [CHK](../monitoring/checks.md)): they carry the same state, policy, silences, and resolution as any other check, and they aggregate into incidents on the Canopy target (see [INC](../monitoring/incidents.md)).

## Conditions

Each self-alert condition is a check with a stable name under the `canopy` source, and at most one state exists per condition: repeated detections update the one state rather than accumulating.
A condition is active while it holds, and recovers when it clears; a condition without automatic recovery stays active until an operator resolves it.

The current conditions are:

- Canopy's own identity for reaching backup storage is broken (escalating).
- An operator-notification delivery has permanently failed (stays until operator-resolved).
- An MCP access token is within fifteen days of its expiry (see [MCP](mcp.md), "Access tokens").
- One or more catalogued checks have gone unreported across the whole fleet for thirty days (see [CHK](../monitoring/checks.md), "Liveness and decommissioning"); it clears when no such check remains, each having been decommissioned or reported again.
- A history has less than two weeks of future range left to write into, failing below one week (see [HST](../platform/history-storage.md), "Running short"); it clears once every history is provisioned ahead again.
- One or more group domains fall outside the DNS zones Canopy is configured with, failing when Canopy can read no zones at all and warning when only some claims are uncovered (see [DOM](../servers/domains.md), "When the zone configuration changes"); it clears when every live group's domains sit within a configured zone again.
- One or more relays are not running the version of the check suite Canopy has named for them (see [K8S](../monitoring/kubernetes.md), "Keeping a relay current"). Each registered cluster is an instance, its detail carrying the version its relay runs and the version named for it, so a relay whose update did not take is visible before it and the rest of the fleet grade the same condition differently. A relay left on an older version means either an update that did not complete or a relay that will not accept the version named, and both want an operator. It clears when every relay runs the version named for it.

## Notification

Self-alerts notify through the incident machinery: an effective failure opens an incident on the Canopy target, which notifies the operator channel per the incident notification rules (grace period, escalation, recovery notice).
When the notification channel is not configured, self-alerts are still recorded and presented, and the operator is warned that notification is off.

## Presentation

Active self-alerts are presented on their own surface in the operator UI, apart from fleet issues and incidents: a persistent notice visible from any page while any alert is active, leading to a view of the active alerts and recent recoveries.
Self-alerts do not appear in the fleet issue listings, and are not presented as belonging to any machine or application.
Only Canopy's own conditions appear on that surface: a check filed about an application, a machine, or a group is a fleet issue, not a self-alert.
Each alert presents its effective result, when it became active, and a description of the condition and what to do about it.

An alert that names things an operator has to act on presents each of them as a way to reach it, not only as text in the message.
The alert for checks gone quiet names each check by its whole identity — its source and its namespace-qualified name, since a name alone can belong to several entries — and each one leads to that check's own policy, where decommissioning it is done.
This matters because the condition is not something resolving the alert clears: it holds until each check named has been decommissioned or has reported again, so an alert that only describes what is wrong is one an operator cannot act on from where they are reading it.
