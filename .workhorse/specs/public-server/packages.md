---
id: PKG
---

# Package reporting

A machine reports the Linux packages installed on it, so Canopy can match them against published vulnerabilities (see [VUL](../security/vulnerabilities.md)).
Packages are a machine's, never an application's: a box shared by two workloads reports one list.

## Report

A report is pushed to the device API by the machine it describes, presenting that machine's enrolled identity.
A device with no machine is refused.

The report carries:

- **distribution** — the distribution's identifier, as `/etc/os-release` gives it in `ID` (e.g. `ubuntu`);
- **release** — the distribution's release, as `/etc/os-release` gives it in `VERSION_ID` (e.g. `24.04`);
- **packages** — every installed package, each by its **source** package name and source version, since vulnerability databases record advisories against source packages rather than the binary packages built from them.

A report is the machine's whole package list, and replaces the one before it.
A report identical to the last one changes nothing.
A report that changes the list is audited as a routine action, with the machine as the record acted on and the distribution, release and package count as its detail (see [AUD](../platform/audit-log.md)).
A machine reports when its list changes, not on every status push, and a report is kept until the next one, however long that is.

## Ecosystems

Canopy places a report in the OSV ecosystem its distribution and release name, Ubuntu releases being named with their long-term support marking where they carry one.
A report from a distribution or release Canopy cannot place is kept and matches nothing.

## Presentation

A machine presents when it last reported its packages and how many it reported, flagging a report Canopy could not place in an ecosystem, since nothing on that machine is being checked.
