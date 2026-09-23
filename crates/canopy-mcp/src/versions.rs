//! `list_versions` / `get_version` tools.

use std::collections::HashMap;

use commons_types::{Uuid, version::VersionStr};
use database::{
	applications::Application, diesel_async::AsyncPgConnection, statuses::Status,
	version_known_issues::VersionKnownIssue, versions::Version,
};
use jiff::Timestamp;
use rmcp::{
	handler::server::wrapper::Parameters,
	model::{CallToolResult, ErrorData as McpError},
	tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
	CanopyMcp,
	util::{mcp_err, not_found, ok_json},
};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListVersionsArgs {
	/// Include draft (unpublished) versions. Defaults to false.
	pub include_drafts: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct VersionArgs {
	/// The Tamanu version, e.g. `2.34.1`.
	pub version: String,
}

#[derive(Serialize)]
struct VersionList {
	versions: Vec<VersionSummary>,
}

#[derive(Serialize)]
struct VersionSummary {
	version: String,
	status: String,
	head_release_date: Option<Timestamp>,
	changelog_summary: String,
	/// Live applications currently reporting this version.
	adoption: u32,
}

#[derive(Serialize)]
struct ServerRef {
	id: Uuid,
	name: Option<String>,
}

#[derive(Serialize)]
struct VersionDetail {
	version: String,
	status: String,
	head_release_date: Option<Timestamp>,
	changelog: String,
	known_issues: Vec<VersionKnownIssue>,
	available_updates: Vec<String>,
	adoption_count: usize,
	adopting_servers: Vec<ServerRef>,
}

#[tool_router(router = versions_router, vis = "pub(crate)")]
impl CanopyMcp {
	#[tool(
		// Every tool reads; none changes anything (see the SAFE spec).
		annotations(read_only_hint = true),
		description = "List known Tamanu versions with release date, changelog summary, and how \
		               many live applications currently run each."
	)]
	async fn list_versions(
		&self,
		Parameters(args): Parameters<ListVersionsArgs>,
	) -> Result<CallToolResult, McpError> {
		let mut conn = self.conn().await?;
		let versions = if args.include_drafts.unwrap_or(false) {
			Version::get_all_including_drafts(&mut conn).await
		} else {
			Version::get_all(&mut conn).await
		}
		.map_err(mcp_err)?;

		let adoption = self.version_adoption(&mut conn).await?;

		let mut out = Vec::with_capacity(versions.len());
		for v in &versions {
			let vs = version_str(v);
			let head_release_date = match vs.parse::<VersionStr>() {
				Ok(p) => Version::get_head_release_date(&mut conn, p).await.ok(),
				Err(_) => None,
			};
			out.push(VersionSummary {
				version: vs.clone(),
				status: v.status.to_string(),
				head_release_date,
				changelog_summary: first_line(&v.changelog),
				adoption: adoption.get(&vs).copied().unwrap_or(0),
			});
		}
		ok_json(&VersionList { versions: out })
	}

	#[tool(
		// Every tool reads; none changes anything (see the SAFE spec).
		annotations(read_only_hint = true),
		description = "Detail for one Tamanu version: changelog, known issues, available updates, \
		               and which live applications run it."
	)]
	async fn get_version(
		&self,
		Parameters(args): Parameters<VersionArgs>,
	) -> Result<CallToolResult, McpError> {
		let mut conn = self.conn().await?;
		let vs = args.version.parse::<VersionStr>().map_err(|_| {
			McpError::invalid_params(format!("invalid version: {}", args.version), None)
		})?;

		let Ok(version) = Version::get_by_version(&mut conn, vs.clone()).await else {
			return Ok(not_found(format!("no version {vs}")));
		};

		let known_issues =
			VersionKnownIssue::list_for_minor(&mut conn, version.major, version.minor)
				.await
				.map_err(mcp_err)?;
		let available_updates = Version::get_updates_for_version(&mut conn, vs.clone())
			.await
			.map_err(mcp_err)?
			.into_iter()
			.map(|u| format!("{}.{}.{}", u.major, u.minor, u.patch))
			.collect();
		let head_release_date = Version::get_head_release_date(&mut conn, vs.clone())
			.await
			.ok();

		// Adoption: live applications whose latest status reports this version.
		let applications = Application::get_all(&mut conn, 0, None)
			.await
			.map_err(mcp_err)?;
		let ids: Vec<Uuid> = applications.iter().map(|s| s.id).collect();
		let statuses = Status::latest_for_servers(&mut conn, &ids)
			.await
			.map_err(mcp_err)?;
		let target = vs.to_string();
		let on_version: std::collections::HashSet<Uuid> = statuses
			.iter()
			.filter(|s| s.version.as_ref().map(|v| v.to_string()) == Some(target.clone()))
			.filter_map(|s| s.server_id)
			.collect();
		let adopting_servers: Vec<ServerRef> = applications
			.iter()
			.filter(|s| on_version.contains(&s.id))
			.map(|s| ServerRef {
				id: s.id,
				name: s.name.clone(),
			})
			.collect();

		ok_json(&VersionDetail {
			version: target,
			status: version.status.to_string(),
			head_release_date,
			changelog: version.changelog.clone(),
			known_issues,
			available_updates,
			adoption_count: adopting_servers.len(),
			adopting_servers,
		})
	}
}

impl CanopyMcp {
	/// Count of live applications reporting each version (by version string).
	async fn version_adoption(
		&self,
		conn: &mut AsyncPgConnection,
	) -> Result<HashMap<String, u32>, McpError> {
		let applications = Application::get_all(conn, 0, None).await.map_err(mcp_err)?;
		let ids: Vec<Uuid> = applications.iter().map(|s| s.id).collect();
		let statuses = Status::latest_for_servers(conn, &ids)
			.await
			.map_err(mcp_err)?;
		let mut adoption: HashMap<String, u32> = HashMap::new();
		for st in &statuses {
			if let Some(v) = &st.version {
				*adoption.entry(v.to_string()).or_default() += 1;
			}
		}
		Ok(adoption)
	}
}

fn version_str(v: &Version) -> String {
	format!("{}.{}.{}", v.major, v.minor, v.patch)
}

fn first_line(s: &str) -> String {
	s.lines().next().unwrap_or("").trim().to_string()
}
