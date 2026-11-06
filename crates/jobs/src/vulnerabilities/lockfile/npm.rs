//! Reading a repository's npm dependencies from a `package-lock.json`.
//!
//! Spec: `.workhorse/specs/security/vulnerabilities.md` (id `VUL`), "Where a
//! package sits".
//!
//! Lockfile v2 and v3 have one entry per installed package, keyed by its
//! location (`node_modules/a/node_modules/b`), plus one per workspace
//! (`packages/server`) and one for the root (`""`). Every installed entry is a
//! package, development-only ones included. From each workspace package's
//! `dependencies`, `devDependencies` and `optionalDependencies` the tree is
//! walked, resolving each dependency by node's rule, to find which workspace
//! packages reach each package, directly or not, by which scope, and how. A
//! workspace package's `devDependencies` give development paths, its
//! `optionalDependencies` give optional runtime paths, and a path keeps the
//! scope it started with all the way down.

use std::collections::BTreeMap;

use database::packages::{NPM, RepositoryPackage, Scope};
use serde::Deserialize;

use super::{Edge, Found, ParseError, walk};

#[derive(Debug, Deserialize)]
struct Lockfile {
	#[serde(rename = "lockfileVersion")]
	version: u32,
	packages: BTreeMap<String, Entry>,
}

#[derive(Debug, Default, Deserialize)]
struct Entry {
	name: Option<String>,
	version: Option<String>,
	#[serde(default)]
	link: bool,
	resolved: Option<String>,
	#[serde(default)]
	dependencies: BTreeMap<String, String>,
	#[serde(default, rename = "devDependencies")]
	dev_dependencies: BTreeMap<String, String>,
	#[serde(default, rename = "optionalDependencies")]
	optional_dependencies: BTreeMap<String, String>,
	#[serde(default, rename = "peerDependencies")]
	peer_dependencies: BTreeMap<String, String>,
	#[serde(default, rename = "peerDependenciesMeta")]
	peer_dependencies_meta: BTreeMap<String, PeerMeta>,
}

#[derive(Debug, Default, Deserialize)]
struct PeerMeta {
	#[serde(default)]
	optional: bool,
}

impl Entry {
	/// The dependencies a package brings with it when installed as a
	/// dependency, with whether each is optional. `optionalDependencies` wins
	/// over `dependencies` where both name a package, and an optional peer is
	/// not installed.
	fn brings(&self) -> impl Iterator<Item = (&str, bool)> {
		let plain = self
			.dependencies
			.keys()
			.filter(|name| !self.optional_dependencies.contains_key(*name))
			.map(|name| (name.as_str(), false));
		let optional = self
			.optional_dependencies
			.keys()
			.map(|name| (name.as_str(), true));
		let peers = self
			.peer_dependencies
			.keys()
			.filter(|name| {
				!self.dependencies.contains_key(*name)
					&& !self.optional_dependencies.contains_key(*name)
					&& !self
						.peer_dependencies_meta
						.get(*name)
						.is_some_and(|meta| meta.optional)
			})
			.map(|name| (name.as_str(), false));
		plain.chain(optional).chain(peers)
	}
}

/// What a lockfile makes of the location of an installed package.
fn package_name(location: &str, entry: &Entry) -> Option<String> {
	if let Some(name) = &entry.name {
		return Some(name.clone());
	}
	let (_, name) = location.rsplit_once("node_modules/")?;
	Some(name.to_owned())
}

fn parent_location(location: &str) -> &str {
	match location.rfind("node_modules/") {
		Some(at) => location[..at].trim_end_matches('/'),
		None => "",
	}
}

/// Where a dependency of the package at `from` resolves, by node's rule: the
/// nearest `node_modules` that holds it, walking up.
fn resolve<'a>(
	packages: &'a BTreeMap<String, Entry>,
	from: &str,
	dependency: &str,
) -> Option<&'a str> {
	let mut base = from;
	loop {
		let candidate = if base.is_empty() {
			format!("node_modules/{dependency}")
		} else {
			format!("{base}/node_modules/{dependency}")
		};
		if let Some((key, _)) = packages.get_key_value(candidate.as_str()) {
			return Some(key.as_str());
		}
		if base.is_empty() {
			return None;
		}
		base = parent_location(base);
	}
}

/// Whether an entry is an installed package, not a workspace or a link to one.
fn installed(location: &str, entry: &Entry) -> bool {
	location.contains("node_modules/") && !entry.link && entry.version.is_some()
}

/// The edge to a dependency of the package at `from`, walking into the
/// workspace a link names, since that is the repository's own code, and keeping
/// the dependency's own name in the chain.
fn edge_to<'a>(
	packages: &'a BTreeMap<String, Entry>,
	from: &str,
	name: &str,
	optional: bool,
) -> Option<Edge<&'a str>> {
	let target = resolve(packages, from, name)?;
	let entry = &packages[target];
	let to = if entry.link {
		entry
			.resolved
			.as_deref()
			.and_then(|resolved| packages.get_key_value(resolved))
			.map(|(key, _)| key.as_str())?
	} else {
		target
	};
	Some(Edge {
		to,
		name: name.to_owned(),
		optional,
	})
}

/// Parse a lockfile into the repository's packages. `repository` names the
/// package of a lockfile with no workspaces and no name of its own.
pub fn parse(lockfile: &str, repository: &str) -> Result<Vec<RepositoryPackage>, ParseError> {
	let lockfile: Lockfile = serde_json::from_str(lockfile).map_err(ParseError::new)?;
	if lockfile.version < 2 {
		return Err(ParseError::new(format!(
			"lockfile version {} lists no packages",
			lockfile.version
		)));
	}
	let packages = &lockfile.packages;

	// The root counts as a workspace package when it depends on anything of its
	// own, as a repository that is one package does.
	let workspaces: Vec<(&str, String)> = packages
		.iter()
		.filter(|(location, entry)| {
			if location.is_empty() {
				!(entry.dependencies.is_empty()
					&& entry.dev_dependencies.is_empty()
					&& entry.optional_dependencies.is_empty()
					&& entry.peer_dependencies.is_empty())
			} else {
				!location.contains("node_modules/") && !entry.link
			}
		})
		.map(|(location, entry)| {
			let name = entry.name.clone().unwrap_or_else(|| {
				if location.is_empty() {
					repository.to_owned()
				} else {
					location.rsplit('/').next().unwrap_or(location).to_owned()
				}
			});
			(location.as_str(), name)
		})
		.collect();

	let mut found = Found::default();
	for (location, entry) in packages {
		if installed(location, entry)
			&& let (Some(name), Some(version)) = (package_name(location, entry), &entry.version)
		{
			found.hold(&name, version);
		}
	}

	for (workspace_location, workspace) in &workspaces {
		let Some(entry) = packages.get(*workspace_location) else {
			continue;
		};
		let scoped = |dependencies: &BTreeMap<String, String>, scope, optional| {
			dependencies
				.keys()
				.filter_map(|name| edge_to(packages, workspace_location, name, optional))
				.map(|edge| (edge, scope))
				.collect::<Vec<_>>()
		};
		let mut roots = Vec::new();
		for (name, optional) in entry.brings() {
			roots.extend(
				edge_to(packages, workspace_location, name, optional)
					.map(|edge| (edge, Scope::Runtime)),
			);
		}
		roots.extend(scoped(&entry.dev_dependencies, Scope::Development, false));

		walk(
			roots,
			|location: &&str| {
				let entry = &packages[*location];
				entry
					.brings()
					.filter_map(|(name, optional)| edge_to(packages, location, name, optional))
					.collect()
			},
			|location, scope, optional, chain| {
				let entry = &packages[*location];
				if installed(location, entry)
					&& let (Some(name), Some(version)) =
						(package_name(location, entry), &entry.version)
				{
					found.reach(&name, version, workspace, scope, optional, chain);
				}
			},
		);
	}

	Ok(found.finish(NPM))
}

#[cfg(test)]
mod tests {
	use database::packages::PackagePath;

	use super::*;

	const LOCKFILE: &str = r#"{
		"lockfileVersion": 3,
		"packages": {
			"": { "name": "tamanu", "workspaces": ["packages/*"] },
			"packages/server": {
				"name": "@tamanu/server",
				"version": "1.0.0",
				"dependencies": { "express": "^4", "@tamanu/shared": "*" },
				"devDependencies": { "jest": "^29" },
				"optionalDependencies": { "fsevents": "^2" }
			},
			"packages/web": {
				"name": "@tamanu/web",
				"version": "1.0.0",
				"dependencies": { "react": "^18", "lodash": "^4" }
			},
			"packages/shared": {
				"name": "@tamanu/shared",
				"version": "1.0.0",
				"dependencies": { "lodash": "^4" }
			},
			"node_modules/@tamanu/shared": { "resolved": "packages/shared", "link": true },
			"node_modules/express": {
				"version": "4.18.2",
				"dependencies": { "body-parser": "1.20.1", "lodash": "^3" }
			},
			"node_modules/express/node_modules/lodash": { "version": "3.10.1" },
			"node_modules/body-parser": { "version": "1.20.1" },
			"node_modules/react": { "version": "18.2.0" },
			"node_modules/lodash": { "version": "4.17.21" },
			"node_modules/fsevents": { "version": "2.3.3", "optional": true },
			"node_modules/jest": {
				"version": "29.0.0",
				"dev": true,
				"dependencies": { "chalk": "^4", "lodash": "^4" }
			},
			"node_modules/chalk": { "version": "4.1.2", "dev": true },
			"node_modules/@types/node": { "version": "20.0.0", "dev": true }
		}
	}"#;

	fn find<'a>(
		packages: &'a [RepositoryPackage],
		name: &str,
		version: &str,
	) -> &'a RepositoryPackage {
		packages
			.iter()
			.find(|p| p.name == name && p.version == version)
			.unwrap_or_else(|| panic!("{name} {version} in {packages:#?}"))
	}

	fn path<'a>(
		package: &'a RepositoryPackage,
		workspace: &str,
		scope: Scope,
		optional: bool,
	) -> Option<&'a PackagePath> {
		package.paths.iter().find(|p| {
			p.workspace_package == workspace && p.scope == scope && p.optional == optional
		})
	}

	#[test]
	fn a_package_only_development_reaches_is_in_the_development_scope() {
		let packages = parse(LOCKFILE, "tamanu").unwrap();

		let jest = find(&packages, "jest", "29.0.0");
		assert_eq!(jest.paths.len(), 1);
		let jest_path = path(jest, "@tamanu/server", Scope::Development, false).unwrap();
		assert!(jest_path.direct);
		assert_eq!(jest_path.chain, ["jest"]);

		let chalk = find(&packages, "chalk", "4.1.2");
		let chalk_path = path(chalk, "@tamanu/server", Scope::Development, false).unwrap();
		assert!(!chalk_path.direct);
		assert_eq!(chalk_path.chain, ["jest", "chalk"]);
	}

	#[test]
	fn a_package_nothing_reaches_is_still_held_with_no_paths() {
		let packages = parse(LOCKFILE, "tamanu").unwrap();
		assert!(find(&packages, "@types/node", "20.0.0").paths.is_empty());
		assert!(
			!packages.iter().any(|p| p.name == "@tamanu/shared"),
			"a linked workspace is the repository's own code, not a dependency"
		);
	}

	#[test]
	fn an_optional_dependency_is_an_optional_runtime_path() {
		let packages = parse(LOCKFILE, "tamanu").unwrap();
		let fsevents = find(&packages, "fsevents", "2.3.3");
		assert_eq!(fsevents.paths.len(), 1);
		let optional = path(fsevents, "@tamanu/server", Scope::Runtime, true).unwrap();
		assert!(optional.direct);
	}

	#[test]
	fn a_package_reached_by_two_scopes_carries_each() {
		let packages = parse(LOCKFILE, "tamanu").unwrap();
		let lodash = find(&packages, "lodash", "4.17.21");

		let runtime = path(lodash, "@tamanu/server", Scope::Runtime, false).unwrap();
		assert_eq!(runtime.chain, ["@tamanu/shared", "lodash"]);
		assert!(!runtime.direct);
		let development = path(lodash, "@tamanu/server", Scope::Development, false).unwrap();
		assert_eq!(development.chain, ["jest", "lodash"]);
	}

	#[test]
	fn the_shortest_chain_of_a_scope_is_kept_and_directness_is_per_path() {
		let packages = parse(LOCKFILE, "tamanu").unwrap();
		let lodash = find(&packages, "lodash", "4.17.21");

		let web = path(lodash, "@tamanu/web", Scope::Runtime, false).unwrap();
		assert!(web.direct);
		assert_eq!(web.chain, ["lodash"]);
		let shared = path(lodash, "@tamanu/shared", Scope::Runtime, false).unwrap();
		assert!(shared.direct);

		let workspaces: Vec<&str> = lodash
			.paths
			.iter()
			.map(|p| p.workspace_package.as_str())
			.collect();
		for workspace in ["@tamanu/server", "@tamanu/shared", "@tamanu/web"] {
			assert!(workspaces.contains(&workspace), "{workspaces:?}");
		}
	}

	#[test]
	fn a_nested_copy_is_resolved_from_its_parent_not_the_hoisted_one() {
		let packages = parse(LOCKFILE, "tamanu").unwrap();
		let nested = find(&packages, "lodash", "3.10.1");
		assert_eq!(nested.paths.len(), 1);
		let path = path(nested, "@tamanu/server", Scope::Runtime, false).unwrap();
		assert!(!path.direct);
		assert_eq!(path.chain, ["express", "lodash"]);
	}

	#[test]
	fn a_workspace_reached_through_a_link_passes_its_own_dependencies_on() {
		let packages = parse(LOCKFILE, "tamanu").unwrap();
		let lodash = find(&packages, "lodash", "4.17.21");
		let through = path(lodash, "@tamanu/server", Scope::Runtime, false).unwrap();
		assert_eq!(through.chain, ["@tamanu/shared", "lodash"]);
	}

	#[test]
	fn a_path_is_optional_once_any_step_along_it_is() {
		let lockfile = r#"{
			"lockfileVersion": 3,
			"packages": {
				"": { "name": "app", "dependencies": { "a": "1" } },
				"node_modules/a": { "version": "1.0.0", "optionalDependencies": { "b": "1" } },
				"node_modules/b": { "version": "1.0.0", "dependencies": { "c": "1" } },
				"node_modules/c": { "version": "1.0.0" }
			}
		}"#;
		let packages = parse(lockfile, "repo").unwrap();
		let c = find(&packages, "c", "1.0.0");
		assert_eq!(c.paths.len(), 1);
		let optional = path(c, "app", Scope::Runtime, true).unwrap();
		assert_eq!(optional.chain, ["a", "b", "c"]);
	}

	#[test]
	fn a_repository_that_is_one_package_is_its_own_workspace_package() {
		let lockfile = r#"{
			"lockfileVersion": 3,
			"packages": {
				"": { "name": "solo", "version": "1.0.0", "dependencies": { "a": "1" } },
				"node_modules/a": { "version": "1.0.0" }
			}
		}"#;
		let packages = parse(lockfile, "repo").unwrap();
		assert!(path(find(&packages, "a", "1.0.0"), "solo", Scope::Runtime, false).is_some());

		let unnamed = lockfile.replace(r#""name": "solo", "#, "");
		let packages = parse(&unnamed, "repo").unwrap();
		assert!(path(find(&packages, "a", "1.0.0"), "repo", Scope::Runtime, false).is_some());
	}

	#[test]
	fn a_lockfile_with_no_package_list_is_refused() {
		assert!(parse(r#"{"lockfileVersion": 1, "packages": {}}"#, "repo").is_err());
		assert!(parse("not json", "repo").is_err());
	}
}
