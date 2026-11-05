//! OSV's HTTP API: which advisories affect a package version, and an
//! advisory's record.

use std::time::Duration;

use commons_errors::{AppError, Result};
use database::osv::{Counted, Record};
use serde::{Deserialize, Serialize};

/// OSV's public API.
pub const BASE_URL: &str = "https://api.osv.dev";

/// The most queries OSV takes in one batch.
const BATCH: usize = 1000;

#[derive(Clone)]
pub struct OsvClient {
	http: reqwest::Client,
	base: String,
}

/// An advisory OSV says affects a package version, with when it was last
/// revised.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct VulnRef {
	pub id: String,
	pub modified: String,
}

#[derive(Serialize)]
struct Query<'a> {
	package: QueryPackage<'a>,
	version: &'a str,
	#[serde(skip_serializing_if = "Option::is_none")]
	page_token: Option<&'a str>,
}

#[derive(Serialize)]
struct QueryPackage<'a> {
	name: &'a str,
	ecosystem: &'a str,
}

#[derive(Serialize)]
struct Batch<'a> {
	queries: Vec<Query<'a>>,
}

#[derive(Default, Deserialize)]
struct BatchResponse {
	#[serde(default)]
	results: Vec<QueryResult>,
}

#[derive(Default, Deserialize)]
struct QueryResult {
	#[serde(default)]
	vulns: Vec<VulnRef>,
	next_page_token: Option<String>,
}

fn upstream(what: &str, e: impl std::fmt::Display) -> AppError {
	AppError::Upstream(format!("OSV {what}: {e}"))
}

impl OsvClient {
	pub fn new(base: impl Into<String>) -> Self {
		Self {
			http: reqwest::Client::builder()
				.timeout(Duration::from_secs(60))
				.build()
				.unwrap_or_default(),
			base: base.into().trim_end_matches('/').to_owned(),
		}
	}

	/// The advisories affecting each package version, in the order asked,
	/// following each query's pages through to the end.
	pub async fn query(&self, asked: &[Counted]) -> Result<Vec<Vec<VulnRef>>> {
		let mut answers: Vec<Vec<VulnRef>> = vec![Vec::new(); asked.len()];
		for (offset, chunk) in asked.chunks(BATCH).enumerate().map(|(i, c)| (i * BATCH, c)) {
			// Index within `asked` and the page to ask for next.
			let mut pending: Vec<(usize, Option<String>)> =
				(0..chunk.len()).map(|i| (offset + i, None)).collect();
			while !pending.is_empty() {
				let batch = Batch {
					queries: pending
						.iter()
						.map(|(index, token)| {
							let c = &asked[*index];
							Query {
								package: QueryPackage {
									name: &c.name,
									ecosystem: &c.ecosystem,
								},
								version: &c.version,
								page_token: token.as_deref(),
							}
						})
						.collect(),
				};
				let response: BatchResponse = self
					.http
					.post(format!("{}/v1/querybatch", self.base))
					.json(&batch)
					.send()
					.await
					.map_err(|e| upstream("querybatch", e))?
					.error_for_status()
					.map_err(|e| upstream("querybatch", e))?
					.json()
					.await
					.map_err(|e| upstream("querybatch", e))?;
				if response.results.len() != pending.len() {
					return Err(upstream(
						"querybatch",
						format!(
							"answered {} of {} queries",
							response.results.len(),
							pending.len()
						),
					));
				}

				let mut next = Vec::new();
				for ((index, _), result) in pending.into_iter().zip(response.results) {
					answers[index].extend(result.vulns);
					if let Some(token) = result.next_page_token.filter(|t| !t.is_empty()) {
						next.push((index, Some(token)));
					}
				}
				pending = next;
			}
		}
		Ok(answers)
	}

	/// An advisory's record.
	pub async fn get(&self, id: &str) -> Result<Record> {
		self.http
			.get(format!("{}/v1/vulns/{id}", self.base))
			.send()
			.await
			.map_err(|e| upstream("vulns", e))?
			.error_for_status()
			.map_err(|e| upstream("vulns", e))?
			.json()
			.await
			.map_err(|e| upstream("vulns", e))
	}
}
