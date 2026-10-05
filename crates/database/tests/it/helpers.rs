//! Fixtures shared across the integration tests.

use std::collections::BTreeSet;

use commons_errors::Result;
use commons_types::{namespace::Namespace, server::app_type::ApplicationType};
use database::{applications::Application, diesel_async::AsyncPgConnection, machines::Machine};
use uuid::Uuid;

/// One application type's namespace.
///
/// Most catalog tests exercise grading, liveness or scoping rather than the
/// namespacing itself, and the names they use (`disk_space`, `x`) are
/// application-subject ones, so this is the namespace ingest would file them
/// in. Tests that are *about* the namespacing name their namespaces inline.
pub fn app_ns() -> Namespace {
	Namespace::Application(ApplicationType::TamanuCentral)
}

/// The checks silenced for the application `id` under `source`.
pub async fn silenced_of_application(
	conn: &mut AsyncPgConnection,
	id: Uuid,
	source: &str,
) -> Result<BTreeSet<String>> {
	let application = Application::get_by_id(conn, id).await?;
	database::silenced_refs::silenced_health_checks_of_application(conn, &application, source).await
}

/// The checks silenced for the machine `id` under `source`.
pub async fn silenced_of_machine(
	conn: &mut AsyncPgConnection,
	id: Uuid,
	source: &str,
) -> Result<BTreeSet<String>> {
	let machine = Machine::get_by_id(conn, id).await?;
	database::silenced_refs::silenced_health_checks_of_machine(conn, &machine, source).await
}
