---
id: ESC
---

# Recovery escrow

Canopy holds the only copy of some of what it keeps: a group's repo passphrase, an environment's secret variables.
It continuously escrows that state so it can be recovered without Canopy, and does so in a way a compromise of Canopy cannot read back.

## Scope

This spec covers the escrow itself: what it is encrypted to, where it is written, what recovering from it takes, and how the ability to recover is kept proven.
What each area puts in it is that area's to say.

## The mechanism

The escrow is encrypted to a set of recipient public keys whose private halves Canopy never holds.
So Canopy can write the escrow and never read it back: a full compromise of Canopy discloses nothing that is only in there.

Recipients are mandatory.
Canopy refuses to run without them, so there is never a silent recovery gap.
A recipient set is identified by its public keys, which is what a reader compares to tell a deliberate rotation from a substitution.

The escrow is written to versioned, object-locked storage, so a past version stays undeletable until its retention expires.
Canopy writes the current state on a cadence to one object rather than accumulating objects; the storage's own versioning is the history.

## What a recovery needs

One recipient's private key decrypts the escrow, so recovery needs no quorum and no other recipient's cooperation.

The escrow carries its own version, so a reader knows the shape it is reading rather than inferring it.
It records when it was taken, since a recovery has to know how stale what it holds is.

A value Canopy could not read when the escrow was taken is written as absent and logged, rather than failing the whole write: a snapshot missing one group's passphrase is worth more than none at all.

## The verification ceremony

An escrow nobody has ever decrypted is not a recovery path, so the ability to recover is proven rather than assumed.
Canopy issues a challenge, an operator answers it with a recipient's private key, and Canopy records the proof.
The private half never reaches Canopy: what it holds is that somebody demonstrated they still have one.

A proof names the recipients it was made against, so a changed recipient set does not inherit the last one.
Canopy surfaces how long it has been since the last successful proof, since recipients quietly rotating away is a recovery gap nobody would otherwise see.
