//! Vulnerability scanning, and the model that triages what it finds.
//!
//! Spec: `.workhorse/specs/security/vulnerabilities.md` (id `VUL`) and
//! `.workhorse/specs/security/vulnerability-triage.md` (id `VTR`).

pub mod ai;
pub mod lockfile;
pub mod osv_client;
pub mod repository;
pub mod scan;
