//! Canopy's own work for Ansible: reading the variable catalogue infra
//! publishes, keeping leases honest, and forgetting a secret's earlier value
//! once it has been held for its week.

pub mod catalogue;
pub mod held;
pub mod leases;
