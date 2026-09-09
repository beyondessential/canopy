// All integration tests for this crate compile into this single binary.
// One test target links (and writes to disk) once instead of once per
// file, which keeps rebuilds from swamping the machine with I/O.
// Nextest still runs every #[tokio::test] in parallel as usual.

mod application_host;
mod application_paths;
mod application_types;
mod artifacts;
mod backups;
mod bestool;
mod certificates;
mod device_admin_endpoints;
mod device_keys;
mod devices;
mod domains;
mod endpoints;
mod enrollment_ticket;
mod group_card_version;
mod health;
mod healthchecks;
mod issues;
mod maintenance;
mod maintenance_targets;
mod mcp;
mod migration_tests;
mod notes;
mod openapi_spec;
mod operator_presence;
mod private_statuses;
mod provision_credential;
mod restore_replicas;
mod server_version_distance;
mod sql;
mod tagged_device_guard;
mod tailnet_device_auth;
mod tailnet_key_expiry_sweep;
mod tailscale_header;
mod update_server;
mod upgrade_plans;
mod version_known_issues;
mod versions;
