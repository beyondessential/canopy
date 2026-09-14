// All integration tests for this crate compile into this single binary.
// One test target links (and writes to disk) once instead of once per
// file, which keeps rebuilds from swamping the machine with I/O.
// Nextest still runs every #[tokio::test] in parallel as usual.

mod artifact_sweep;
mod domains;
