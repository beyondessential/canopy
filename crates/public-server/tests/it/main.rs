// All integration tests for this crate compile into this single binary.
// One test target links (and writes to disk) once instead of once per
// file, which keeps rebuilds from swamping the machine with I/O.
// Nextest still runs every #[tokio::test] in parallel as usual.

mod application_aliases;
mod artifact_scopes;
mod auth_requirements;
mod backup;
mod backup_secrets;
mod calendar;
mod check_severities;
mod device_key_auth;
mod error_scenarios;
mod health;
mod index;
mod machine_enrollment;
mod mcp;
mod names;
mod openapi_spec;
mod password;
mod reporting_schemas;
mod restore;
mod server_self;
mod server_versions;
mod servers_list;
mod static_files;
mod statuses;
mod statuses_split;
mod tags;
mod timesync;
mod versions;
