//! Reading Tamanu's dependencies from its `package-lock.json`.
//!
//! Spec: `.workhorse/specs/security/vulnerabilities.md` (id `VUL`), "Sources".
//!
//! Lockfile v3 has one entry per installed package, keyed by its location
//! (`node_modules/a/node_modules/b`), plus one per workspace (`packages/server`).
//! A version's dependencies are every entry the lockfile does not mark
//! development-only. For each, the dependency tree is walked from every
//! workspace to find which workspaces reach it, whether any depends on it
//! directly, and how.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use commons_errors::{AppError, Result};
use database::packages::VersionPackage;
use serde::Deserialize;

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
	dev: bool,
	#[serde(default)]
	link: bool,
	resolved: Option<String>,
	#[serde(default)]
	dependencies: BTreeMap<String, String>,
	#[serde(default, rename = "optionalDependencies")]
	optional_dependencies: BTreeMap<String, String>,
}

/// Where Tamanu's repository is read from.
pub const BASE_URL: &str = "https://raw.githubusercontent.com/beyondessential/tamanu";

/// The URL of a version's lockfile at its release tag.
pub fn url(base: &str, version: &str) -> String {
	format!(
		"{}/v{version}/package-lock.json",
		base.trim_end_matches('/')
	)
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

/// Parse a lockfile into a version's packages.
pub fn parse(lockfile: &str) -> Result<Vec<VersionPackage>> {
	let lockfile: Lockfile = serde_json::from_str(lockfile)
		.map_err(|e| AppError::Custom(format!("package-lock.json: {e}")))?;
	if lockfile.version < 2 {
		return Err(AppError::Custom(format!(
			"package-lock.json: lockfile version {} lists no packages",
			lockfile.version
		)));
	}
	let packages = &lockfile.packages;

	let workspaces: Vec<(&str, String)> = packages
		.iter()
		.filter(|(location, entry)| {
			!location.is_empty() && !location.contains("node_modules/") && !entry.link
		})
		.map(|(location, entry)| {
			let name = entry
				.name
				.clone()
				.unwrap_or_else(|| location.rsplit('/').next().unwrap_or(location).to_owned());
			(location.as_str(), name)
		})
		.collect();

	// Installed packages that production uses, by location.
	let installed = |location: &str, entry: &Entry| {
		location.contains("node_modules/") && !entry.link && !entry.dev && entry.version.is_some()
	};

	let mut found: BTreeMap<(String, String), VersionPackage> = BTreeMap::new();
	for (location, entry) in packages {
		if !installed(location, entry) {
			continue;
		}
		let (Some(name), Some(version)) = (package_name(location, entry), &entry.version) else {
			continue;
		};
		found
			.entry((name.clone(), version.clone()))
			.or_insert_with(|| VersionPackage {
				name,
				version: version.clone(),
				workspaces: Vec::new(),
				direct: false,
				paths: BTreeMap::new(),
			});
	}

	for (workspace_location, workspace) in &workspaces {
		let mut seen: HashMap<&str, Vec<String>> = HashMap::new();
		let mut queue: VecDeque<(&str, Vec<String>)> = VecDeque::new();
		queue.push_back((workspace_location, Vec::new()));
		while let Some((location, chain)) = queue.pop_front() {
			let Some(entry) = packages.get(location) else {
				continue;
			};
			let dependencies = entry
				.dependencies
				.keys()
				.chain(entry.optional_dependencies.keys());
			for dependency in dependencies {
				let Some(target) = resolve(packages, location, dependency) else {
					continue;
				};
				if seen.contains_key(target) {
					continue;
				}
				let mut chain = chain.clone();
				chain.push(dependency.clone());
				seen.insert(target, chain.clone());

				let target_entry = &packages[target];
				// A workspace linked in as a dependency is Tamanu's own code: walk
				// into it, and keep the chain, without it being a package.
				let next = if target_entry.link {
					target_entry
						.resolved
						.as_deref()
						.filter(|r| packages.contains_key(*r))
				} else {
					Some(target)
				};
				if let (Some(name), Some(version)) =
					(package_name(target, target_entry), &target_entry.version)
					&& installed(target, target_entry)
					&& let Some(package) = found.get_mut(&(name, version.clone()))
				{
					if !package.workspaces.contains(workspace) {
						package.workspaces.push(workspace.clone());
					}
					if chain.len() == 1 {
						package.direct = true;
					}
					let shortest = package
						.paths
						.get(workspace)
						.is_none_or(|held| held.len() > chain.len());
					if shortest {
						package.paths.insert(workspace.clone(), chain.clone());
					}
				}
				if let Some(next) = next {
					queue.push_back((next, chain));
				}
			}
		}
	}

	let mut out: Vec<VersionPackage> = found.into_values().collect();
	for package in &mut out {
		package.workspaces.sort();
		let unique: BTreeSet<_> = package.workspaces.drain(..).collect();
		package.workspaces = unique.into_iter().collect();
	}
	Ok(out)
}

#[cfg(test)]
mod tests {
	use super::*;

	const LOCKFILE: &str = r#"{
		"lockfileVersion": 3,
		"packages": {
			"": { "name": "tamanu", "workspaces": ["packages/*"] },
			"packages/server": {
				"name": "@tamanu/server",
				"version": "1.0.0",
				"dependencies": { "express": "^4", "@tamanu/shared": "*" },
				"devDependencies": { "jest": "^29" }
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
			"node_modules/jest": { "version": "29.0.0", "dev": true },
			"node_modules/@types/node": { "version": "20.0.0", "dev": true }
		}
	}"#;

	fn find<'a>(packages: &'a [VersionPackage], name: &str, version: &str) -> &'a VersionPackage {
		packages
			.iter()
			.find(|p| p.name == name && p.version == version)
			.unwrap_or_else(|| panic!("{name} {version} in {packages:#?}"))
	}

	#[test]
	fn development_only_packages_are_left_out() {
		let packages = parse(LOCKFILE).unwrap();
		assert!(!packages.iter().any(|p| p.name == "jest"));
		assert!(!packages.iter().any(|p| p.name == "@types/node"));
		assert!(
			!packages.iter().any(|p| p.name == "@tamanu/shared"),
			"a linked workspace is Tamanu's own code, not a dependency"
		);
	}

	#[test]
	fn a_package_records_the_workspaces_that_reach_it_and_whether_directly() {
		let packages = parse(LOCKFILE).unwrap();

		let react = find(&packages, "react", "18.2.0");
		assert_eq!(react.workspaces, ["@tamanu/web"]);
		assert!(react.direct);

		let body_parser = find(&packages, "body-parser", "1.20.1");
		assert_eq!(body_parser.workspaces, ["@tamanu/server"]);
		assert!(!body_parser.direct);
		assert_eq!(
			body_parser.paths["@tamanu/server"],
			["express", "body-parser"]
		);

		let lodash = find(&packages, "lodash", "4.17.21");
		assert_eq!(
			lodash.workspaces,
			["@tamanu/server", "@tamanu/shared", "@tamanu/web"]
		);
		assert!(lodash.direct);
	}

	#[test]
	fn a_nested_copy_is_resolved_from_its_parent_not_the_hoisted_one() {
		let packages = parse(LOCKFILE).unwrap();
		let nested = find(&packages, "lodash", "3.10.1");
		assert_eq!(nested.workspaces, ["@tamanu/server"]);
		assert!(!nested.direct);
		assert_eq!(nested.paths["@tamanu/server"], ["express", "lodash"]);
	}

	#[test]
	fn a_workspace_reached_through_a_link_passes_its_own_dependencies_on() {
		let packages = parse(LOCKFILE).unwrap();
		let lodash = find(&packages, "lodash", "4.17.21");
		assert!(lodash.workspaces.contains(&"@tamanu/server".to_owned()));
		assert!(lodash.paths.contains_key("@tamanu/server"));
		assert_eq!(lodash.paths["@tamanu/server"], ["@tamanu/shared", "lodash"]);
	}

	#[test]
	fn a_lockfile_with_no_package_list_is_refused() {
		assert!(parse(r#"{"lockfileVersion": 1, "packages": {}}"#).is_err());
		assert!(parse("not json").is_err());
	}

	#[test]
	fn the_lockfile_is_read_at_the_release_tag() {
		assert_eq!(
			url(BASE_URL, "2.3.4"),
			"https://raw.githubusercontent.com/beyondessential/tamanu/v2.3.4/package-lock.json"
		);
	}
}
