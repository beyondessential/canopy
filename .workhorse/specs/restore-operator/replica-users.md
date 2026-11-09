---
id: USER
---

# Replica database users

The login roles pgro provisions in each restored database, and how their credentials are exposed for connection.

Roles are provisioned once, as part of the restore's initialisation, rather than continuously reconciled.
A change to a replica's users takes effect on the next restore.
pgro does not alter roles in a restore that is already running, and never drops a role it previously created, which leaves whatever process uses the database free to make its own user customisations without the operator overwriting them.

## Analytics user

The primary login role is named `analytics`.
pgro generates its password and stores it, with the username, in the replica's credentials Secret `<replica>-creds`, under the keys `username` and `password`.
The replica's connection details are surfaced in its status.

The role is reset to a fixed state on every restore, and created first if it does not exist.
Its attributes are `LOGIN` and none of `SUPERUSER`, `CREATEDB`, `CREATEROLE`, `REPLICATION` or `BYPASSRLS`, every role membership it held is revoked, and its password is set.
A role of that name in the source cluster is reset the same way, so it does not keep production's attributes or grants.

Permissions are then granted on top of that base, following whether the restore stays read-only in effect.
On PostgreSQL 14 or later, a replica with no persistent schemas, no redaction and no migration target is granted `pg_read_all_data`.
Every other replica is granted `SUPERUSER` for at least the restore's initial provisioning, including any replica on a PostgreSQL version below 14.

`pg_read_all_data` covers `SELECT` on every table, view and sequence and `USAGE` on every schema, but not the execution of functions.
Function execution reaches the analytics user by default only through `PUBLIC`, which the database-wide lockdown below closes, so the `pg_read_all_data` case also grants `EXECUTE` on every function.

Redaction is the only thing that demotes the role afterwards, and only on a replica with no persistent schemas.
A migration target does not affect that demotion, so a replica with both redaction and a migration target ends at `pg_read_all_data` once redaction completes.
A replica with persistent schemas, or with a migration target and no redaction, keeps the role at `SUPERUSER` for as long as the restore runs.

> [!NOTE]
> Holding `SUPERUSER` for the life of a persistent-schemas or migrating replica is a known limit of the approach, not a requirement.

## Extra users

A replica may carry additional login roles, declared through the `extra_users` parameter of the `analytics` intent (see [INT](intents.md)).
The parameter is a comma-separated list, each entry either `name`, with no schema access, or `name:schema1+schema2`, with a `+`-joined list of schemas after a colon, for example `reporting,dbt_reader:dbt+staging`.

Each named role is reset to a fixed state on every restore, and created first if it does not exist, as the analytics user is, so a role of that name in the source cluster does not keep production's attributes or grants.
It ends up with `LOGIN` and none of `SUPERUSER`, `CREATEDB`, `CREATEROLE`, `REPLICATION` or `BYPASSRLS`, and every role membership it held is revoked.
It can connect and read the system catalogues and do nothing else until its own schemas are granted back, or something else grants it more.
pgro does not decide what these roles may see beyond their schemas, and because every restore is a fresh database, any grant made to one by other means has to be reapplied to the next restore.

An extra user's sessions are read-only whatever the replica's own mode, so a grant made later widens what the role can read without letting it write.
That applies to the extra user's own sessions only.

Each schema in an extra user's list is granted `USAGE`, and `SELECT` on all its current tables and sequences, in each database where the schema exists, and is skipped silently in a database where it does not.
Default privileges also grant `SELECT` on tables the analytics role creates in that schema afterwards, so tables the persistent-schemas migration creates, which runs as the analytics role, are covered without a second grant step once it finishes.
That covers only objects the analytics role creates, and a table created in the schema by another role needs its own grant.
These grants are applied last, after the database-wide lockdown below strips each role's inherited ownership and direct grants, because granting first would have the strip revoke the grant immediately.

pgro generates a password for each extra user and stores it in a Secret of its own, named `<replica>-user-<slugged name>-creds`.
The username is slugged to fit Kubernetes object names, and the Secret's `username` key carries the real, unslugged role name.
The Secret is owned by the replica, so it is cleaned up when the replica is deleted, and it is created before the restore's initialisation runs, so the generated password is available when the role is created.
Extra users are not surfaced in the replica's status, and each one's Secret name is derived from the replica's name and the username.
An extra user whose name matches the analytics user is ignored, since that role is already provisioned.

## Database-wide lockdown

A restore is physical, so the snapshot carries the source cluster's catalogue rows as they were, not only its data.
Role attributes are reset above, but ownership, direct grants and anything granted broadly to `PUBLIC` survive unless something removes them.
A single lockdown step removes them, and it runs in every connectable database of every restore, whether or not the replica has extra users, and regardless of persistent schemas, redaction or a migration target.

> [!NOTE]
> The lockdown runs unconditionally, so a later reconcile never has to answer whether it was applied for a given restore.
> Gating it on extra users would leave a replica that once had extra users and later has none with `PUBLIC`'s grants revoked for good, with nothing recording it and no way back, since removing the last extra user gives no signal to reverse it.

For the analytics user and each extra user, every object the role owns is handed to the `postgres` superuser, keeping the objects rather than dropping them, and whatever direct grants remain are then revoked.
Both steps do nothing for a role with nothing owned or granted, so they run the same way whether the role is fresh or collided with a production one.

Privileges are additive in PostgreSQL and there is no per-role deny, so a privilege `PUBLIC` holds cannot be taken from one role, and the grant has to come off `PUBLIC` itself.
The lockdown therefore revokes `PUBLIC`'s privileges on every schema except the system ones, on every table, view, sequence, materialised view and foreign table, and on every function, along with its default privileges for objects not yet created.
It reaches every such object, and not only those in the `public` schema.

The system catalogues, the information schema and the temporary and toast schemas are excluded throughout, since access to the system catalogues through `PUBLIC` is the one thing an otherwise ungranted role is meant to keep.
Nothing the restore is meant to reach is lost to the lockdown.
The analytics user holds `SUPERUSER`, which bypasses every access check the lockdown touches, or `pg_read_all_data` with the explicit `EXECUTE` grant above, and every role restored from the source cluster has had its password cleared by then, so none of them can authenticate to use a privilege `PUBLIC` might otherwise still give it.

The `template1` database is included in the lockdown.
A database created without a named template is copied byte for byte from it, so leaving it out would have every database created later in that instance start from the open default again.

Last, each extra user with a non-empty schema list has its schemas granted back, as under "Extra users".
