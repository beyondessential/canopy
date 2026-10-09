# DNS names and TLS certificates as separate features

Scenarios for splitting DNS address requests from TLS certificate requests, and for showing a certificate's expiry once.
"Kind" means addresses or certificate.
Where a scenario names a machine page, the same scenario applies to the application and group pages unless a section says otherwise.

## Expiry display

- [ ] A certificate with 89 days left of a 90-day life shows "expires in 89 days" once, with no second "left" reading beside it (verifies spec: CRT#presentation)
- [ ] Hovering the expiry shows the exact instant (verifies spec: CRT#presentation)
- [ ] A certificate with 79 days left on the application page and on the machine page shows the same wording and the same number of days, not 79 on one and 80 on the other (verifies spec: CRT#presentation)
- [ ] A 90-day certificate with 60 days left reads calm, with 25 days left reads as due for renewal, and with 10 days left reads as urgent, matching its state chip each time (verifies spec: CRT#presentation)
- [ ] A 6-day certificate with 2 days left reads as urgent, while a 90-day certificate with 2 days left also reads as urgent, and a 90-day certificate with 7 days left does not read as urgent (verifies spec: CRT#presentation)
- [ ] An expired certificate shows "expired N days ago" in the urgent colour
- [ ] A certificate with under an hour left shows minutes and a certificate with under a day left shows hours, rounded the same way on every page
- [ ] The expiry colour and the state chip change together when the clock crosses the renewal point

## Declarations

- [ ] An operator declares a name for addresses on an application; it appears in that application's DNS names section and not in its TLS certificates section (verifies spec: NAM#declared-dns-names)
- [ ] An operator declares a name for certificates on an application; it appears in the TLS certificates section and not in the DNS names section (verifies spec: NAM#declared-dns-names)
- [ ] Declaring the same name for both kinds on the same application succeeds and shows in both sections
- [ ] Declaring a name for certificates on application B while application A holds it for addresses is refused, and the refusal names A (verifies spec: NAM#declared-dns-names)
- [ ] Declaring a name for addresses on application B while application A holds it for certificates is refused, and the refusal names A
- [ ] Releasing a name for certificates leaves its address declaration in place and stops its certificates being renewed (verifies spec: NAM#declared-dns-names)
- [ ] After releasing a name for its last kind, another application can declare it for either kind
- [ ] Releasing a name for addresses leaves published records published and its certificates held
- [ ] A name outside every domain the group controls can be declared for either kind and shows flagged as outside the group's domains in the matching section

## Resolution

- [ ] A certificate request about a name an application on the machine holds only for addresses resolves to that application and adds the certificate declaration, given the TLS grant (verifies spec: NAM#resolving-the-application)
- [ ] The same request from an application without the TLS grant is refused for the missing grant and adds no declaration
- [ ] An address registration about a name held only for certificates resolves the same way, given the DNS grant
- [ ] A request about a name another application on a different machine holds is refused as undeclared, for either kind, with the same refusal as a name nobody holds
- [ ] A request naming a type the machine contradicts is refused as before for either kind, and the refusal names the holding application's type

## Undeclared requests

- [ ] A machine asking for a certificate for an undeclared name gets one record of kind certificate, shown only in the TLS certificates section (verifies spec: NAM#undeclared-requests)
- [ ] The same machine then asking for addresses for that name gets a second record of kind addresses, shown only in the DNS names section, and the first record is unchanged
- [ ] Asking again for the same kind updates the existing record's last-asked time rather than adding a row
- [ ] Declaring the name for certificates clears the certificate record and leaves the address record
- [ ] A record clears when the machine's next request of that kind is accepted, and after a day without that kind of request, and not on the other kind's request
- [ ] The per-machine bound refuses a new record past the limit while the existing ones stand, with kinds counted separately as records
- [ ] The group notice and the Status page notice say how many undeclared requests there are, of which kind, and lead to the machine or group

## Denials

- [ ] Denying a name for certificates refuses certificate requests about it as denied and leaves address requests about it unrefused (verifies spec: NAM#denied-dns-names)
- [ ] Denying a name for addresses refuses address requests as denied and leaves certificate requests unrefused
- [ ] A request refused as denied records no undeclared record
- [ ] Declaring the name for certificates lifts the certificate denial and leaves an address denial of the same name standing
- [ ] Lifting a denial from the machine's DNS names section affects only the address denial
- [ ] A denial shows who denied it, when, and the note, in the section for its kind only
- [ ] Denying a name for both kinds creates two denials, each lifted separately

## Machine page

- [ ] A machine with only certificate activity shows a TLS certificates section and no DNS names section (verifies spec: NAM#on-a-machine)
- [ ] A machine with only address activity shows a DNS names section and no TLS certificates section
- [ ] The DNS names section never shows a certificate state, an expiry, or a certificate-kind request or denial
- [ ] The TLS certificates section never shows addresses, published state, or an address-kind request or denial
- [ ] A machine with no declared, undeclared, or denied names for a kind shows no section for it
- [ ] On a machine with several applications each declared name shows the application declaring it, in the section for its kind
- [ ] On a machine with one application, an undeclared record for a name another application holds can be listed but declaring it is refused with the holder named
- [ ] Declaring from the machine page gives the same result and the same refusal as declaring from the application page
- [ ] Both sections render with the same layout and controls, differing only in the data and kind shown

## Group page

- [ ] Under each domain, the DNS names section lists names declared for addresses beneath it with whether their records are published, and shows no certificate information (verifies spec: NAM#on-a-group)
- [ ] Under each domain, the TLS certificates section lists names declared for certificates beneath it with whether each holds a current certificate, and shows no address information
- [ ] A name declared for both kinds appears under both sections

## Application page

- [ ] The application page shows separate DNS names and TLS certificates sections in place of one combined panel
- [ ] The DNS names section shows addresses, published state, and the declared-without-addresses and withdrawing distinctions
- [ ] The TLS certificates section shows each certificate with profile, state, and time left, pending and failed requests with the reason, and the profile and pause controls
- [ ] A paused application shows as paused in both sections, with who paused it, when, and why

## Entitlements

- [ ] The entitlements answer lists the names an application declares for addresses and for certificates apart (verifies spec: NAM#what-an-application-may-act-on)
- [ ] An application with no declarations of either kind still gets an empty entry rather than an error

## Migration

- [ ] After migrating a database where every declaration is a row with no addresses, each becomes a certificate declaration for the same application and the address side is empty
- [ ] A migrated row that carries registered addresses is both an address declaration and a certificate declaration
- [ ] Every pre-existing denial becomes a certificate denial and no address denial exists afterwards
- [ ] Pre-existing undeclared records keep their kind, and none is lost or duplicated
- [ ] A certificate renewal due before the migration still renews after it, for the same application

## Compatibility

- [ ] `just gen-openapi` and `just gen-api` produce no change to the public API document or the published client crate
- [ ] An agent that registers addresses and requests certificates through the public endpoints behaves as before for a machine with one application
- [ ] `just check-generated` passes with the private API document and types regenerated
