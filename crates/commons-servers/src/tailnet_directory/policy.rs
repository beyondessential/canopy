//! What the tailnet policy grants on Canopy: access levels.
//!
//! Spec: `.workhorse/specs/private-server/access-levels.md` (id `ADM`), "The
//! Canopy grant" and "Resolving grant sources".

use std::collections::{HashMap, HashSet};

use database::access_lists::{ListLevel, normalise_login};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The Canopy application capability, whose values carry level keys, and the
/// service tag a grant must target for them to count.
pub const CANOPY_CAP: &str = "bes.au/cap/canopy";
pub const CANOPY_SERVICE_TAG: &str = "tag:server-canopy";

/// The subset of the tailnet policy file Canopy reads.
#[derive(Debug, Deserialize)]
pub(super) struct PolicyFile {
	#[serde(default)]
	groups: HashMap<String, Vec<String>>,
	#[serde(default)]
	grants: Vec<Grant>,
}

#[derive(Debug, Deserialize)]
struct Grant {
	#[serde(default)]
	src: Vec<String>,
	#[serde(default)]
	dst: Vec<String>,
	#[serde(default)]
	app: HashMap<String, Vec<serde_json::Value>>,
}

/// Who a grant's source covers, as the permissions page reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceCoverage {
	/// A group the policy defines, listing this many members.
	Group { members: usize },
	/// The one person the source names.
	Person,
	/// Everyone on the tailnet: `autogroup:member` and `*`.
	Everyone,
	/// No one: a group the policy does not define, or a source Canopy does not
	/// resolve to tailnet users.
	NoOne,
}

/// A source of a grant conferring a level, as the policy writes it, with who
/// it covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ConferringSource {
	/// The source as written in the grant.
	pub source: String,
	/// Who it resolves to.
	pub covers: SourceCoverage,
}

/// The callers a set of sources covers.
#[derive(Debug, Default, Clone)]
struct Coverage {
	/// Lowercased logins.
	logins: HashSet<String>,
	everyone: bool,
}

impl Coverage {
	fn covers(&self, login: &str) -> bool {
		self.everyone || self.logins.contains(login)
	}

	fn extend(&mut self, other: &Self) {
		self.everyone |= other.everyone;
		self.logins.extend(other.logins.iter().cloned());
	}
}

/// What the policy grants one level.
#[derive(Debug, Default, Clone)]
struct LevelGrants {
	coverage: Coverage,
	/// Each conferring source once, in the order the policy first names it.
	sources: Vec<ConferringSource>,
}

impl LevelGrants {
	fn add(&mut self, sources: &[(ConferringSource, Coverage)]) {
		for (source, coverage) in sources {
			self.coverage.extend(coverage);
			if !self.sources.iter().any(|s| s.source == source.source) {
				self.sources.push(source.clone());
			}
		}
	}
}

/// Everything one read of the policy grants on Canopy.
#[derive(Debug, Default, Clone)]
pub(super) struct PolicyGrants {
	admin: LevelGrants,
	pm: LevelGrants,
}

impl PolicyGrants {
	pub(super) fn resolve(policy: &PolicyFile) -> Self {
		let mut out = Self::default();
		for grant in &policy.grants {
			if !grant.dst.iter().any(|d| d == CANOPY_SERVICE_TAG) {
				continue;
			}
			let Some(values) = grant.app.get(CANOPY_CAP) else {
				continue;
			};
			let sources: Vec<_> = grant
				.src
				.iter()
				.map(|src| resolve_source(policy, src))
				.collect();

			let key_set = |key: &str| {
				values
					.iter()
					.any(|v| v.get(key).and_then(serde_json::Value::as_bool) == Some(true))
			};
			if key_set("admin") {
				out.admin.add(&sources);
			}
			if key_set("pm") {
				out.pm.add(&sources);
			}
		}
		out
	}

	fn level_grants(&self, level: ListLevel) -> &LevelGrants {
		match level {
			ListLevel::Admin => &self.admin,
			ListLevel::Pm => &self.pm,
		}
	}

	/// The highest level the policy grants a lowercased login.
	pub(super) fn level(&self, login: &str) -> Option<ListLevel> {
		ListLevel::ALL
			.into_iter()
			.find(|level| self.level_grants(*level).coverage.covers(login))
	}

	/// The sources conferring a level.
	pub(super) fn sources(&self, level: ListLevel) -> Vec<ConferringSource> {
		self.level_grants(level).sources.clone()
	}
}

/// Resolve one grant source to the callers it covers.
fn resolve_source(policy: &PolicyFile, src: &str) -> (ConferringSource, Coverage) {
	let mut coverage = Coverage::default();
	let covers = match src {
		"*" | "autogroup:member" => {
			coverage.everyone = true;
			SourceCoverage::Everyone
		}
		// Tagged devices never reach the operator surface, so a tagged source
		// covers no one who could hold a level.
		"autogroup:tagged" => SourceCoverage::NoOne,
		s if s.starts_with("autogroup:") || s.starts_with("tag:") => {
			tracing::warn!(source = %s, "canopy grant uses a source Canopy does not resolve; ignoring");
			SourceCoverage::NoOne
		}
		s if s.starts_with("group:") => match policy.groups.get(s) {
			Some(members) => {
				coverage
					.logins
					.extend(members.iter().map(|m| normalise_login(m)));
				SourceCoverage::Group {
					members: members.len(),
				}
			}
			None => {
				tracing::warn!(group = %s, "canopy grant references an undefined group; ignoring");
				SourceCoverage::NoOne
			}
		},
		s if s.contains('@') => {
			coverage.logins.insert(normalise_login(s));
			SourceCoverage::Person
		}
		s => {
			tracing::warn!(source = %s, "canopy grant uses a source Canopy does not resolve; ignoring");
			SourceCoverage::NoOne
		}
	};
	(
		ConferringSource {
			source: src.to_owned(),
			covers,
		},
		coverage,
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn resolve(json: serde_json::Value) -> PolicyGrants {
		PolicyGrants::resolve(&serde_json::from_value(json).unwrap())
	}

	#[test]
	fn group_source_resolves_to_members_whatever_their_case() {
		let grants = resolve(serde_json::json!({
			"groups": { "group:ops": ["Felix@BES.au", "sam@bes.au"] },
			"grants": [{
				"app": { "bes.au/cap/canopy": [{ "admin": true }] },
				"dst": ["tag:server-canopy"],
				"src": ["group:ops"],
			}],
		}));
		assert_eq!(grants.level("felix@bes.au"), Some(ListLevel::Admin));
		assert_eq!(grants.level("sam@bes.au"), Some(ListLevel::Admin));
		assert_eq!(grants.level("other@bes.au"), None);
	}

	#[test]
	fn pm_on_the_canopy_tag_confers_pm_and_on_another_tag_nothing() {
		let on_tag = resolve(serde_json::json!({
			"groups": { "group:pm": ["pat@bes.au"] },
			"grants": [{
				"app": { "bes.au/cap/canopy": [{ "pm": true }] },
				"dst": ["tag:server-canopy"],
				"src": ["group:pm"],
			}],
		}));
		assert_eq!(on_tag.level("pat@bes.au"), Some(ListLevel::Pm));

		let elsewhere = resolve(serde_json::json!({
			"groups": { "group:pm": ["pat@bes.au"] },
			"grants": [{
				"app": { "bes.au/cap/canopy": [{ "pm": true }] },
				"dst": ["tag:server-other"],
				"src": ["group:pm"],
			}],
		}));
		assert_eq!(elsewhere.level("pat@bes.au"), None);
	}

	#[test]
	fn both_keys_give_admin_and_neither_gives_nothing() {
		let both = resolve(serde_json::json!({
			"grants": [{
				"app": { "bes.au/cap/canopy": [{ "admin": true, "pm": true }] },
				"dst": ["tag:server-canopy"],
				"src": ["dana@bes.au"],
			}],
		}));
		assert_eq!(both.level("dana@bes.au"), Some(ListLevel::Admin));

		let neither = resolve(serde_json::json!({
			"grants": [{
				"app": { "bes.au/cap/canopy": [{ "admin": false }] },
				"dst": ["tag:server-canopy"],
				"src": ["dana@bes.au"],
			}],
		}));
		assert_eq!(neither.level("dana@bes.au"), None);
	}

	#[test]
	fn another_capability_confers_nothing() {
		let grants = resolve(serde_json::json!({
			"grants": [{
				"app": { "bes.au/cap/other": [{ "admin": true }] },
				"dst": ["tag:server-canopy"],
				"src": ["dana@bes.au"],
			}],
		}));
		assert_eq!(grants.level("dana@bes.au"), None);
	}

	#[test]
	fn star_and_autogroup_member_cover_everyone() {
		for source in ["*", "autogroup:member"] {
			let grants = resolve(serde_json::json!({
				"grants": [{
					"app": { "bes.au/cap/canopy": [{ "pm": true }] },
					"dst": ["tag:server-canopy"],
					"src": [source],
				}],
			}));
			assert_eq!(
				grants.level("anyone@bes.au"),
				Some(ListLevel::Pm),
				"{source}"
			);
		}
	}

	#[test]
	fn unresolved_sources_cover_no_one() {
		let grants = resolve(serde_json::json!({
			"grants": [{
				"app": { "bes.au/cap/canopy": [{ "admin": true }] },
				"dst": ["tag:server-canopy"],
				"src": ["autogroup:tagged", "tag:server-canopy", "autogroup:admin", "group:missing"],
			}],
		}));
		assert_eq!(grants.level("anyone@bes.au"), None);
	}

	#[test]
	fn sources_are_reported_as_written_with_who_they_cover() {
		let grants = resolve(serde_json::json!({
			"groups": { "group:pm": ["a@bes.au", "b@bes.au", "c@bes.au"] },
			"grants": [
				{
					"app": { "bes.au/cap/canopy": [{ "pm": true }] },
					"dst": ["tag:server-canopy"],
					"src": ["group:pm", "Dana@BES.au", "*", "autogroup:member", "group:missing", "autogroup:admin"],
				},
				{
					"app": { "bes.au/cap/canopy": [{ "pm": true }] },
					"dst": ["tag:server-canopy"],
					"src": ["group:pm"],
				},
			],
		}));
		assert_eq!(
			grants.sources(ListLevel::Pm),
			vec![
				ConferringSource {
					source: "group:pm".into(),
					covers: SourceCoverage::Group { members: 3 },
				},
				ConferringSource {
					source: "Dana@BES.au".into(),
					covers: SourceCoverage::Person,
				},
				ConferringSource {
					source: "*".into(),
					covers: SourceCoverage::Everyone,
				},
				ConferringSource {
					source: "autogroup:member".into(),
					covers: SourceCoverage::Everyone,
				},
				ConferringSource {
					source: "group:missing".into(),
					covers: SourceCoverage::NoOne,
				},
				ConferringSource {
					source: "autogroup:admin".into(),
					covers: SourceCoverage::NoOne,
				},
			],
		);
		assert!(grants.sources(ListLevel::Admin).is_empty());
	}
}
