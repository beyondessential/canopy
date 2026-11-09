//! Integration tests that run against a Kubernetes cluster with MinIO and kopia.
//! Every test is ignored by default; CI runs them one module at a time.

mod canopy_integration;
mod ephemeral;
mod error_paths;
mod extra_user_grants;
mod extra_users;
mod helpers;
mod lifecycle;
mod locale_fix;
mod migration;
mod persistent_schemas;
mod redaction;
mod schema_build;
mod switchover;
mod ttls;
