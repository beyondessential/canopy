//! Reading a repository's Cargo dependencies from a `Cargo.lock` and the
//! manifests of its workspace members.
//!
//! Spec: `.workhorse/specs/security/vulnerabilities.md` (id `VUL`), "Where a
//! package sits".
//!
//! A lockfile does not say what kind of dependency an edge is, so each member's
//! `Cargo.toml` does: `[dependencies]` is runtime, `[build-dependencies]` is
//! build and `[dev-dependencies]` is development, in the tables of
//! `[target.*]` as well. Packages further down inherit the scope of the edge
//! that leads to them. The workspace members are the packages with no source,
//! that a manifest was read for. Packages from git or a path, and from
//! registries other than crates.io, are walked through but not themselves
//! packages OSV could be asked about.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cargo_lock::{Dependency, Lockfile, Package};
use database::packages::{CARGO, RepositoryPackage, Scope};

use super::{Edge, Found, ParseError, walk};

/// What a manifest says of its package and the dependencies it declares.
struct Manifest {
	name: String,
	/// Each declared dependency, by the name of the package it is, with the
	/// scopes it is declared in.
	dependencies: BTreeMap<String, BTreeSet<Scope>>,
}

/// Names are compared as crates.io does, where `-` and `_` are the same.
fn normalised(name: &str) -> String {
	name.to_ascii_lowercase().replace('_', "-")
}

fn read_manifest(text: &str) -> Result<Option<Manifest>, ParseError> {
	let table: toml::Table = text.parse().map_err(ParseError::new)?;
	let Some(name) = table
		.get("package")
		.and_then(|package| package.get("name"))
		.and_then(toml::Value::as_str)
	else {
		return Ok(None);
	};

	let mut dependencies: BTreeMap<String, BTreeSet<Scope>> = BTreeMap::new();
	let mut declare = |tables: &toml::Table| {
		for (key, scope) in [
			("dependencies", Scope::Runtime),
			("build-dependencies", Scope::Build),
			("build_dependencies", Scope::Build),
			("dev-dependencies", Scope::Development),
			("dev_dependencies", Scope::Development),
		] {
			let Some(declared) = tables.get(key).and_then(toml::Value::as_table) else {
				continue;
			};
			for (dependency, value) in declared {
				let package = value
					.get("package")
					.and_then(toml::Value::as_str)
					.unwrap_or(dependency);
				dependencies
					.entry(normalised(package))
					.or_default()
					.insert(scope);
			}
		}
	};
	declare(&table);
	if let Some(targets) = table.get("target").and_then(toml::Value::as_table) {
		for target in targets.values().filter_map(toml::Value::as_table) {
			declare(target);
		}
	}
	Ok(Some(Manifest {
		name: name.to_owned(),
		dependencies,
	}))
}

/// The members a workspace manifest names, as globs relative to its directory,
/// and those it leaves out. Empty for a manifest that is not a workspace root.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Workspace {
	pub members: Vec<String>,
	pub exclude: Vec<String>,
}

pub fn workspace(manifest: &str) -> Result<Workspace, ParseError> {
	let table: toml::Table = manifest.parse().map_err(ParseError::new)?;
	let Some(workspace) = table.get("workspace").and_then(toml::Value::as_table) else {
		return Ok(Workspace::default());
	};
	let list = |key: &str| -> Vec<String> {
		workspace
			.get(key)
			.and_then(toml::Value::as_array)
			.into_iter()
			.flatten()
			.filter_map(toml::Value::as_str)
			.map(|path| {
				path.trim_start_matches("./")
					.trim_end_matches('/')
					.to_owned()
			})
			.collect()
	};
	Ok(Workspace {
		members: list("members"),
		exclude: list("exclude"),
	})
}

fn is_crates_io(package: &Package) -> bool {
	package
		.source
		.as_ref()
		.is_some_and(|source| source.is_default_registry())
}

/// Parse a lockfile, with the text of the `Cargo.toml` of each workspace member
/// (and of the root, where it is a package), into the repository's packages.
pub fn parse(lockfile: &str, manifests: &[&str]) -> Result<Vec<RepositoryPackage>, ParseError> {
	let lockfile: Lockfile = lockfile.parse().map_err(ParseError::new)?;
	let packages = &lockfile.packages;

	let mut members: HashMap<String, Manifest> = HashMap::new();
	for text in manifests {
		if let Some(manifest) = read_manifest(text)? {
			members.insert(manifest.name.clone(), manifest);
		}
	}

	let mut by_name: HashMap<(&str, String), Vec<usize>> = HashMap::new();
	for (index, package) in packages.iter().enumerate() {
		by_name
			.entry((package.name.as_str(), package.version.to_string()))
			.or_default()
			.push(index);
	}
	let locate = |dependency: &Dependency| -> Option<usize> {
		let candidates = by_name.get(&(dependency.name.as_str(), dependency.version.to_string()))?;
		match candidates.as_slice() {
			[only] => Some(*only),
			several => several
				.iter()
				.copied()
				.find(|index| packages[*index].source == dependency.source)
				.or_else(|| several.first().copied()),
		}
	};
	let edges = |package: &Package| -> Vec<Edge<usize>> {
		package
			.dependencies
			.iter()
			.filter_map(|dependency| {
				Some(Edge {
					to: locate(dependency)?,
					name: dependency.name.as_str().to_owned(),
					optional: false,
				})
			})
			.collect()
	};

	let mut found = Found::default();
	for package in packages {
		if is_crates_io(package) {
			found.hold(package.name.as_str(), &package.version.to_string());
		}
	}

	let mut walked = 0;
	for member in packages.iter().filter(|package| package.source.is_none()) {
		let Some(manifest) = members.get(member.name.as_str()) else {
			continue;
		};
		walked += 1;

		let mut roots = Vec::new();
		for dependency in &member.dependencies {
			let Some(to) = locate(dependency) else {
				continue;
			};
			let declared = manifest.dependencies.get(&normalised(dependency.name.as_str()));
			let scopes: Vec<Scope> = match declared {
				Some(scopes) => scopes.iter().copied().collect(),
				None => vec![Scope::Runtime],
			};
			for scope in scopes {
				roots.push((
					Edge {
						to,
						name: dependency.name.as_str().to_owned(),
						optional: false,
					},
					scope,
				));
			}
		}

		walk(
			roots,
			|index: &usize| edges(&packages[*index]),
			|index, scope, optional, chain| {
				let package = &packages[*index];
				if is_crates_io(package) {
					found.reach(
						package.name.as_str(),
						&package.version.to_string(),
						&member.name,
						scope,
						optional,
						chain,
					);
				}
			},
		);
	}

	if walked == 0 && packages.iter().any(is_crates_io) {
		return Err(ParseError::new(
			"none of the packages with no source has a Cargo.toml that was read",
		));
	}
	Ok(found.finish(CARGO))
}

#[cfg(test)]
mod tests {
	use database::packages::PackagePath;

	use super::*;

	const REGISTRY: &str = "registry+https://github.com/rust-lang/crates.io-index";

	fn lockfile() -> String {
		format!(
			r#"
version = 4

[[package]]
name = "app"
version = "0.1.0"
dependencies = ["core", "serde", "cc", "tempfile", "winapi", "tool"]

[[package]]
name = "core"
version = "0.1.0"
dependencies = ["itoa"]

[[package]]
name = "tool"
version = "0.1.0"
source = "git+https://example.com/tool#abc"
dependencies = ["itoa"]

[[package]]
name = "serde"
version = "1.0.200"
source = "{REGISTRY}"
dependencies = ["itoa"]

[[package]]
name = "itoa"
version = "1.0.0"
source = "{REGISTRY}"

[[package]]
name = "cc"
version = "1.0.0"
source = "{REGISTRY}"
dependencies = ["shlex"]

[[package]]
name = "shlex"
version = "1.0.0"
source = "{REGISTRY}"

[[package]]
name = "tempfile"
version = "3.0.0"
source = "{REGISTRY}"

[[package]]
name = "winapi"
version = "0.3.9"
source = "{REGISTRY}"
"#
		)
	}

	const APP: &str = r#"
[package]
name = "app"
version = "0.1.0"

[dependencies]
serde = "1"
core = { path = "../core" }
tool = { git = "https://example.com/tool" }

[build-dependencies]
cc = "1"

[dev-dependencies]
tempfile = "3"

[target.'cfg(windows)'.dependencies]
winapi = "0.3"
"#;

	const CORE: &str = r#"
[package]
name = "core"
version = "0.1.0"

[dependencies]
itoa = "1"
"#;

	fn find<'a>(packages: &'a [RepositoryPackage], name: &str) -> &'a RepositoryPackage {
		packages
			.iter()
			.find(|p| p.name == name)
			.unwrap_or_else(|| panic!("{name} in {packages:#?}"))
	}

	fn path<'a>(
		package: &'a RepositoryPackage,
		workspace: &str,
		scope: Scope,
	) -> Option<&'a PackagePath> {
		package
			.paths
			.iter()
			.find(|p| p.workspace_package == workspace && p.scope == scope)
	}

	#[test]
	fn dependency_kinds_become_scopes() {
		let packages = parse(&lockfile(), &[APP, CORE]).unwrap();
		assert!(path(find(&packages, "serde"), "app", Scope::Runtime).unwrap().direct);
		assert!(path(find(&packages, "cc"), "app", Scope::Build).unwrap().direct);
		assert!(path(find(&packages, "tempfile"), "app", Scope::Development).unwrap().direct);
		assert_eq!(find(&packages, "cc").paths.len(), 1);
	}

	#[test]
	fn target_specific_tables_count() {
		let packages = parse(&lockfile(), &[APP, CORE]).unwrap();
		assert!(path(find(&packages, "winapi"), "app", Scope::Runtime).is_some());
	}

	#[test]
	fn a_package_further_down_inherits_the_scope_of_the_edge_to_it() {
		let packages = parse(&lockfile(), &[APP, CORE]).unwrap();
		let shlex = find(&packages, "shlex");
		let inherited = path(shlex, "app", Scope::Build).unwrap();
		assert!(!inherited.direct);
		assert_eq!(inherited.chain, ["cc", "shlex"]);
		assert_eq!(shlex.paths.len(), 1);
	}

	#[test]
	fn the_shortest_chain_is_kept_and_git_and_path_packages_are_walked_through() {
		let packages = parse(&lockfile(), &[APP, CORE]).unwrap();
		let itoa = find(&packages, "itoa");
		assert_eq!(itoa.paths.len(), 2, "one for each member");

		let from_app = path(itoa, "app", Scope::Runtime).unwrap();
		assert_eq!(from_app.chain.len(), 2);
		assert!(from_app.chain == ["serde", "itoa"] || from_app.chain == ["core", "itoa"] || from_app.chain == ["tool", "itoa"]);
		assert!(path(itoa, "core", Scope::Runtime).unwrap().direct);

		assert!(
			!packages.iter().any(|p| p.name == "tool" || p.name == "core"),
			"only crates.io packages are held"
		);
	}

	#[test]
	fn a_workspace_manifest_names_its_members_and_exclusions() {
		let manifest = "[workspace]\nmembers = [\"crates/*\", \"./tools/x/\"]\nexclude = [\"crates/old\"]\n";
		assert_eq!(
			workspace(manifest).unwrap(),
			Workspace {
				members: vec!["crates/*".into(), "tools/x".into()],
				exclude: vec!["crates/old".into()],
			}
		);
		assert_eq!(workspace(APP).unwrap(), Workspace::default());
	}

	#[test]
	fn a_lockfile_with_no_member_manifest_is_refused() {
		assert!(parse(&lockfile(), &[]).is_err());
		assert!(parse("not toml [", &[APP]).is_err());
		assert!(parse(&lockfile(), &["not toml ["]).is_err());
	}
}
