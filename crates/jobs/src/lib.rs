//! Shared library for the backup scheduler binaries: the in-process kopia
//! execution layer + the scheduler/monitor loops. Kept here (not in
//! `commons-applications`) so `kube`/`k8s-openapi` stay off the crates that don't
//! read k8s Secrets; the pure scheduler helpers (jitter, due-ness, billing,
//! retention floor) live in `commons_servers::backup_jobs`.

pub mod artifact_sweep;
pub mod backup;
pub mod domains;
