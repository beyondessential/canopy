// All integration tests for this crate compile into this single binary.
// One test target links (and writes to disk) once instead of once per
// file, which keeps rebuilds from swamping the machine with I/O.
// Nextest still runs every #[tokio::test] in parallel as usual.

mod artifact_sweep;
mod domains;
mod relay;
mod relay_end_to_end;
mod relay_ingest;
mod session_sweep;
