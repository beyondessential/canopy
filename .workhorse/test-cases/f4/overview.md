# Resolving DNS names on multi-application machines

Scenarios for how a machine's address and certificate requests resolve to one of its applications, what happens when they cannot, and the operator side: declaring, denying, and the notices.

## Resolution

- [x] Two applications with the same grants under the same domains: a certificate request is refused as `dns-name-undeclared` and recorded as asked for a certificate (verifies spec: CRT#resolving-the-application)
- [x] A repeat ask updates the one record, including what it was asked for (verifies spec: CRT#undeclared-requests)
- [x] A machine's undeclared records are bounded: past the bound a new DNS name is not recorded and one already held still refreshes (verifies spec: CRT#undeclared-requests)
- [x] Naming the application type resolves the request, declares the DNS name for that application, and ends the undeclared record (verifies spec: CRT#resolving-the-application)
- [x] Once declared, a request without a type resolves from the declaration
- [x] Only one application holding the needed grant: the request resolves to it with no type and no operator (verifies spec: CRT#resolving-the-application)
- [x] A type contradicting the declaring application on the same machine is refused, naming the declaring type (verifies spec: CRT#resolving-the-application)
- [x] A single-application machine asking for a DNS name another machine's application holds is refused as undeclared and recorded (verifies spec: CRT#resolving-the-application)
- [x] A DNS name held elsewhere and one held by nobody refuse alike (verifies spec: CRT#resolving-the-application)
- [x] A single-application machine still gets the grant and domain refusals, not undeclared (verifies spec: CRT#identity-and-authorisation)
- [x] Address registration resolves by a named type the same way a certificate request does

## Denial

- [x] A denied DNS name is refused as `dns-name-denied` without the operator's note, however often asked, places no order, and records nothing (verifies spec: CRT#denied-dns-names)
- [x] A single-application machine naming a type it does not host is refused as `dns-name-type-mismatch`, naming the type it does, and declares nothing (verifies spec: CRT#resolving-the-application)
- [x] Outside the group's domains, a DNS name declared on another machine and one declared nowhere are both refused as `name-not-entitled`, and neither is recorded (verifies spec: CRT#resolving-the-application)
- [x] The monitor sweep drops undeclared records not asked about for a day (verifies spec: CRT#undeclared-requests)
- [x] Denying ends the undeclared record; lifting removes the denial; lifting again is a 404 (verifies spec: CRT#denied-dns-names)
- [x] Denying a DNS name declared on one of the machine's applications is refused, naming it (verifies spec: CRT#denied-dns-names)
- [x] Declaring a DNS name ends both its undeclared record and its denial on the declaring application's machine (verifies spec: CRT#denied-dns-names)
- [x] A denial outlasts a day without the machine asking (verifies spec: CRT#denied-dns-names)

## Notices

- [x] Notices count each machine's undeclared requests fleet-wide and within one group; one not repeated for a day is excluded (verifies spec: CRT#presentation)
- [x] The group page notice names its machines and links to them; the Status notice names the group (verifies spec: CRT#presentation)
- [x] A denial raises no notice, and with nothing waiting there is no notice (verifies spec: CRT#presentation)

## Operator interface

- [x] Declaring on the application page shows the DNS name as declared with no addresses registered (verifies spec: CRT#presentation)
- [x] Declaring outside the group's domains is allowed and flagged (verifies spec: CRT#declared-dns-names)
- [x] Releasing a DNS name removes it from the application page
- [x] An undeclared request is declared on a chosen application from the machine page and then shows under Declared (verifies spec: CRT#presentation)
- [x] Denying from the machine page records the note; lifting clears the section (verifies spec: CRT#presentation)
- [x] A machine with nothing to show has no DNS names section
- [x] An operator without admin sees the machine section without Declare, Deny or Lift

## Client

- [x] A refusal's message includes the reason Canopy gave in its problem document (verifies spec: APIC#the-consumer-supplies-the-transport)
- [x] A refusal without a problem document reports the status alone

## Smoke

- [ ] On a real multi-application box (Tamanu and mSupply), bestool's requests appear under Not yet declared, declaring each lets the next collection pass issue, and `bestool canopy certs` shows the reason for any refusal
