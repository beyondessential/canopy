# API Errors

## Not implemented

Issued when an endpoint exists but the behaviour behind it does not yet.
**501 Not Implemented**.

## Environment

Issued with an environment variable is not present or in the wrong format.

This should never be exposed over the API.

## Header

Issued when an HTTP Header is missing or malformed. **400 Bad Request** — the
header came from the client, so nothing on the server is at fault and
retrying the same request cannot succeed.

## Version Parse

Issued when a version or version range in URLs or API bodies is not parseable.
**400 Bad Request**, for the same reason as [Header](#header).

## Database

Database and query errors.

## Render

HTML template errors.

## IO

I/O errors, typically issued when handling too-large bodies.

## Resource not found

Issued when a database resource (such as a version, server, or other entity) cannot be found.

## No matching versions

Issued when a version range is valid, but does not match any of the available versions.

## Artifact not found

Issued when the version has no artifact with the given id that the caller is offered. An artifact scoped to a group the caller is not in is reported the same way as one that does not exist, so which groups hold an artifact cannot be discovered through this endpoint.

## Artifact digest mismatch

Issued when the bytes Canopy holds for an artifact do not match the digest recorded for it. The artifact is not served: a corrupted artifact fails the read rather than reaching a server as the artifact it is not.

## Unusable range

Issued when a version range is syntactically valid, but not usable to obtain concrete versions.

## Timesync

Issued for the /timesync endpoint.

## Auth: missing header

Issued when a header carrying the caller's identity is required but not
present. **401 Unauthorized**.

## Auth: missing certificate

Issued when a client certificate is required but not provided.

## Auth: invalid certificate

Issued when the provided client certificate is malformed, expired, revoked, or otherwise invalid.

## Auth: certificate not found

Issued when the provided certificate is well-formed but does not match any known device identity.

## Auth: insufficient permissions

Issued when the authenticated device is valid but lacks the necessary role.

## Auth: failed

Issued when authentication fails for unspecified reasons.

## Device has no server

Issued when a device calls an endpoint that acts on "its" server but is not
registered against any server. Devices must be linked to a server
(`servers.device_id`) first.

## Auth: tailnet directory unavailable

Issued when the private server can't reach the Tailscale control plane to
resolve a caller's tailnet IP to a node identity. The path to recover is
operator-side: check the OAuth credentials and the `TAILSCALE_TAILNET`
config, and confirm the device-list refresh loop is making progress.

## Auth: tailnet node not permitted

Issued when a tailnet caller's resolved node identity is missing the tag
required by `TAILSCALE_REQUIRED_TAG`. The node is on the tailnet, but not
one that's allowed to authenticate as a device.

## Tagged device not allowed

Issued when a tagged-device caller (no `Tailscale-User-Login` header,
source IP in the Tailscale CGNAT or ULA ranges) hits any private-server
route outside `/public/...`. Those routes are for human admins and
internal callers only.

## Ingest denied

Issued when a status push comes from a reporting source whose ingest mode
is set to `deny`. The source's reports are refused outright; set the
source to `allow` or `ignore` in the healthcheck settings to change this.

## Device tailscale node already claimed

Issued by the admin attach-tailscale flow when the requested node id
is already attached to a different device row. Resolve with the merge
flow if those two rows represent the same physical machine.

## Device merge conflict

Issued by the admin device-merge flow when source and target both
hold tailscale identity, or both are attached to a server. The
operator must clear one side first (detach tailscale or null out
`servers.device_id`) before retrying the merge.

## Bad request

Issued when a client sends syntactically or semantically invalid
input. Body content explains the specific issue (e.g. malformed
ticket, unsupported ticket version, unparseable URL).

## Conflict

Issued when the requested change conflicts with existing state in
a way the operator can resolve (e.g. importing a ticket whose
canonical URL is already claimed by a different server id). Body
content explains the conflict.

## Certificate key compromised

Issued when a certificate is requested for a key that was revoked
as compromised. Canopy will never certify that key again, for any
name or server, so retrying with it cannot succeed.

Distinct from a plain conflict so a client can act on it
unattended: the remedy is always the same — generate a fresh key
pair, submit a new signing request for it, and discard the old
key. Body content names the key and the certificate whose
revocation barred it.

## Name management paused

Issued when a paused server asks Canopy to act on its behalf —
publish an address record, or obtain a certificate. Canopy makes no
new changes for a paused server, though nothing already in place is
withdrawn.

Distinct from a withheld grant so a client can tell *not now* from
*not you*: a pause is expected to lift, so the client should wait
and try later rather than treat the refusal as permanent. Body
content says when the pause was set and why.

Revoking a certificate pauses its server automatically, so this is
the refusal an agent meets while an operator is investigating.

## Name not entitled

Issued when a server asks Canopy to act on a name that is not
within any domain its own group controls.

Reported identically whether the name is unclaimed or controlled by
a different group, so the endpoint cannot be used to discover other
groups' names. Distinct from a withheld grant so a client can
tell "this name is not yours" from "you may not manage names at
all".

## Auth: tailnet identity missing

Issued on the private-server's `/public/...` mount when the
extractor can't resolve the caller to a tailnet identity. Causes,
in order of likelihood:

- `CLIENT_IP_SOURCE` doesn't match what the upstream proxy emits,
  so `ClientIp` resolves to the wrong address.
- The caller's IP isn't in Tailscale's CGNAT v4 (`100.64.0.0/10`)
  or ULA v6 (`fd7a:115c:a1e0::/48`) ranges — i.e. they aren't
  reaching us through the Tailscale ingress.
- The IP *is* in tailnet space, but the directory cache doesn't
  know about it: the node may have just joined the tailnet and a
  refresh hasn't happened yet, or the directory background task
  is failing.

## Auth: token not valid

Returned (HTTP 401, with a `WWW-Authenticate: Bearer` challenge) by the
public-server MCP mount (`/mcp`) when the request carries no usable access
token: the `Authorization: Bearer` header is missing or malformed, or the
token is unknown, revoked, or expired. The response is deliberately uniform
across those cases so it can't be used as a token-lifecycle oracle; the
distinction is logged server-side.

If you're wiring up an agent and hit this, mint a fresh token from the admin
UI (Settings → MCP access) and check it was pasted whole — tokens start with
`canopy_mcp_`. Tokens expire a year after minting.

## Enrollment failed

Returned by the public-server enrollment endpoints
(`/servers/register/begin`, `/servers/register/complete`) for *every*
pre-completion failure: unknown or archived server, invalid/expired/consumed
enrollment token, invalid/expired/already-used challenge nonce, a key already
bound to a different live server, or a bad proof-of-possession signature.

The response is deliberately uniform (HTTP 403, no distinguishing detail) so
the endpoint can't be used as an existence/lifecycle oracle for a probed
`server_id`. The specific reason is logged server-side. If you're enrolling a
server and hit this, re-mint the token from the admin UI (it may have expired
or already been used) and confirm the box is presenting the same certificate
across `begin` and `complete`.

## Rate limited

Returned (HTTP 429) when a caller exceeds the enrollment endpoints'
in-process rate limit — currently per source IP and per target server over a
one-minute window. Enrollment is a rare, human-paced operation, so the budgets
are generous for legitimate use; hitting this means an unusual volume of
`/servers/register/*` traffic (a token-guesser, a griefer burning challenges,
or a misbehaving client retrying in a tight loop). Trips are logged under the
`enrollment` target for alerting. Back off and retry after the window.

## Upstream

Returned (HTTP 502) when a dependency Canopy proxies to on the caller's behalf
failed or is not configured. This covers the backup-credentials path: the
cross-account STS `AssumeRole` used to mint short-lived S3 credentials
(`POST /backup-credentials`) failed or the STS client is not configured on this
Canopy instance, or reading a group's repo-password k8s Secret for `GET
/backup-target` failed or the kube client is not configured. The response body
is a generic, caller-safe summary — the underlying detail (which can name IAM
roles, buckets, or secret names) is logged server-side only. Retry after a short
delay; if it persists, the Canopy instance's AWS/IRSA or kube wiring (or the group's
provisioning) likely needs attention.

## Backup Rotation In Progress

Returned (HTTP 503) by `POST /backup-credentials` and `GET /backup-target` while
a passphrase rotation is in flight for the group. The repository password is
about to change, so handing it out would give the device a passphrase that
stops working mid-backup — `kopia change-password` rewrites the repository's
format blob, and a run already under way fails when it next touches it.

This is not a failure: rotation is one short operation, so retry after a few
seconds. Backups skipped during the window are picked up on the next cycle.

A rotation that crashes without clearing its marker stops blocking issuance
once the marker ages past the rotation window, so this cannot wedge a group's
backups indefinitely.

## Other

An unclassified error.
