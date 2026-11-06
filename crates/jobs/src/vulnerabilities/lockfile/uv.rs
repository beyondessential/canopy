//! Reading a repository's Python dependencies from a `uv.lock`.
//!
//! Spec: `.workhorse/specs/security/vulnerabilities.md` (id `VUL`), "Where a
//! package sits".
//!
//! The workspace members are the packages with an editable or virtual source,
//! and those the lockfile lists as members. A member's `dependencies` are
//! runtime, its `optional-dependencies` (extras) are runtime and optional, and
//! its `dev-dependencies` (dependency groups) are development. Packages further
//! down inherit the scope of the edge that leads to them, and take on the
//! dependencies of an extra a dependency asks for. Names are normalised as PEP
//! 503 has them. Only packages from a registry are ones OSV can be asked
//! about; the rest are walked through.

use std::collections::{BTreeMap, HashMap};

use database::packages::{PYPI, RepositoryPackage, Scope};
use serde::Deserialize;

use super::{Edge, Found, ParseError, walk};

#[derive(Debug, Deserialize)]
struct Lockfile {
	#[serde(default, rename = "package")]
	packages: Vec<Package>,
	#[serde(default)]
	manifest: Manifest,
}

#[derive(Debug, Default, Deserialize)]
struct Manifest {
	#[serde(default)]
	members: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Package {
	name: String,
	version: Option<String>,
	source: Option<toml::Table>,
	#[serde(default)]
	dependencies: Vec<Dependency>,
	#[serde(default, rename = "optional-dependencies")]
	optional_dependencies: BTreeMap<String, Vec<Dependency>>,
	#[serde(default, rename = "dev-dependencies")]
	dev_dependencies: BTreeMap<String, Vec<Dependency>>,
}

#[derive(Debug, Deserialize)]
struct Dependency {
	name: String,
	version: Option<String>,
	#[serde(default)]
	extra: Vec<String>,
}

impl Package {
	fn from_registry(&self) -> bool {
		self.source
			.as_ref()
			.is_some_and(|source| source.contains_key("registry"))
	}

	fn is_workspace_source(&self) -> bool {
		self.source
			.as_ref()
			.is_some_and(|source| source.contains_key("editable") || source.contains_key("virtual"))
	}
}

/// A package name as PEP 503 normalises it: lowercase, with each run of `-`,
/// `_` and `.` one `-`.
pub fn normalise(name: &str) -> String {
	let mut out = String::with_capacity(name.len());
	let mut separated = false;
	for c in name.chars() {
		if matches!(c, '-' | '_' | '.') {
			separated = true;
		} else {
			if separated && !out.is_empty() {
				out.push('-');
			}
			separated = false;
			out.push(c.to_ascii_lowercase());
		}
	}
	out
}

/// A node of the walk: a package, and the extras it was asked for.
type Node = (usize, Vec<String>);

/// Parse a lockfile into the repository's packages.
pub fn parse(lockfile: &str) -> Result<Vec<RepositoryPackage>, ParseError> {
	let lockfile: Lockfile = toml::from_str(lockfile).map_err(ParseError::new)?;
	let packages = &lockfile.packages;

	let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
	for (index, package) in packages.iter().enumerate() {
		by_name
			.entry(normalise(&package.name))
			.or_default()
			.push(index);
	}
	let locate = |dependency: &Dependency| -> Option<usize> {
		let candidates = by_name.get(&normalise(&dependency.name))?;
		match (candidates.as_slice(), &dependency.version) {
			([only], _) => Some(*only),
			(several, Some(version)) => several
				.iter()
				.copied()
				.find(|index| packages[*index].version.as_ref() == Some(version)),
			(several, None) => several.first().copied(),
		}
	};
	let edge = |dependency: &Dependency, optional: bool| -> Option<Edge<Node>> {
		Some(Edge {
			to: (locate(dependency)?, dependency.extra.clone()),
			name: normalise(&dependency.name),
			optional,
		})
	};

	let members: Vec<&Package> = packages
		.iter()
		.filter(|package| {
			package.is_workspace_source()
				|| lockfile
					.manifest
					.members
					.iter()
					.any(|member| normalise(member) == normalise(&package.name))
		})
		.collect();

	let mut found = Found::default();
	for package in packages {
		if let (true, Some(version)) = (package.from_registry(), &package.version) {
			found.hold(&normalise(&package.name), version);
		}
	}

	for member in members {
		let workspace = normalise(&member.name);
		let mut roots: Vec<(Edge<Node>, Scope)> = Vec::new();
		roots.extend(
			member
				.dependencies
				.iter()
				.filter_map(|dependency| edge(dependency, false))
				.map(|edge| (edge, Scope::Runtime)),
		);
		roots.extend(
			member
				.optional_dependencies
				.values()
				.flatten()
				.filter_map(|dependency| edge(dependency, true))
				.map(|edge| (edge, Scope::Runtime)),
		);
		roots.extend(
			member
				.dev_dependencies
				.values()
				.flatten()
				.filter_map(|dependency| edge(dependency, false))
				.map(|edge| (edge, Scope::Development)),
		);

		walk(
			roots,
			|(index, extras): &Node| {
				let package = &packages[*index];
				let asked = extras
					.iter()
					.filter_map(|extra| package.optional_dependencies.get(extra))
					.flatten();
				package
					.dependencies
					.iter()
					.chain(asked)
					.filter_map(|dependency| edge(dependency, false))
					.collect()
			},
			|(index, _), scope, optional, chain| {
				let package = &packages[*index];
				if let (true, Some(version)) = (package.from_registry(), &package.version) {
					found.reach(
						&normalise(&package.name),
						version,
						&workspace,
						scope,
						optional,
						chain,
					);
				}
			},
		);
	}

	Ok(found.finish(PYPI))
}

#[cfg(test)]
mod tests {
	use database::packages::PackagePath;

	use super::*;

	const LOCKFILE: &str = r#"
version = 1
requires-python = ">=3.12"

[manifest]
members = ["app", "worker"]

[[package]]
name = "app"
version = "0.1.0"
source = { editable = "." }
dependencies = [
    { name = "requests" },
    { name = "Some_Thing" },
]

[package.optional-dependencies]
speed = [
    { name = "ujson" },
]

[package.dev-dependencies]
dev = [
    { name = "pytest" },
]

[[package]]
name = "worker"
version = "0.1.0"
source = { virtual = "worker" }
dependencies = [
    { name = "requests", extra = ["socks"] },
]

[[package]]
name = "requests"
version = "2.31.0"
source = { registry = "https://pypi.org/simple" }
dependencies = [
    { name = "urllib3" },
]

[package.optional-dependencies]
socks = [
    { name = "pysocks" },
]

[[package]]
name = "urllib3"
version = "2.0.0"
source = { registry = "https://pypi.org/simple" }

[[package]]
name = "pysocks"
version = "1.7.1"
source = { registry = "https://pypi.org/simple" }

[[package]]
name = "some-thing"
version = "1.0.0"
source = { registry = "https://pypi.org/simple" }

[[package]]
name = "ujson"
version = "5.0.0"
source = { registry = "https://pypi.org/simple" }

[[package]]
name = "pytest"
version = "8.0.0"
source = { registry = "https://pypi.org/simple" }
dependencies = [
    { name = "urllib3" },
]
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
		optional: bool,
	) -> Option<&'a PackagePath> {
		package.paths.iter().find(|p| {
			p.workspace_package == workspace && p.scope == scope && p.optional == optional
		})
	}

	#[test]
	fn members_are_found_and_names_are_normalised() {
		let packages = parse(LOCKFILE).unwrap();
		assert!(!packages.iter().any(|p| p.name == "app" || p.name == "worker"));
		let thing = find(&packages, "some-thing");
		assert!(path(thing, "app", Scope::Runtime, false).unwrap().direct);
		assert_eq!(thing.ecosystem, "PyPI");
		assert!(path(find(&packages, "requests"), "worker", Scope::Runtime, false).is_some());
	}

	#[test]
	fn extras_are_optional_and_groups_are_development() {
		let packages = parse(LOCKFILE).unwrap();
		let ujson = find(&packages, "ujson");
		assert_eq!(ujson.paths.len(), 1);
		assert!(path(ujson, "app", Scope::Runtime, true).unwrap().direct);

		let pytest = find(&packages, "pytest");
		assert_eq!(pytest.paths.len(), 1);
		assert!(path(pytest, "app", Scope::Development, false).unwrap().direct);
	}

	#[test]
	fn a_package_reached_by_two_scopes_carries_each_with_its_shortest_chain() {
		let packages = parse(LOCKFILE).unwrap();
		let urllib3 = find(&packages, "urllib3");
		let runtime = path(urllib3, "app", Scope::Runtime, false).unwrap();
		assert_eq!(runtime.chain, ["requests", "urllib3"]);
		assert!(!runtime.direct);
		let development = path(urllib3, "app", Scope::Development, false).unwrap();
		assert_eq!(development.chain, ["pytest", "urllib3"]);
	}

	#[test]
	fn a_dependency_asking_for_an_extra_takes_on_its_dependencies() {
		let packages = parse(LOCKFILE).unwrap();
		let pysocks = find(&packages, "pysocks");
		assert_eq!(pysocks.paths.len(), 1);
		let through = path(pysocks, "worker", Scope::Runtime, false).unwrap();
		assert_eq!(through.chain, ["requests", "pysocks"]);
	}

	#[test]
	fn names_are_normalised_as_pep_503_has_them() {
		assert_eq!(normalise("Some_Thing"), "some-thing");
		assert_eq!(normalise("a.b__c--d"), "a-b-c-d");
		assert_eq!(normalise("zope.interface"), "zope-interface");
	}

	#[test]
	fn a_lockfile_that_does_not_parse_is_refused() {
		assert!(parse("not toml [").is_err());
	}
}
