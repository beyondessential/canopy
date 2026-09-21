# In-cluster relay and transport — test cases

What verifies the relay, the transport it speaks to Canopy over, and the identity that gates it. Automated coverage lives in `crates/relay-protocol/tests/handshake.rs`, `crates/relay/tests/dispatch.rs`, and `crates/jobs/tests/it/relay.rs` and `relay_end_to_end.rs`.

The security cases are the ones that matter most here. With no CA and no chain in either direction, the device key and the pin are the whole gate, and a regression in either would not fail loudly — it would quietly accept the wrong peer.

## Identity and the gate

- [x] A device enrolled at the relay role connects, is authenticated by the key it presents, and is registered against that device (verifies spec: K8S).
- [x] The SPKI Canopy stores at provisioning is byte-identical to the one the relay's handshake presents, so a minted credential authenticates without any conversion step.
- [x] A device key Canopy has never seen completes the handshake and is then refused by the lookup, and the connection does not survive it.
- [x] A deactivated key stops authenticating, so revocation works through the existing path.
- [x] A device holding a good key at another role (a server) is refused, so the role is load-bearing rather than decoration.
- [x] A peer presenting no certificate at all is refused at the handshake, before reaching a lookup with nothing to look up.
- [x] A relay refuses a Canopy whose public key is not the one it pins.
- [ ] Signature verification is genuinely performed on both verifiers, so the handshake is proof of possession rather than a presented public certificate being enough. Currently rests on the verifiers delegating to rustls; a peer that holds an enrolled device's certificate but not its private key should be shown to fail.
- [ ] A relay whose key is revoked while connected loses its connection, rather than keeping the one it already holds until it happens to reconnect.

## Protocol

- [x] Every request and every response crosses the wire as itself, covered exhaustively — the set is Canopy's authority over a cluster, so a variant that fails to round-trip is a method that silently does not work.
- [x] A harvest filing carries the status-push body verbatim, since that is what makes parity structural rather than maintained.
- [x] Every substrate filing target round-trips: an instance, a namespace, and the cluster.
- [x] Successive frames on one stream stay separate, and a clean end of stream is distinguished from a stream that ended mid-frame.
- [x] A frame larger than the ceiling is refused before its body is allocated, so an oversized length prefix costs the reader nothing.
- [x] A relay offering a protocol version Canopy does not speak fails at the handshake, rather than connecting and then failing to parse.
- [x] A response of the wrong shape for the request is rejected as a protocol failure, so a caller cannot read one answer as another.

## What the relay decides for itself

- [x] A deployment with no scheduled expiry cannot be put to sleep, and the relay is what refuses it (verifies spec: K8S).
- [x] A deployment with an expiry sleeps and wakes.
- [x] A version below the relay's floor is refused before its Deployment is touched, which is the answer to a Canopy ordering a downgrade.
- [x] A version at or above the floor is accepted and reaches the cluster work.
- [x] The floor is compiled into the relay, so it is not something Canopy or a deployment supplies.
- [x] A version string that is not a version is refused rather than interpreted; a leniently-parsed one (a leading `v`, a trailing prerelease) is still held to the floor.
- [x] A namespace the relay does not serve is a refusal rather than a failure, so Canopy can tell declining from having tried.

## Both ends together

- [x] The shipped relay client connects to the shipped listener, answers what Canopy asks on connect, and Canopy holds what it answered rather than something it assumed.
- [x] A filing written by the relay's client loop is read by Canopy's listener over a real connection.
- [x] A filing Canopy cannot place costs the filing and not the connection, and the connection is still answering afterwards.
- [ ] A relay reconnects after the connection drops, and Canopy's registry ends up holding the new connection rather than the stale one. The replacement logic is written and commented; nothing exercises it.
- [ ] A relay that reconnects while Canopy still holds its previous connection displaces it, and the old connection's teardown does not remove the new entry.

## Ingestion

- [x] The existing status-push behaviour is unchanged across the hoist into `commons-servers` (the full public-server suite, which covers the push path, passes).
- [x] A push may not claim the `kubernetes` source, which is reserved for a relay's substrate checks (verifies spec: K8S).
- [ ] A harvest filing for a placeable instance records a status and files its checks, identically to the same body arriving as an HTTP push. Blocked on filing placement: no server record carries Kubernetes coordinates yet, so nothing is placeable. This is the case that proves the parity the design rests on, and it is owed.
- [ ] A substrate filing at each grain — server, namespace, cluster — lands at the matching scope, with the relay as provenance only at server scope. Blocked on the same placement.
- [ ] A substrate check registers its shipped policy on first sight and an operator's later edit is not overwritten.

## Operational

- [x] A relay refuses to start without a usable device key or a usable pin, rather than carrying on unable to authenticate or unsure which Canopy it is talking to.
- [ ] The relay hub refuses to start without its key, rather than generating one that no relay recognises. The binary requires it and fails on an unusable one; nothing exercises the binary's own startup.
- [ ] A relay pod in a cluster reaches Canopy over the overlay network with a kernel-mode sidecar, and QUIC passes. Manual: userspace mode exposes a TCP-only proxy QUIC cannot traverse, which is the reason the sidecar mode is specified.
- [ ] A relay in Canopy's own cluster reaches Canopy over cluster DNS with no sidecar, on the same code path.
