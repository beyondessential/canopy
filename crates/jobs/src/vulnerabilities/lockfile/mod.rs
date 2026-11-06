//! Reading the dependencies of a repository from its lockfiles.
//!
//! Spec: `.workhorse/specs/security/vulnerabilities.md` (id `VUL`), "Where a
//! package sits".
//!
//! Each ecosystem's parser returns the packages of one lockfile, each with the
//! paths that reach it from a workspace package: the scope of the path, whether
//! it is optional, whether the workspace package depends on it directly, and the
//! shortest chain of packages along it. A package reached by paths of different
//! scopes carries each of them.

use std::{
	collections::{BTreeMap, HashSet, VecDeque},
	hash::Hash,
};

use database::packages::{PackagePath, RepositoryPackage, Scope};

pub mod cargo;
pub mod npm;
pub mod uv;

/// Why a lockfile, or a manifest it needs, could not be read.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ParseError(pub String);

impl ParseError {
	fn new(why: impl std::fmt::Display) -> Self {
		Self(why.to_string())
	}
}

/// One step along a path: the package it arrives at, by the name the package
/// before it knows it by, and whether taking it makes the rest of the path
/// optional.
pub(super) struct Edge<N> {
	pub to: N,
	pub name: String,
	pub optional: bool,
}

/// Walk breadth first from a workspace package's own dependencies, keeping the
/// first, so shortest, chain to each node for each scope and optionality it is
/// reached with. A path keeps the scope of the dependency it started from and
/// turns optional once any step along it is. `reached` is called once for each
/// such way of reaching a node.
pub(super) fn walk<N: Clone + Eq + Hash>(
	roots: impl IntoIterator<Item = (Edge<N>, Scope)>,
	mut edges_of: impl FnMut(&N) -> Vec<Edge<N>>,
	mut reached: impl FnMut(&N, Scope, bool, &[String]),
) {
	let mut seen: HashSet<(N, Scope, bool)> = HashSet::new();
	let mut queue: VecDeque<(N, Scope, bool, Vec<String>)> = VecDeque::new();
	let mut visit = |node: N,
	                 scope: Scope,
	                 optional: bool,
	                 chain: Vec<String>,
	                 queue: &mut VecDeque<(N, Scope, bool, Vec<String>)>| {
		if seen.insert((node.clone(), scope, optional)) {
			reached(&node, scope, optional, &chain);
			queue.push_back((node, scope, optional, chain));
		}
	};
	for (edge, scope) in roots {
		visit(edge.to, scope, edge.optional, vec![edge.name], &mut queue);
	}
	while let Some((node, scope, optional, chain)) = queue.pop_front() {
		for edge in edges_of(&node) {
			let mut next = chain.clone();
			next.push(edge.name);
			visit(edge.to, scope, optional || edge.optional, next, &mut queue);
		}
	}
}

/// The packages of one lockfile, each with the shortest chain of each path
/// that reaches it.
#[derive(Default)]
pub(super) struct Found {
	by_package: BTreeMap<(String, String), BTreeMap<(String, Scope, bool), PackagePath>>,
}

impl Found {
	/// Record that the lockfile holds a package, whether or not anything is
	/// found to reach it.
	pub fn hold(&mut self, name: &str, version: &str) {
		self.by_package
			.entry((name.to_owned(), version.to_owned()))
			.or_default();
	}

	pub fn reach(
		&mut self,
		name: &str,
		version: &str,
		workspace_package: &str,
		scope: Scope,
		optional: bool,
		chain: &[String],
	) {
		let paths = self
			.by_package
			.entry((name.to_owned(), version.to_owned()))
			.or_default();
		let held = paths
			.entry((workspace_package.to_owned(), scope, optional))
			.or_insert_with(|| PackagePath {
				workspace_package: workspace_package.to_owned(),
				scope,
				optional,
				direct: false,
				chain: chain.to_vec(),
			});
		if chain.len() < held.chain.len() {
			held.chain = chain.to_vec();
		}
		held.direct = held.chain.len() == 1;
	}

	pub fn finish(self, ecosystem: &str) -> Vec<RepositoryPackage> {
		self.by_package
			.into_iter()
			.map(|((name, version), paths)| RepositoryPackage {
				ecosystem: ecosystem.to_owned(),
				name,
				version,
				paths: paths.into_values().collect(),
			})
			.collect()
	}
}
