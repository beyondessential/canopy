# Authentication and authorisation in Canopy today

Read from the branch at the start of the P6 design loop. This is the factual input to all three sides of the card's design space, and the brief every direction mockup works from.

## How a caller proves who it is

Three mechanisms, plus a few static shared secrets.

### Device key (machines)

A client certificate checked by nginx or Envoy, a QUIC mTLS handshake for the relay, or a tailnet node id standing in for one. The extractor is shared (`crates/commons-servers/src/device_auth/mod.rs:133`), and every authenticated request writes a row to `device_connections`.

- **mTLS certificate**: a PEM certificate percent-encoded in a header the ingress sets, named by `CANOPY_DEVICE_AUTH_CERT_HEADER` (`mtls` or `xfcc`). The certificate's public key is looked up against `device_keys.key_data` where `is_active`. There is no proof of possession at this layer: trust rests on the proxy.
- **Tailnet node identity**, only on the private server's `/public` mount: the caller's IP, which must fall in a tailnet range, is mapped to a node id through the Tailscale API cache and looked up against `devices.tailscale_node_id`. A single optional tag gate comes from the `TAILSCALE_REQUIRED_TAG` environment variable.
- **Relay**: mutual TLS over QUIC, authorised by public key against a device whose role is `Relay`.

A device's permissions come from one closed, compiled-in enum of roles: `Admin`, `Releaser`, `Machine`, `BackupRestore`, `Relay`. The admin role passes every role check. Which endpoints each role may call is fixed in the route table.

Finer-grained checks sit on top of the role and are database rows: an application's `may_manage_dns` and `may_manage_tls` flags, a restore consumer's enabled row in `restore_replicas` for a group and type, and an identity grant linking a machine or application to a relying party.

### Tailscale user headers (humans)

The private server reads `Tailscale-User-Login` and `-Name` from headers the Tailscale ingress injects, trusted as-is with no signature. Security depends on the server being reachable only through that ingress.

Any tailnet user passes `TailscaleUser`, which about 76 handlers take: mostly reads, plus the read-only SQL console, snippet saving, safety-mode session control and the operator MCP mount. A handful of private handlers declare no identity extractor at all and are protected only by the tagged-device guard.

Admin status resolves from either of two sources:

- **The allow-list**: the `admins` table, an exact case-sensitive match on email, edited through the Admins page.
- **The tailnet policy**: a grant of the form `app["bes.au/cap/canopy"]` with `{admin:true}` whose destination contains `tag:server-canopy`. Both strings are compiled in. The policy is read from the Tailscale API every 60 seconds, and is authored only in the Tailscale policy file, never in Canopy.

In debug builds the extractors shortcut to a fixed `admin@localhost` without reading headers, unless `CANOPY_TRUST_TAILSCALE_HEADERS` is set. The shortcut is compiled out of release builds.

### MCP bearer tokens (agents off the tailnet)

A `canopy_mcp_` prefixed bearer token, stored and looked up by hash, with an expiry fixed at one year and a revocation timestamp. Its permissions are a `grants` array held to a compiled-in catalogue of exactly two values, `triage` and `notes`, enforced again by a database CHECK constraint. Grants are fixed at mint time and cannot be changed afterwards.

### Other secrets

The server-versions page and the upgrades calendar share one static environment secret. The OIDC identity-token issuer takes its issuer and keys from the environment, though its relying parties and grants are database rows. The restore operator reaches Canopy as a tailnet node with the backup-restore role, holding no key of its own. Backup credentials are short-lived STS credentials scoped to a group.

## Safety modes are not authorisation

Every route declares a grade: `read_only`, `write`, `danger(reasons)` or `public`. Write and danger need a session raised to at least that mode. But any tailnet user may raise to any mode, and a request carrying the `x-canopy-client` header is held to no grade at all. A grade authorises nobody: authorisation stays in the per-handler extractors. The two are orthogonal and stay that way.

## Where the card's problem shows

- **Nothing is dynamic where it matters.** The tag gate, the admin capability string, the certificate-header choice and the MCP grant catalogue are all compiled in or environment-only. An operator cannot name a new tagged device, or a new grant, without a deploy.
- **Permission granularity is coarse.** A device gets one of five roles, a human is admin or not, a token gets one of two grants. The scoped checks that do exist are each bespoke rows in a feature's own table, not a shared model.
- **The control plane is five pages.** Settings › Access holds Admins, MCP access, Certificate authority, Identity provider and Audit log, each covering one credential kind, none of them granting a scoped permission to a principal. Devices and their roles live in a different section entirely, and name-management grants live on an application's edit page.

## Tables holding identity and permission

`admins` (email), `tailscale_users` (login, display only), `operator_sessions` (login, mode, raise expiry), `devices` (role, tailnet node), `device_keys` (device, public key, active), `device_connections` (device, ip, user agent), `machine_enrollment_tokens` and `_challenges`, `mcp_tokens` (name, hash, grants, expiry, revoked, last used), `relying_parties` and `identity_grants`, `restore_replicas` and `restore_consumer_capabilities`, `kubernetes_clusters.relay_identity_id`, `audit_entries` (actor kind, id, role).

## Specs that cover this ground

`private-server/admin-access.md` (ADM), `private-server/device-trust.md` (DTR), `private-server/provisioned-credentials.md` (DPK), `private-server/safety-modes.md` (SAFE), `private-server/mcp.md` (MCP), `private-server/identity-provider.md` (IDP), `public-server/identity-tokens.md` (IDT), `public-server/machine-identity.md` (DID), `public-server/restore-replicas.md` (RST), `platform/audit-log.md` (AUD).
