---
id: ART
---

# Version artifacts

An artifact is a file published for a version: an installer, a package, a set of migrations, a manifest.
Canopy holds what each one is and whom it is for, and a server or the infrastructure acting on a version's behalf learns from Canopy which files a version has and fetches each from where it rests.
An artifact may be for one group alone, and Canopy offers it only to a caller whose group it is.

## What an artifact belongs to

An artifact belongs to one exact version, or to a range of versions given as a semver pattern, and never to both.
A range artifact covers every version its pattern matches, so a file that does not change between releases is published once instead of per release.

An artifact carries a type and a platform, which together say what it is and what it is for.
A version's artifacts are the exact-version ones plus every range artifact whose pattern matches it.

An artifact may belong to a group, and one that does is for that group alone.
An artifact belonging to no group is for every group.
A group scope exists because some artifacts are derived from a group's own data and are wrong for anyone else.

## Where an artifact rests

An artifact belonging to no group rests at a location Canopy records and does not hold, which whoever is offered the artifact reads for itself.

A group-scoped artifact is carried to Canopy by the registration that publishes it, and Canopy holds it.
A publisher sends the bytes on the connection it registers over and is issued no credential to any store, so being authorised to register for a group is the whole of what publishing into it takes.
Canopy holds such an artifact in storage of its own, apart from any group's backup repo, so an artifact carries the retention, access, and cost basis of an artifact rather than those a backup repo is kept under (see [BAK](../public-server/backup.md)).
Where Canopy puts them is its own, and no caller addresses them there.
Canopy holds an artifact's bytes for as long as that artifact is registered, and keeps none of what it has stopped serving.

Canopy serves the bytes only to a caller the artifact is offered to.
The boundary is therefore enforced on the read rather than resting on a location being hard to guess.

An artifact Canopy holds and an artifact Canopy records a location for are one thing to whoever is offered it.
It is offered one artifact per type and platform, and where the bytes rest is not part of what it is offered.

## What a version offers

Canopy offers a caller one artifact per type and platform, chosen from the artifacts that caller may see: those belonging to no group, and those scoped to the caller's group where that group is known.
Where several match, the most specific is offered.
An artifact scoped to the caller's group is more specific than one belonging to no group.
Among artifacts of the same scope, an exact-version artifact is more specific than any range artifact, and between two ranges the narrower is more specific.
A group-scoped artifact and an unscoped one of the same type and platform are therefore both recorded, each group is offered the one for it, and no caller is offered both.
A pattern Canopy cannot parse matches nothing rather than everything, so a malformed range withholds a file instead of offering it to the whole fleet.

The full set, including the artifacts specificity passed over, is available to operators.
What resolution hides is a fact about how a version was published and an operator has to be able to see it.

## Who is offered a group-scoped artifact

A caller whose credential is bound to a server has that server's group, where the server has one.
It never names a group and is answered for its own alone, so a server cannot ask what another group is offered.
A caller carrying no group of its own names the group it asks about and is answered for a group it is authorised for: an operator for any group, and a component that produces or applies a group's artifacts for that group, as defined with those artifacts.
A read carrying no identity, or naming no group, is answered with the unscoped artifacts alone, so giving an artifact a group narrows who is offered it rather than widening what an open path serves.
A group-scoped artifact's existence is disclosed only to a caller it is offered to: a caller that names or guesses one it is not offered is answered as though it did not exist, so which groups hold one is not enumerable through the artifact surface.
Canopy passes a group-scoped artifact's bytes only to a caller it is offered to.

## Registration

A registration names the version or range, the type, the platform, and the group where the artifact has one, and carries either the location of an unscoped artifact or the bytes of a group-scoped one.
The group is named on the registration rather than inferred from the caller.

A releaser device registers unscoped artifacts, and carries no authorisation for any group.
An operator registers either.
A component that produces a group-scoped artifact registers it for that group under an authorisation defined with that artifact, and is authorised for no other.
A registration naming a group the caller is not authorised for is refused, which is the only gate publishing into a group has to pass.
A registration replaces whatever is already registered for the same version or range, type, platform, and group, so a rebuilt artifact is published exactly as a first one is and a caller is never offered two of a kind.

Canopy records which device registered an artifact and, where the registration names one, the run that produced it, so an artifact that arrived by automation is distinguishable from one entered by hand and traceable to what made it.

## Digests

An artifact carries a digest where whoever registers it records one, and a group-scoped artifact carries one always.
Canopy verifies a group-scoped artifact's bytes against its digest as they arrive and refuses the registration on a mismatch, so a corrupted upload is refused while whoever sent it is still there to send it again.
It verifies them again as it serves them and refuses them on a mismatch, so an artifact corrupted after it was taken in fails the read rather than reaching a server as the artifact it is not.
An unscoped artifact is read from its location by the caller rather than by Canopy, so its digest is what that caller checks what it fetched against, and an artifact registered without one is fetched unchecked.
