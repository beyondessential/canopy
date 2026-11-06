//! What counts, and which advisories match it.

use std::collections::HashMap;

use commons_errors::Result;
use commons_types::server::rank::ServerRank;
use diesel::sql_types;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use uuid::Uuid;

use super::{Source, items::EXPOSURE_CTE};
use crate::osv::Counted;

#[derive(diesel::QueryableByName)]
struct CountedRow {
	#[diesel(sql_type = sql_types::Text)]
	source: String,
	#[diesel(sql_type = sql_types::Text)]
	ecosystem: String,
	#[diesel(sql_type = sql_types::Text)]
	name: String,
	#[diesel(sql_type = sql_types::Text)]
	version: String,
}

/// The package versions each source carries now: a repository's, from its
/// default branch as last read, and Linux's, from the live production machines
/// that have reported packages.
pub async fn counted_by_source(
	conn: &mut AsyncPgConnection,
) -> Result<HashMap<Source, Vec<Counted>>> {
	let rows: Vec<CountedRow> = diesel::sql_query(
		"SELECT repository AS source, ecosystem, name, version FROM repository_packages \
		 UNION \
		 SELECT 'linux', r.ecosystem, p.name, p.version \
		 FROM machine_packages p \
		 JOIN machine_package_reports r USING (machine_id) \
		 JOIN machines m ON m.id = p.machine_id \
		 WHERE m.deleted_at IS NULL AND m.rank = $1 AND r.ecosystem IS NOT NULL",
	)
	.bind::<sql_types::Text, _>(ServerRank::Production.to_string())
	.load(conn)
	.await?;
	let mut out: HashMap<Source, Vec<Counted>> = HashMap::new();
	for row in rows {
		let Some(source) = Source::parse(&row.source) else {
			continue;
		};
		out.entry(source).or_default().push(Counted {
			ecosystem: row.ecosystem,
			name: row.name,
			version: row.version,
		});
	}
	Ok(out)
}

/// The distinct package versions to ask OSV about, however many repositories
/// or machines carry each.
pub async fn counted_packages(conn: &mut AsyncPgConnection) -> Result<Vec<Counted>> {
	let mut all: Vec<Counted> = counted_by_source(conn)
		.await?
		.into_values()
		.flatten()
		.collect();
	all.sort();
	all.dedup();
	Ok(all)
}

/// The live production machines that have reported packages, which a change in
/// makes a scan due.
pub async fn production_machines(conn: &mut AsyncPgConnection) -> Result<Vec<Uuid>> {
	#[derive(diesel::QueryableByName)]
	struct Row {
		#[diesel(sql_type = sql_types::Uuid)]
		id: Uuid,
	}
	let rows: Vec<Row> = diesel::sql_query(
		"SELECT m.id FROM machines m \
		 JOIN machine_package_reports r ON r.machine_id = m.id \
		 WHERE m.deleted_at IS NULL AND m.rank = $1 ORDER BY m.id",
	)
	.bind::<sql_types::Text, _>(ServerRank::Production.to_string())
	.load(conn)
	.await?;
	Ok(rows.into_iter().map(|row| row.id).collect())
}

/// Replace what advisories match each queried package version with what the
/// last OSV query for it found. The advisories found must already be stored.
pub async fn replace_matches(
	conn: &mut AsyncPgConnection,
	answers: &[(Counted, Vec<String>)],
) -> Result<()> {
	let (mut ecosystems, mut names, mut versions) = (Vec::new(), Vec::new(), Vec::new());
	let mut ids = Vec::new();
	let (mut found_ecosystems, mut found_names, mut found_versions) =
		(Vec::new(), Vec::new(), Vec::new());
	for (queried, found) in answers {
		ecosystems.push(queried.ecosystem.as_str());
		names.push(queried.name.as_str());
		versions.push(queried.version.as_str());
		for id in found {
			ids.push(id.as_str());
			found_ecosystems.push(queried.ecosystem.as_str());
			found_names.push(queried.name.as_str());
			found_versions.push(queried.version.as_str());
		}
	}

	diesel::sql_query(
		"DELETE FROM vulnerability_matches m \
		 USING unnest($1::text[], $2::text[], $3::text[]) AS q(e, p, v) \
		 WHERE m.ecosystem = q.e AND m.package = q.p AND m.package_version = q.v \
		 AND NOT EXISTS ( \
		   SELECT 1 FROM unnest($4::text[], $5::text[], $6::text[], $7::text[]) AS f(id, e, p, v) \
		   WHERE f.id = m.vulnerability_id AND f.e = m.ecosystem \
		   AND f.p = m.package AND f.v = m.package_version)",
	)
	.bind::<sql_types::Array<sql_types::Text>, _>(&ecosystems)
	.bind::<sql_types::Array<sql_types::Text>, _>(&names)
	.bind::<sql_types::Array<sql_types::Text>, _>(&versions)
	.bind::<sql_types::Array<sql_types::Text>, _>(&ids)
	.bind::<sql_types::Array<sql_types::Text>, _>(&found_ecosystems)
	.bind::<sql_types::Array<sql_types::Text>, _>(&found_names)
	.bind::<sql_types::Array<sql_types::Text>, _>(&found_versions)
	.execute(conn)
	.await?;

	diesel::sql_query(
		"INSERT INTO vulnerability_matches (vulnerability_id, ecosystem, package, package_version) \
		 SELECT * FROM unnest($1::text[], $2::text[], $3::text[], $4::text[]) \
		 ON CONFLICT DO NOTHING",
	)
	.bind::<sql_types::Array<sql_types::Text>, _>(&ids)
	.bind::<sql_types::Array<sql_types::Text>, _>(&found_ecosystems)
	.bind::<sql_types::Array<sql_types::Text>, _>(&found_names)
	.bind::<sql_types::Array<sql_types::Text>, _>(&found_versions)
	.execute(conn)
	.await?;
	Ok(())
}

/// Drop matches for package versions no longer in `keep`.
pub async fn prune_matches(conn: &mut AsyncPgConnection, keep: &[Counted]) -> Result<usize> {
	let ecosystems: Vec<&str> = keep.iter().map(|c| c.ecosystem.as_str()).collect();
	let names: Vec<&str> = keep.iter().map(|c| c.name.as_str()).collect();
	let versions: Vec<&str> = keep.iter().map(|c| c.version.as_str()).collect();
	let pruned = diesel::sql_query(
		"DELETE FROM vulnerability_matches m WHERE NOT EXISTS ( \
		   SELECT 1 FROM unnest($1::text[], $2::text[], $3::text[]) AS k(e, p, v) \
		   WHERE k.e = m.ecosystem AND k.p = m.package AND k.v = m.package_version)",
	)
	.bind::<sql_types::Array<sql_types::Text>, _>(&ecosystems)
	.bind::<sql_types::Array<sql_types::Text>, _>(&names)
	.bind::<sql_types::Array<sql_types::Text>, _>(&versions)
	.execute(conn)
	.await?;
	Ok(pruned)
}

/// Make an item of every advisory a source carries a package version of, and
/// refresh the text the list searches, for every item something counts for.
/// Items are kept when nothing counts for them any more, with the text they
/// had.
pub async fn refresh_items(conn: &mut AsyncPgConnection) -> Result<()> {
	diesel::sql_query(format!(
		"WITH {EXPOSURE_CTE} \
		 INSERT INTO vulnerability_items (vulnerability_id, source) \
		 SELECT DISTINCT vulnerability_id, source FROM exposure \
		 ON CONFLICT DO NOTHING"
	))
	.bind::<sql_types::Nullable<sql_types::Array<sql_types::Text>>, _>(None::<Vec<String>>)
	.bind::<sql_types::Text, _>(ServerRank::Production.to_string())
	.execute(conn)
	.await?;

	diesel::sql_query(format!(
		"WITH {EXPOSURE_CTE}, \
		 packages AS ( \
		   SELECT vulnerability_id, source, string_agg(DISTINCT name, ' ') AS names \
		   FROM exposure GROUP BY 1, 2), \
		 workspaces AS ( \
		   SELECT e.vulnerability_id, e.source, string_agg(DISTINCT p.workspace_package, ' ') AS names \
		   FROM exposure e JOIN repository_package_paths p \
		     ON p.repository = e.source AND p.ecosystem = e.ecosystem \
		    AND p.name = e.name AND p.version = e.version \
		   GROUP BY 1, 2), \
		 text AS ( \
		   SELECT k.vulnerability_id, k.source, \
		     concat_ws(' ', k.vulnerability_id, array_to_string(v.aliases, ' '), v.summary, \
		               k.source, k.names, w.names) AS search_text \
		   FROM packages k \
		   JOIN vulnerabilities v ON v.id = k.vulnerability_id \
		   LEFT JOIN workspaces w USING (vulnerability_id, source)) \
		 UPDATE vulnerability_items i SET search_text = t.search_text \
		 FROM text t \
		 WHERE i.vulnerability_id = t.vulnerability_id AND i.source = t.source \
		 AND i.search_text IS DISTINCT FROM t.search_text"
	))
	.bind::<sql_types::Nullable<sql_types::Array<sql_types::Text>>, _>(None::<Vec<String>>)
	.bind::<sql_types::Text, _>(ServerRank::Production.to_string())
	.execute(conn)
	.await?;
	Ok(())
}
