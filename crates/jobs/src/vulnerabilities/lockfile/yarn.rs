//! Reading a repository's npm dependencies from a Yarn `yarn.lock`.
//!
//! Spec: `.workhorse/specs/security/vulnerabilities.md` (id `VUL`), "Where a
//! package sits".
//!
//! Only Yarn 2 and later's lockfile is read. It has one entry per resolved
//! package, keyed by the descriptors (`name@npm:^4.1.0`) that resolve to it, and
//! one per workspace (`name@workspace:packages/server`), so the workspaces and
//! how every dependency resolves are all in the lockfile. What it does not say
//! is why a workspace depends on something: its entry lists development and
//! runtime dependencies together. Each workspace's `package.json` supplies that,
//! and so is needed alongside: its `dependencies` give runtime paths, its
//! `optionalDependencies` optional runtime paths and its `devDependencies`
//! development paths, and a path keeps the scope it started with all the way
//! down. A workspace another reaches passes on only its runtime dependencies. A
//! package's own entry lists its dependencies, marking the optional ones in
//! `dependenciesMeta`. Peer dependencies are provided by whatever depends on the
//! package, which lists them itself. A package a workspace depends on under
//! another name, as `"react-router-dom-v6": "npm:react-router-dom@6.3.0"` does,
//! is held under the name it really has, since a resolution names the package
//! itself however the descriptor that reaches it is written.

use std::{
	borrow::Cow,
	cell::RefCell,
	collections::{BTreeMap, BTreeSet, HashMap},
	fmt::Display,
	rc::Rc,
};

use database::packages::{NPM, RepositoryPackage, Scope};
use serde::Deserialize;

use super::{Edge, Found, ParseError, walk};

/// What Yarn names the workspace at the root of a repository that gives it no
/// name of its own.
const UNNAMED_ROOT: &str = "root-workspace-0b6124";

#[derive(Debug, Deserialize)]
struct Raw {
	version: Option<String>,
	resolution: Option<String>,
	#[serde(default)]
	dependencies: BTreeMap<String, String>,
	#[serde(default, rename = "dependenciesMeta")]
	dependencies_meta: BTreeMap<String, DependencyMeta>,
}

#[derive(Debug, Default, Deserialize)]
struct DependencyMeta {
	#[serde(default)]
	optional: bool,
}

#[derive(Debug)]
struct Entry {
	/// The package's name.
	name: String,
	/// How the package is obtained: `npm`, `patch`, `file`, `workspace`...
	protocol: String,
	/// What follows the protocol in the package's resolution: for a workspace,
	/// where it is.
	location: String,
	/// The lockfile's own identification of the package.
	resolution: String,
	version: Option<String>,
	dependencies: BTreeMap<String, String>,
	optional: BTreeSet<String>,
}

impl Entry {
	fn workspace(&self) -> bool {
		self.protocol == "workspace"
	}

	/// Whether the package is one OSV can be asked about: not the repository's
	/// own code, nor a link to a directory.
	fn installed(&self) -> bool {
		!matches!(self.protocol.as_str(), "workspace" | "link" | "portal") && self.version.is_some()
	}
}

/// A package's name, from the front of a descriptor or a resolution: up to the
/// `@` that starts the version or protocol, after any leading one of a scope.
fn split_ident(ident: &str) -> Option<(&str, &str)> {
	let search = ident.strip_prefix('@').map_or(0, |_| 1);
	let at = ident[search..].find('@')? + search;
	Some((&ident[..at], &ident[at + 1..]))
}

/// A descriptor or resolution with any credentials in it taken out. What goes
/// wrong with a lockfile is logged and kept against the repository, and one
/// that names a host can carry a token committed with it.
fn without_credentials(text: &str) -> Cow<'_, str> {
	let Some(scheme) = text.find("://") else {
		return Cow::Borrowed(text);
	};
	let host = scheme + 3;
	let host_ends = text[host..].find('/').map_or(text.len(), |at| host + at);
	match text[host..host_ends].rfind('@') {
		None => Cow::Borrowed(text),
		Some(at) => Cow::Owned(format!("{}***{}", &text[..host], &text[host + at..])),
	}
}

fn percent_decode(text: &str) -> String {
	let bytes = text.as_bytes();
	let mut out = Vec::with_capacity(bytes.len());
	let mut at = 0;
	while at < bytes.len() {
		let decoded = (bytes[at] == b'%')
			.then(|| text.get(at + 1..at + 3))
			.flatten()
			.and_then(|hex| u8::from_str_radix(hex, 16).ok());
		match decoded {
			Some(byte) => {
				out.push(byte);
				at += 3;
			}
			None => {
				out.push(bytes[at]);
				at += 1;
			}
		}
	}
	String::from_utf8_lossy(&out).into_owned()
}

/// A workspace's `package.json`, as far as which of its dependencies are of
/// which kind.
#[derive(Debug, Default, Deserialize)]
struct Manifest {
	#[serde(default)]
	dependencies: BTreeMap<String, serde::de::IgnoredAny>,
	#[serde(default, rename = "devDependencies")]
	dev_dependencies: BTreeMap<String, serde::de::IgnoredAny>,
	#[serde(default, rename = "optionalDependencies")]
	optional_dependencies: BTreeMap<String, serde::de::IgnoredAny>,
	#[serde(default)]
	resolutions: BTreeMap<String, String>,
}

/// A `resolutions` entry of the root `package.json`, which makes a dependency
/// resolve to something other than what the package that has it asked for. The
/// lockfile keys the result by the new descriptor, while the dependent's own
/// entry still names the one it asked for.
#[derive(Debug)]
struct Override {
	/// The package the dependency must be of, where the pattern names one: the
	/// `webpack` of `webpack/memory-fs`, and the `@babel/core` of
	/// `@babel/core@npm:7.0.0/@babel/generator`, since a parent may be written
	/// as a descriptor. A pattern that names no parent is any package.
	parent: Option<String>,
	name: String,
	/// The range the dependency must ask for, where the entry names one.
	range: Option<String>,
	/// The descriptor it resolves to instead.
	descriptor: String,
}

impl Override {
	fn new(pattern: &str, target: &str) -> Option<Self> {
		let (path, range) = match pattern.rfind('@') {
			Some(at) if at > 0 && !pattern[..at].ends_with('/') => {
				(&pattern[..at], Some(pattern[at + 1..].to_owned()))
			}
			_ => (pattern, None),
		};
		let segments: Vec<&str> = path.split('/').collect();
		let name_from = match segments.len() {
			0 | 1 => 0,
			n if segments[n - 2].starts_with('@') => n - 2,
			n => n - 1,
		};
		let name = segments[name_from..].join("/");
		// Yarn's patterns take one parent, so what is left is it. A `**`, which
		// Yarn does not take, is dropped rather than read as part of a name.
		let parent = segments[..name_from]
			.iter()
			.filter(|segment| **segment != "**")
			.copied()
			.collect::<Vec<&str>>()
			.join("/");
		// A parent may carry a descriptor, as the `@npm:7.0.0` of
		// `@babel/core@npm:7.0.0/@babel/generator` does; the package it names is
		// what a dependent is matched against.
		let parent = split_ident(&parent).map_or(parent.clone(), |(name, _)| name.to_owned());
		Some(Self {
			parent: (!parent.is_empty()).then_some(parent),
			name,
			range,
			descriptor: if target.contains(':') {
				target.to_owned()
			} else {
				format!("npm:{target}")
			},
		})
	}

	fn applies(&self, from: &Entry, name: &str, descriptor: &str) -> bool {
		self.name == name
			&& self
				.range
				.as_ref()
				.is_none_or(|range| range == descriptor || format!("npm:{range}") == descriptor)
			&& self
				.parent
				.as_ref()
				.is_none_or(|parent| *parent == from.name)
	}
}

/// Finds what dependencies resolve to, keeping track of those it cannot.
struct Resolver<'a> {
	lockfile: &'a Lockfile,
	overrides: Vec<Override>,
	/// The manifest of each workspace entry, by its index.
	kinds: BTreeMap<usize, Manifest>,
	unresolved: RefCell<BTreeSet<String>>,
	/// What each entry's dependencies resolve to, worked out the first time
	/// a walk arrives at it. Which entry a dependency resolves to is the same
	/// whichever workspace's walk asks, so this is done once for the lockfile
	/// rather than once for each of its workspaces.
	resolved: RefCell<HashMap<usize, Rc<[(usize, String, bool)]>>>,
}

impl Resolver<'_> {
	/// Where the dependencies of the entry at `index` lead, as the entry it
	/// arrives at, the name the step is taken by, and whether it is optional.
	/// A workspace reached as a dependency passes on only its runtime
	/// dependencies, whoever reached it.
	fn edges_of(&self, index: usize) -> Rc<[(usize, String, bool)]> {
		if let Some(resolved) = self.resolved.borrow().get(&index) {
			return resolved.clone();
		}
		let entry = &self.lockfile.entries[index];
		let resolved: Rc<[(usize, String, bool)]> = entry
			.dependencies
			.iter()
			.filter_map(|(dependency, descriptor)| {
				let optional = match self.kinds.get(&index) {
					Some(manifest) => match manifest.kind(dependency) {
						(Scope::Development, _) => return None,
						(_, optional) => optional,
					},
					None => entry.optional.contains(dependency),
				};
				let edge = self.edge(entry, dependency, descriptor, optional)?;
				Some((edge.to, edge.name, edge.optional))
			})
			.collect();
		self.resolved.borrow_mut().insert(index, resolved.clone());
		resolved
	}

	fn edge(
		&self,
		from: &Entry,
		name: &str,
		descriptor: &str,
		optional: bool,
	) -> Option<Edge<usize>> {
		let lockfile = self.lockfile;
		// An override is asked first, since the descriptor the dependent asks
		// for may still be in the lockfile for some other package that was not
		// overridden, and resolving to that would attribute the wrong version
		// here. The one that names a parent is the more particular, so it wins.
		let overridden = self
			.overrides
			.iter()
			.filter(|rule| rule.applies(from, name, descriptor))
			.max_by_key(|rule| rule.parent.is_some())
			.and_then(|rule| lockfile.locate(from, name, &rule.descriptor));
		let located = overridden.or_else(|| lockfile.locate(from, name, descriptor));
		let Some(to) = located else {
			self.unresolved.borrow_mut().insert(format!(
				"{name}@{} of {}",
				without_credentials(descriptor),
				from.name
			));
			return None;
		};
		Some(Edge {
			to,
			name: name.to_owned(),
			optional,
		})
	}
}

impl Manifest {
	/// The scope a workspace depends on a package by, and whether optionally.
	/// `optionalDependencies` wins over `dependencies` where both name a
	/// package, as npm and Yarn both have it. A dependency the manifest does not
	/// say anything about is taken as a plain runtime one.
	fn kind(&self, name: &str) -> (Scope, bool) {
		if self.optional_dependencies.contains_key(name) {
			(Scope::Runtime, true)
		} else if self.dependencies.contains_key(name) {
			(Scope::Runtime, false)
		} else if self.dev_dependencies.contains_key(name) {
			(Scope::Development, false)
		} else {
			(Scope::Runtime, false)
		}
	}
}

/// A Yarn lockfile that has been read, but not yet resolved into paths, which
/// needs the manifests of its workspaces.
#[derive(Debug)]
pub struct Lockfile {
	entries: Vec<Entry>,
	by_descriptor: BTreeMap<String, usize>,
}

fn unreadable(why: impl Display) -> ParseError {
	ParseError(why.to_string())
}

/// Read a lockfile.
pub fn read(lockfile: &str) -> Result<Lockfile, ParseError> {
	// A classic lockfile names itself in its header, under a banner saying it
	// is generated, so the header is read rather than only its first line.
	if lockfile
		.lines()
		.take_while(|line| line.trim().is_empty() || line.starts_with('#'))
		.any(|line| line.contains("yarn lockfile v1"))
	{
		return Err(unreadable(
			"it is a Yarn classic (version 1) lockfile, and only Yarn 2 and later's are read",
		));
	}

	let mut raw: BTreeMap<String, Option<Raw>> =
		serde_saphyr::from_str(lockfile).map_err(unreadable)?;
	if raw.remove("__metadata").is_none() {
		return Err(unreadable(
			"it has no __metadata, so is not a Yarn lockfile",
		));
	}

	let mut entries = Vec::with_capacity(raw.len());
	let mut by_descriptor = BTreeMap::new();
	for (key, raw) in raw {
		let raw =
			raw.ok_or_else(|| unreadable(format!("{} has no fields", without_credentials(&key))))?;
		let resolution = raw.resolution.ok_or_else(|| {
			unreadable(format!("{} has no resolution", without_credentials(&key)))
		})?;
		let (name, rest) = split_ident(&resolution).ok_or_else(|| {
			unreadable(format!(
				"the resolution {} names no package",
				without_credentials(&resolution)
			))
		})?;
		let (protocol, location) = rest.split_once(':').unwrap_or((rest, ""));

		let index = entries.len();
		for descriptor in key.split(", ") {
			by_descriptor.insert(descriptor.trim().to_owned(), index);
		}
		entries.push(Entry {
			name: name.to_owned(),
			protocol: protocol.to_owned(),
			location: location.to_owned(),
			version: raw.version,
			optional: raw
				.dependencies_meta
				.into_iter()
				.filter(|(_, meta)| meta.optional)
				.map(|(name, _)| {
					split_ident(&name).map_or(name.clone(), |(name, _)| name.to_owned())
				})
				.collect(),
			dependencies: raw.dependencies,
			resolution,
		});
	}
	Ok(Lockfile {
		entries,
		by_descriptor,
	})
}

impl Lockfile {
	/// Where the workspaces are, relative to the lockfile, whose `package.json`
	/// [`parse`](Self::parse) needs: the root is the empty string.
	pub fn workspaces(&self) -> Vec<&str> {
		self.entries
			.iter()
			.filter(|entry| entry.workspace())
			.map(|entry| match entry.location.as_str() {
				"." => "",
				location => location,
			})
			.collect()
	}

	/// The entry a dependency of `from` resolves to. A dependency on a file
	/// is resolved separately for each package that has it, so the entry made
	/// for `from` is the one wanted.
	fn locate(&self, from: &Entry, name: &str, descriptor: &str) -> Option<usize> {
		let descriptor = format!("{name}@{descriptor}");
		if let Some(index) = self.by_descriptor.get(&descriptor) {
			return Some(*index);
		}
		let prefix = format!("{descriptor}::locator=");
		let candidates: Vec<(&String, &usize)> = self
			.by_descriptor
			.range(prefix.clone()..)
			.take_while(|(key, _)| key.starts_with(&prefix))
			.collect();
		let made_for_from = candidates
			.iter()
			.find(|(key, _)| percent_decode(&key[prefix.len()..]) == from.resolution);
		made_for_from
			.or(candidates.first())
			.map(|(_, index)| **index)
	}

	/// Parse a lockfile into the repository's packages. `manifests` holds the
	/// `package.json` of each workspace by where it is, as
	/// [`workspaces`](Self::workspaces) gives them. `repository` names the
	/// workspace at the root where it has no name of its own.
	pub fn parse(
		&self,
		repository: &str,
		manifests: &BTreeMap<String, &str>,
	) -> Result<Vec<RepositoryPackage>, ParseError> {
		let mut kinds: BTreeMap<usize, Manifest> = BTreeMap::new();
		for (index, entry) in self.entries.iter().enumerate() {
			if !entry.workspace() {
				continue;
			}
			let at = if entry.location == "." {
				""
			} else {
				entry.location.as_str()
			};
			let text = *manifests
				.get(at)
				.ok_or_else(|| unreadable(format!("the workspace {at:?} has no package.json")))?;
			let manifest = serde_json::from_str(text).map_err(|e| {
				unreadable(format!(
					"package.json of the workspace {at:?} does not parse: {e}"
				))
			})?;
			kinds.insert(index, manifest);
		}

		let root = self
			.entries
			.iter()
			.position(|entry| entry.workspace() && entry.location == ".");
		let resolver = Resolver {
			lockfile: self,
			overrides: root
				.and_then(|root| kinds.get(&root))
				.into_iter()
				.flat_map(|manifest| &manifest.resolutions)
				.filter_map(|(pattern, target)| Override::new(pattern, target))
				.collect(),
			kinds,
			unresolved: RefCell::default(),
			resolved: RefCell::default(),
		};

		let mut found = Found::default();
		for entry in &self.entries {
			if let (true, Some(version)) = (entry.installed(), &entry.version) {
				found.hold(&entry.name, version);
			}
		}

		// The root counts as a workspace package when it depends on anything of
		// its own, as a repository that is one package does.
		for (workspace_index, manifest) in &resolver.kinds {
			let workspace = &self.entries[*workspace_index];
			if workspace.location == "." && workspace.dependencies.is_empty() {
				continue;
			}
			let name = if workspace.name == UNNAMED_ROOT {
				repository
			} else {
				&workspace.name
			};

			let roots: Vec<(Edge<usize>, Scope)> = workspace
				.dependencies
				.iter()
				.filter_map(|(dependency, descriptor)| {
					let (scope, optional) = manifest.kind(dependency);
					Some((
						resolver.edge(workspace, dependency, descriptor, optional)?,
						scope,
					))
				})
				.collect();

			walk(
				roots,
				|index: &usize| {
					resolver
						.edges_of(*index)
						.iter()
						.map(|(to, name, optional)| Edge {
							to: *to,
							name: name.clone(),
							optional: *optional,
						})
						.collect()
				},
				|index, scope, optional, chain| {
					let entry = &self.entries[*index];
					if let (true, Some(version)) = (entry.installed(), &entry.version) {
						found.reach(&entry.name, version, name, scope, optional, chain);
					}
				},
			);
		}

		let unresolved = resolver.unresolved.into_inner();
		if !unresolved.is_empty() {
			let shown: Vec<&str> = unresolved.iter().take(3).map(String::as_str).collect();
			return Err(unreadable(format!(
				"{} dependencies resolve to nothing the lockfile holds, such as {}",
				unresolved.len(),
				shown.join(", ")
			)));
		}
		Ok(found.finish(NPM))
	}
}

#[cfg(test)]
mod tests {
	use database::packages::PackagePath;

	use super::*;

	const LOCKFILE: &str = r#"# This file is generated by running "yarn install" inside your project.
# Manual changes might be lost - proceed with caution!

__metadata:
  version: 8
  cacheKey: 10c0

"@tamanu/server@workspace:packages/server":
  version: 0.0.0-use.local
  resolution: "@tamanu/server@workspace:packages/server"
  dependencies:
    "@tamanu/shared": "workspace:*"
    express: "npm:^4"
    fsevents: "npm:^2"
    jest: "npm:^29"
    xlsx: "file:../../vendor/xlsx-0.20.3.tgz"
  languageName: unknown
  linkType: soft

"@tamanu/shared@workspace:*, @tamanu/shared@workspace:packages/shared":
  version: 0.0.0-use.local
  resolution: "@tamanu/shared@workspace:packages/shared"
  dependencies:
    jest-extended: "npm:^4"
    lodash: "npm:^4"
  languageName: unknown
  linkType: soft

"@tamanu/web@workspace:packages/web":
  version: 0.0.0-use.local
  resolution: "@tamanu/web@workspace:packages/web"
  dependencies:
    lodash: "npm:^4"
    react: "npm:^18"
  languageName: unknown
  linkType: soft

"tamanu@workspace:.":
  version: 0.0.0-use.local
  resolution: "tamanu@workspace:."
  dependencies:
    prettier: "npm:^3"
  languageName: unknown
  linkType: soft

"body-parser@npm:1.20.1":
  version: 1.20.1
  resolution: "body-parser@npm:1.20.1"
  checksum: 10c0/aa
  languageName: node
  linkType: hard

"chalk@npm:^4":
  version: 4.1.2
  resolution: "chalk@npm:4.1.2"
  languageName: node
  linkType: hard

"express@npm:^4":
  version: 4.18.2
  resolution: "express@npm:4.18.2"
  dependencies:
    body-parser: "npm:1.20.1"
    lodash: "npm:^3"
  languageName: node
  linkType: hard

"fsevents@npm:^2":
  version: 2.3.3
  resolution: "fsevents@npm:2.3.3"
  conditions: os=darwin
  languageName: node
  linkType: hard

"fsevents@patch:fsevents@npm%3A^2#optional!builtin<compat/fsevents>":
  version: 2.3.3
  resolution: "fsevents@patch:fsevents@npm%3A2.3.3#optional!builtin<compat/fsevents>::version=2.3.3&hash=df0bf1"
  languageName: node
  linkType: hard

"jest-extended@npm:^4":
  version: 4.0.0
  resolution: "jest-extended@npm:4.0.0"
  languageName: node
  linkType: hard

"jest@npm:^29":
  version: 29.0.0
  resolution: "jest@npm:29.0.0"
  dependencies:
    chalk: "npm:^4"
    lodash: "npm:^4"
  languageName: node
  linkType: hard

"lodash@npm:^3":
  version: 3.10.1
  resolution: "lodash@npm:3.10.1"
  languageName: node
  linkType: hard

"lodash@npm:^4":
  version: 4.17.21
  resolution: "lodash@npm:4.17.21"
  languageName: node
  linkType: hard

"prettier@npm:^3":
  version: 3.0.0
  resolution: "prettier@npm:3.0.0"
  dependencies:
    sharp-darwin: "npm:^1"
  dependenciesMeta:
    sharp-darwin:
      optional: true
  languageName: node
  linkType: hard

"react@npm:^18":
  version: 18.2.0
  resolution: "react@npm:18.2.0"
  languageName: node
  linkType: hard

"sharp-darwin@npm:^1":
  version: 1.0.0
  resolution: "sharp-darwin@npm:1.0.0"
  languageName: node
  linkType: hard

"@types/node@npm:^20":
  version: 20.0.0
  resolution: "@types/node@npm:20.0.0"
  languageName: node
  linkType: hard

"xlsx@file:../../vendor/xlsx-0.20.3.tgz::locator=%40tamanu%2Fserver%40workspace%3Apackages%2Fserver":
  version: 0.20.3
  resolution: "xlsx@file:../../vendor/xlsx-0.20.3.tgz#../../vendor/xlsx-0.20.3.tgz::hash=a0b0ea&locator=%40tamanu%2Fserver%40workspace%3Apackages%2Fserver"
  languageName: node
  linkType: hard
"#;

	fn manifests() -> BTreeMap<String, &'static str> {
		let manifest = |text: &'static str| text;
		BTreeMap::from([
			(
				String::new(),
				manifest(r#"{"name":"tamanu","devDependencies":{"prettier":"^3"}}"#),
			),
			(
				"packages/server".into(),
				manifest(
					r#"{"name":"@tamanu/server",
					"dependencies":{"@tamanu/shared":"workspace:*","express":"^4","xlsx":"file:../../vendor/xlsx-0.20.3.tgz"},
					"devDependencies":{"jest":"^29"},
					"optionalDependencies":{"fsevents":"^2"}}"#,
				),
			),
			(
				"packages/shared".into(),
				manifest(
					r#"{"name":"@tamanu/shared","dependencies":{"lodash":"^4"},"devDependencies":{"jest-extended":"^4"}}"#,
				),
			),
			(
				"packages/web".into(),
				manifest(r#"{"name":"@tamanu/web","dependencies":{"react":"^18","lodash":"^4"}}"#),
			),
		])
	}

	fn packages() -> Vec<RepositoryPackage> {
		read(LOCKFILE)
			.unwrap()
			.parse("tamanu", &manifests())
			.unwrap()
	}

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
	fn the_workspaces_are_named_by_where_they_are() {
		let lockfile = read(LOCKFILE).unwrap();
		let mut workspaces = lockfile.workspaces();
		workspaces.sort();
		assert_eq!(
			workspaces,
			["", "packages/server", "packages/shared", "packages/web"]
		);
	}

	#[test]
	fn a_workspace_that_depends_on_a_package_in_development_reaches_it_by_that_scope() {
		let packages = packages();

		let jest = find(&packages, "jest", "29.0.0");
		assert_eq!(jest.paths.len(), 1);
		let direct = path(jest, "@tamanu/server", Scope::Development, false).unwrap();
		assert!(direct.direct);
		assert_eq!(direct.chain, ["jest"]);

		let chalk = find(&packages, "chalk", "4.1.2");
		let through = path(chalk, "@tamanu/server", Scope::Development, false).unwrap();
		assert!(!through.direct);
		assert_eq!(through.chain, ["jest", "chalk"]);

		let prettier = find(&packages, "prettier", "3.0.0");
		assert!(path(prettier, "tamanu", Scope::Development, false).is_some());
	}

	#[test]
	fn a_package_nothing_reaches_is_still_held_with_no_paths() {
		let packages = packages();
		assert!(find(&packages, "@types/node", "20.0.0").paths.is_empty());
		assert!(
			!packages.iter().any(|p| p.name.starts_with("@tamanu/")),
			"a workspace is the repository's own code, not a dependency"
		);
	}

	#[test]
	fn an_optional_dependency_is_an_optional_runtime_path() {
		let packages = packages();
		let fsevents = find(&packages, "fsevents", "2.3.3");
		assert_eq!(
			fsevents.paths.len(),
			1,
			"the patched copy is the same package"
		);
		assert!(
			path(fsevents, "@tamanu/server", Scope::Runtime, true)
				.unwrap()
				.direct
		);
	}

	#[test]
	fn a_dependency_under_another_name_is_held_under_the_one_it_really_has() {
		let lockfile = r#"__metadata:
  version: 8

"app@workspace:.":
  version: 0.0.0-use.local
  resolution: "app@workspace:."
  dependencies:
    react-router-dom-v6: "npm:react-router-dom@6.3.0"
  languageName: unknown
  linkType: soft

"react-router-dom-v6@npm:react-router-dom@6.3.0, react-router-dom@npm:6.3.0":
  version: 6.3.0
  resolution: "react-router-dom@npm:6.3.0"
  languageName: node
  linkType: hard
"#;
		let manifests = BTreeMap::from([(
			String::new(),
			r#"{"name":"app","dependencies":{"react-router-dom-v6":"npm:react-router-dom@6.3.0"}}"#,
		)]);
		let packages = read(lockfile).unwrap().parse("app", &manifests).unwrap();
		assert!(
			!packages.iter().any(|p| p.name == "react-router-dom-v6"),
			"the name it is asked for is not a package: {packages:#?}"
		);
		let real = find(&packages, "react-router-dom", "6.3.0");
		let reached = path(real, "app", Scope::Runtime, false).unwrap();
		assert_eq!(
			reached.chain,
			["react-router-dom-v6"],
			"the chain says what the workspace calls it"
		);
	}

	#[test]
	fn a_dependency_listed_as_both_plain_and_optional_is_optional() {
		let lockfile = r#"__metadata:
  version: 8

"app@workspace:.":
  version: 0.0.0-use.local
  resolution: "app@workspace:."
  dependencies:
    fsevents: "npm:^2"
  languageName: unknown
  linkType: soft

"fsevents@npm:^2":
  version: 2.3.3
  resolution: "fsevents@npm:2.3.3"
  languageName: node
  linkType: hard
"#;
		let manifests = BTreeMap::from([(
			String::new(),
			r#"{"name":"app",
			"dependencies":{"fsevents":"^2"},
			"optionalDependencies":{"fsevents":"^2"}}"#,
		)]);
		let packages = read(lockfile).unwrap().parse("app", &manifests).unwrap();
		let fsevents = find(&packages, "fsevents", "2.3.3");
		assert!(
			path(fsevents, "app", Scope::Runtime, true).is_some(),
			"optionalDependencies wins: {:?}",
			fsevents.paths
		);
	}

	/// The lockfile keys an overridden result by the new descriptor, while the
	/// dependent still asks for the one it wrote, so the descriptor it asks for
	/// can still be in the lockfile for a package that was not overridden.
	const COLLIDING: &str = r#"__metadata:
  version: 8

"app@workspace:.":
  version: 0.0.0-use.local
  resolution: "app@workspace:."
  dependencies:
    d2: "npm:^1"
    e2: "npm:^1"
  languageName: unknown
  linkType: soft

"d2@npm:^1":
  version: 1.0.0
  resolution: "d2@npm:1.0.0"
  dependencies:
    left-pad: "npm:^1"
  languageName: node
  linkType: hard

"e2@npm:^1":
  version: 1.0.0
  resolution: "e2@npm:1.0.0"
  dependencies:
    left-pad: "npm:^1"
  languageName: node
  linkType: hard

"left-pad@npm:^1":
  version: 1.0.0
  resolution: "left-pad@npm:1.0.0"
  languageName: node
  linkType: hard

"left-pad@npm:1.2.3":
  version: 1.2.3
  resolution: "left-pad@npm:1.2.3"
  languageName: node
  linkType: hard
"#;

	#[test]
	fn a_resolution_wins_over_the_descriptor_the_dependent_still_asks_for() {
		let manifests = BTreeMap::from([(
			String::new(),
			r#"{"name":"app","dependencies":{"d2":"^1","e2":"^1"},
			"resolutions":{"d2/left-pad":"1.2.3"}}"#,
		)]);
		let packages = read(COLLIDING).unwrap().parse("app", &manifests).unwrap();

		let overridden = find(&packages, "left-pad", "1.2.3");
		assert_eq!(
			path(overridden, "app", Scope::Runtime, false)
				.unwrap()
				.chain,
			["d2", "left-pad"],
			"the package the resolution names takes the version it gives"
		);

		let untouched = find(&packages, "left-pad", "1.0.0");
		assert_eq!(
			path(untouched, "app", Scope::Runtime, false).unwrap().chain,
			["e2", "left-pad"],
			"and nothing else is moved off the version it asked for"
		);
	}

	#[test]
	fn a_resolution_applies_to_the_dependent_its_pattern_names() {
		let dependent = |name: &str| Entry {
			name: name.to_owned(),
			protocol: "npm".into(),
			location: String::new(),
			resolution: format!("{name}@npm:1.0.0"),
			version: Some("1.0.0".into()),
			dependencies: BTreeMap::new(),
			optional: BTreeSet::new(),
		};
		let applies = |pattern: &str, to: &str| {
			Override::new(pattern, "1.2.3")
				.unwrap()
				.applies(&dependent(to), "left-pad", "npm:^1")
		};

		assert!(
			applies("left-pad", "anything"),
			"a bare name is any dependent"
		);
		assert!(applies("**/left-pad", "anything"));
		assert!(applies("d2/left-pad", "d2"));
		assert!(!applies("d2/left-pad", "e2"));

		assert!(applies("d2/**/left-pad", "d2"));
		assert!(
			!applies("d2/**/left-pad", "e2"),
			"a parent that names a package is not a wildcard"
		);

		// Every form Yarn's manifest documentation gives for the key.
		let named = |pattern: &str, to: &str| {
			let rule = Override::new(pattern, "1.2.3").unwrap();
			rule.applies(&dependent(to), &rule.name.clone(), "npm:^1")
		};
		assert!(named("relay-compiler", "anything"));
		assert!(named("webpack/memory-fs", "webpack"));
		assert!(!named("webpack/memory-fs", "rollup"));
		assert!(named("@babel/core/json5", "@babel/core"));
		assert!(named("@babel/core/@babel/generator", "@babel/core"));
		assert!(
			named("@babel/core@npm:7.0.0/@babel/generator", "@babel/core"),
			"a parent may be written as a descriptor"
		);
		assert_eq!(
			Override::new("@babel/core@npm:7.0.0/@babel/generator", "1.2.3")
				.unwrap()
				.name,
			"@babel/generator"
		);

		assert!(Override::new("left-pad@^1", "1.2.3").unwrap().applies(
			&dependent("d2"),
			"left-pad",
			"npm:^1"
		));
		assert!(!Override::new("left-pad@^2", "1.2.3").unwrap().applies(
			&dependent("d2"),
			"left-pad",
			"npm:^1"
		));
	}

	#[test]
	fn a_package_a_dependency_marks_optional_is_an_optional_path() {
		let packages = packages();
		let sharp = find(&packages, "sharp-darwin", "1.0.0");
		let optional = path(sharp, "tamanu", Scope::Development, true).unwrap();
		assert_eq!(optional.chain, ["prettier", "sharp-darwin"]);
	}

	#[test]
	fn a_workspace_another_reaches_passes_on_only_its_runtime_dependencies() {
		let packages = packages();

		let lodash = find(&packages, "lodash", "4.17.21");
		let through = path(lodash, "@tamanu/server", Scope::Runtime, false).unwrap();
		assert_eq!(through.chain, ["@tamanu/shared", "lodash"]);
		let development = path(lodash, "@tamanu/server", Scope::Development, false).unwrap();
		assert_eq!(development.chain, ["jest", "lodash"]);

		let extended = find(&packages, "jest-extended", "4.0.0");
		assert_eq!(extended.paths.len(), 1);
		assert!(path(extended, "@tamanu/shared", Scope::Development, false).is_some());
	}

	#[test]
	fn directness_and_the_shortest_chain_are_per_workspace() {
		let packages = packages();
		let lodash = find(&packages, "lodash", "4.17.21");

		let web = path(lodash, "@tamanu/web", Scope::Runtime, false).unwrap();
		assert!(web.direct);
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
	fn two_versions_of_a_package_are_told_apart_by_what_asks_for_them() {
		let packages = packages();
		let nested = find(&packages, "lodash", "3.10.1");
		assert_eq!(nested.paths.len(), 1);
		let path = path(nested, "@tamanu/server", Scope::Runtime, false).unwrap();
		assert_eq!(path.chain, ["express", "lodash"]);
	}

	#[test]
	fn a_file_dependency_is_the_copy_made_for_the_workspace_that_asked() {
		let packages = packages();
		let xlsx = find(&packages, "xlsx", "0.20.3");
		assert!(
			path(xlsx, "@tamanu/server", Scope::Runtime, false)
				.unwrap()
				.direct
		);
	}

	#[test]
	fn an_unnamed_root_workspace_is_named_for_the_repository() {
		let lockfile = LOCKFILE.replace(
			r#"tamanu@workspace:"#,
			&format!("{UNNAMED_ROOT}@workspace:"),
		);
		let packages = read(&lockfile)
			.unwrap()
			.parse("the-repo", &manifests())
			.unwrap();
		let prettier = find(&packages, "prettier", "3.0.0");
		assert!(path(prettier, "the-repo", Scope::Development, false).is_some());
	}

	#[test]
	fn a_root_that_depends_on_nothing_is_not_a_workspace_package() {
		let lockfile = LOCKFILE.replace(
			"  dependencies:\n    prettier: \"npm:^3\"\n  languageName: unknown",
			"  languageName: unknown",
		);
		let packages = read(&lockfile)
			.unwrap()
			.parse("tamanu", &manifests())
			.unwrap();
		assert!(
			packages
				.iter()
				.flat_map(|p| &p.paths)
				.all(|p| p.workspace_package != "tamanu")
		);
	}

	#[test]
	fn a_missing_or_broken_manifest_is_refused() {
		let lockfile = read(LOCKFILE).unwrap();
		let mut manifests = manifests();
		manifests.insert("packages/web".into(), "not json");
		assert!(lockfile.parse("tamanu", &manifests).is_err());
		manifests.remove("packages/web");
		assert!(lockfile.parse("tamanu", &manifests).is_err());
	}

	#[test]
	fn a_classic_or_unrelated_file_is_refused() {
		let classic = "# THIS IS AN AUTOGENERATED FILE. DO NOT EDIT THIS FILE DIRECTLY.\n\
		               # yarn lockfile v1\n\n\n\"a@^1\":\n  version \"1.0.0\"\n";
		let why = read(classic).unwrap_err().to_string();
		assert!(
			why.contains("classic"),
			"the marker sits under the banner, as a real one does: {why}"
		);
		assert!(read("name: not a lockfile\n").is_err());
		assert!(read("").is_err());
	}

	#[test]
	fn what_goes_wrong_is_said_without_the_credentials_in_it() {
		assert_eq!(
			without_credentials("https://ci-bot:ghp_secret@github.com/o/r.git#commit"),
			"https://***@github.com/o/r.git#commit"
		);
		assert_eq!(
			without_credentials("git+ssh://git@github.com/o/r.git"),
			"git+ssh://***@github.com/o/r.git"
		);
		assert_eq!(without_credentials("npm:^1.2.3"), "npm:^1.2.3");
		assert_eq!(
			without_credentials("https://registry.example/a.tgz"),
			"https://registry.example/a.tgz"
		);

		// A dependency on a host nothing holds, so the failure names it.
		let lockfile = r#"__metadata:
  version: 8

"app@workspace:.":
  version: 0.0.0-use.local
  resolution: "app@workspace:."
  dependencies:
    private: "https://ci-bot:ghp_secret@github.com/o/r.git"
  languageName: unknown
  linkType: soft
"#;
		let manifests = BTreeMap::from([(
			String::new(),
			r#"{"name":"app","dependencies":{"private":"https://ci-bot:ghp_secret@github.com/o/r.git"}}"#,
		)]);
		let why = read(lockfile)
			.unwrap()
			.parse("app", &manifests)
			.unwrap_err()
			.to_string();
		assert!(!why.contains("ghp_secret"), "{why}");
		assert!(why.contains("private"), "it still says what failed: {why}");
	}

	#[test]
	fn names_are_split_from_what_follows_them() {
		assert_eq!(
			split_ident("lodash@npm:4.0.0"),
			Some(("lodash", "npm:4.0.0"))
		);
		assert_eq!(
			split_ident("@scope/name@workspace:a/b"),
			Some(("@scope/name", "workspace:a/b"))
		);
		assert_eq!(split_ident("lodash"), None);
	}
}

#[cfg(test)]
mod scratch_real {
	use super::*;
	#[test]
	fn tupaia_unchanged() {
		let root = std::path::Path::new("/home/felix/code/work/tupaia");
		let lock = read(&std::fs::read_to_string(root.join("yarn.lock")).unwrap()).unwrap();
		let mut manifests = BTreeMap::new();
		for ws in lock.workspaces() {
			manifests.insert(
				ws.to_owned(),
				std::fs::read_to_string(root.join(ws).join("package.json"))
					.unwrap()
					.leak() as &str,
			);
		}
		let pkgs = lock.parse("tupaia", &manifests).unwrap();
		let mut scopes = std::collections::BTreeMap::<String, usize>::new();
		for p in &pkgs {
			for path in &p.paths {
				*scopes
					.entry(format!("{:?}/{}", path.scope, path.optional))
					.or_default() += 1;
			}
		}
		eprintln!(
			"packages {} unreached {} scopes {scopes:?}",
			pkgs.len(),
			pkgs.iter().filter(|p| p.paths.is_empty()).count()
		);
		let lodash: Vec<&str> = pkgs
			.iter()
			.filter(|p| p.name == "lodash")
			.map(|p| p.version.as_str())
			.collect();
		eprintln!("lodash versions {lodash:?}");
	}
}
