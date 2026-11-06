//! Reading the dependencies of a tracked repository's default branch from
//! GitHub, without the unauthenticated API's rate limit of sixty requests an
//! hour getting in the way.
//!
//! Spec: `.workhorse/specs/security/vulnerabilities.md` (id `VUL`), "Where a
//! package sits" and "Exposure and closing".
//!
//! Three kinds of request are made, from cheapest to dearest:
//!
//! - the head of the default branch, from git's smart HTTP advertisement
//!   (`info/refs`), which uses none of the API's quota and so can be asked for
//!   every few minutes;
//! - only when the head has moved, the repository's tree at that commit, with
//!   one API request;
//! - the lockfiles, and the manifests Cargo's and Yarn's need, from the raw file
//!   host at that commit.
//!
//! Every `package-lock.json`, `yarn.lock`, `Cargo.lock` and `uv.lock` in the
//! tree counts, apart from those inside `node_modules`. A repository that cannot
//! be reached, or has a lockfile that does not parse, is not read at all: what
//! is held for it stays as it was.

use std::{
	collections::{BTreeMap, BTreeSet},
	fmt,
	time::Duration,
};

use database::{packages::RepositoryPackage, vulnerabilities::Repository};
use futures::{StreamExt as _, TryStreamExt as _, stream};
use serde::Deserialize;

use super::lockfile::{self, ParseError};

/// Where each kind of request goes. Only a test points these anywhere but
/// GitHub.
#[derive(Clone, Debug)]
pub struct Endpoints {
	/// Git's smart HTTP, which names a repository's head.
	pub git: String,
	/// GitHub's API, which lists a tree.
	pub api: String,
	/// Where raw file contents are served.
	pub raw: String,
}

impl Default for Endpoints {
	fn default() -> Self {
		Self {
			git: "https://github.com".into(),
			api: "https://api.github.com".into(),
			raw: "https://raw.githubusercontent.com".into(),
		}
	}
}

/// Why a repository could not be read.
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
	/// GitHub could not be reached, or did not give what was asked for.
	Unreachable(String),
	/// A lockfile, or a manifest it needs, does not parse.
	Unparseable { lockfile: String, reason: String },
}

impl fmt::Display for Failure {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Unreachable(why) => f.write_str(why),
			Self::Unparseable { lockfile, reason } => {
				write!(f, "{lockfile} does not parse: {reason}")
			}
		}
	}
}

impl std::error::Error for Failure {}

/// How many files are fetched at once.
const FETCH_CONCURRENCY: usize = 8;

/// The most API requests walking a tree directory by directory may take, for
/// the rare repository whose whole tree is too big for one listing.
const MAX_DIRECTORY_LISTINGS: usize = 60;

#[derive(Clone)]
pub struct GitHub {
	http: reqwest::Client,
	endpoints: Endpoints,
}

#[derive(Deserialize)]
struct Tree {
	#[serde(default)]
	tree: Vec<TreeEntry>,
	#[serde(default)]
	truncated: bool,
}

#[derive(Deserialize)]
struct TreeEntry {
	path: String,
	#[serde(rename = "type")]
	kind: String,
	#[serde(default)]
	sha: String,
}

/// The kind of lockfile a file name is, if it is one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
	Npm,
	Yarn,
	Cargo,
	Uv,
}

impl Kind {
	fn of(path: &str) -> Option<Self> {
		if path.starts_with("node_modules/") || path.contains("/node_modules/") {
			return None;
		}
		match path.rsplit('/').next()? {
			"package-lock.json" => Some(Self::Npm),
			"yarn.lock" => Some(Self::Yarn),
			"Cargo.lock" => Some(Self::Cargo),
			"uv.lock" => Some(Self::Uv),
			_ => None,
		}
	}
}

fn directory(path: &str) -> &str {
	path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn join(dir: &str, file: &str) -> String {
	if dir.is_empty() {
		file.to_owned()
	} else {
		format!("{dir}/{file}")
	}
}

/// The commit a smart HTTP advertisement says `HEAD` is at.
///
/// The advertisement is a series of length-prefixed lines, the first ref's
/// carrying the server's capabilities after a NUL.
fn advertised_head(body: &[u8]) -> Option<String> {
	let mut rest = body;
	while rest.len() >= 4 {
		let length = usize::from_str_radix(std::str::from_utf8(&rest[..4]).ok()?, 16).ok()?;
		if length == 0 {
			rest = &rest[4..];
			continue;
		}
		if length < 4 || length > rest.len() {
			return None;
		}
		let line = &rest[4..length];
		rest = &rest[length..];
		let line = line.split(|byte| *byte == 0).next().unwrap_or_default();
		let line = std::str::from_utf8(line).ok()?.trim_end();
		if let Some((sha, name)) = line.split_once(' ')
			&& name == "HEAD"
			&& sha.len() >= 4
			&& sha.bytes().all(|byte| byte.is_ascii_hexdigit())
		{
			return Some(sha.to_ascii_lowercase());
		}
	}
	None
}

impl GitHub {
	pub fn new(endpoints: Endpoints) -> Self {
		Self {
			http: reqwest::Client::builder()
				.user_agent("canopy-vulnerability-scan")
				.timeout(Duration::from_secs(120))
				.build()
				.unwrap_or_default(),
			endpoints,
		}
	}

	fn url(base: &str, segments: &[&str]) -> Result<reqwest::Url, Failure> {
		let unusable = |why: &dyn fmt::Display| {
			Failure::Unreachable(format!("the address {base} is unusable: {why}"))
		};
		let mut url = reqwest::Url::parse(base).map_err(|e| unusable(&e))?;
		url.path_segments_mut()
			.map_err(|()| unusable(&"it cannot take a path"))?
			.pop_if_empty()
			.extend(segments);
		Ok(url)
	}

	async fn get(&self, what: &str, url: reqwest::Url) -> Result<reqwest::Response, Failure> {
		let response = self
			.http
			.get(url)
			.send()
			.await
			.map_err(|e| Failure::Unreachable(format!("{what}: {e}")))?;
		let status = response.status();
		if status.is_success() {
			Ok(response)
		} else if status == reqwest::StatusCode::FORBIDDEN
			|| status == reqwest::StatusCode::TOO_MANY_REQUESTS
		{
			Err(Failure::Unreachable(format!(
				"{what}: GitHub answered {status}, perhaps because a rate limit was reached"
			)))
		} else {
			Err(Failure::Unreachable(format!(
				"{what}: GitHub answered {status}"
			)))
		}
	}

	/// The commit the default branch of `slug` is at, from git's smart HTTP,
	/// which costs none of the API's quota.
	pub async fn head(&self, slug: &str) -> Result<String, Failure> {
		let mut segments: Vec<String> = slug.split('/').map(str::to_owned).collect();
		if let Some(last) = segments.last_mut() {
			last.push_str(".git");
		}
		segments.extend(["info".to_owned(), "refs".to_owned()]);
		let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
		let mut url = Self::url(&self.endpoints.git, &segments)?;
		url.query_pairs_mut()
			.append_pair("service", "git-upload-pack");
		let what = format!("asking for the head of {slug}");
		let body = self
			.get(&what, url)
			.await?
			.bytes()
			.await
			.map_err(|e| Failure::Unreachable(format!("{what}: {e}")))?;
		advertised_head(&body).ok_or_else(|| {
			Failure::Unreachable(format!(
				"{what}: it has no head, or the answer is not git's"
			))
		})
	}

	async fn tree(&self, slug: &str, sha: &str, recursive: bool) -> Result<Tree, Failure> {
		let mut segments: Vec<&str> = vec!["repos"];
		segments.extend(slug.split('/'));
		segments.extend(["git", "trees", sha]);
		let mut url = Self::url(&self.endpoints.api, &segments)?;
		if recursive {
			url.query_pairs_mut().append_pair("recursive", "1");
		}
		let what = format!("listing {slug} at {sha}");
		self.get(&what, url)
			.await?
			.json()
			.await
			.map_err(|e| Failure::Unreachable(format!("{what}: {e}")))
	}

	/// The path of every file in the repository at `commit`.
	async fn files(&self, slug: &str, commit: &str) -> Result<Vec<String>, Failure> {
		let listing = self.tree(slug, commit, true).await?;
		if !listing.truncated {
			return Ok(listing
				.tree
				.into_iter()
				.filter(|entry| entry.kind == "blob")
				.map(|entry| entry.path)
				.collect());
		}

		// Too big for one listing: go directory by directory, leaving out what
		// no lockfile that counts is in.
		let mut files = Vec::new();
		let mut pending = vec![(String::new(), commit.to_owned())];
		let mut listings = 0;
		while let Some((dir, sha)) = pending.pop() {
			listings += 1;
			if listings > MAX_DIRECTORY_LISTINGS {
				return Err(Failure::Unreachable(format!(
					"{slug} has too many files to list in {MAX_DIRECTORY_LISTINGS} requests"
				)));
			}
			for entry in self.tree(slug, &sha, false).await?.tree {
				let path = join(&dir, &entry.path);
				match entry.kind.as_str() {
					"blob" => files.push(path),
					"tree" if entry.path != "node_modules" && entry.path != ".git" => {
						pending.push((path, entry.sha));
					}
					_ => {}
				}
			}
		}
		Ok(files)
	}

	async fn raw(&self, slug: &str, commit: &str, path: &str) -> Result<String, Failure> {
		let mut segments: Vec<&str> = slug.split('/').collect();
		segments.push(commit);
		segments.extend(path.split('/'));
		let url = Self::url(&self.endpoints.raw, &segments)?;
		let what = format!("reading {path} of {slug}");
		self.get(&what, url)
			.await?
			.text()
			.await
			.map_err(|e| Failure::Unreachable(format!("{what}: {e}")))
	}

	async fn raw_all(
		&self,
		slug: &str,
		commit: &str,
		paths: &BTreeSet<String>,
	) -> Result<BTreeMap<String, String>, Failure> {
		let fetches: Vec<_> = paths
			.iter()
			.map(|path| self.raw_named(slug, commit, path))
			.collect();
		stream::iter(fetches)
			.buffer_unordered(FETCH_CONCURRENCY)
			.try_collect()
			.await
	}

	async fn raw_named(
		&self,
		slug: &str,
		commit: &str,
		path: &str,
	) -> Result<(String, String), Failure> {
		let text = self.raw(slug, commit, path).await?;
		Ok((path.to_owned(), text))
	}

	/// Read the dependencies of `repository` as its default branch stands at
	/// `commit`: every lockfile in it, each with the manifests it needs.
	pub async fn read(
		&self,
		repository: &Repository,
		commit: &str,
	) -> Result<Vec<RepositoryPackage>, Failure> {
		let slug = repository.slug;
		let files = self.files(slug, commit).await?;
		let lockfiles: Vec<(String, Kind)> = files
			.iter()
			.filter_map(|path| Some((path.clone(), Kind::of(path)?)))
			.collect();

		// The lockfiles, and the root manifest beside each Cargo.lock, which
		// says where the members are.
		let mut first: BTreeSet<String> = BTreeSet::new();
		for (path, kind) in &lockfiles {
			first.insert(path.clone());
			if *kind == Kind::Cargo {
				let manifest = join(directory(path), "Cargo.toml");
				if !files.contains(&manifest) {
					return Err(Failure::Unparseable {
						lockfile: path.clone(),
						reason: "there is no Cargo.toml beside it".into(),
					});
				}
				first.insert(manifest);
			}
		}
		let mut texts = self.raw_all(slug, commit, &first).await?;

		// The manifests of each Cargo workspace's members, and of each Yarn
		// workspace, which the lockfile names.
		let mut members: BTreeMap<String, Vec<String>> = BTreeMap::new();
		let mut yarn: BTreeMap<String, lockfile::yarn::Lockfile> = BTreeMap::new();
		let mut second: BTreeSet<String> = BTreeSet::new();
		for (path, kind) in &lockfiles {
			if *kind == Kind::Yarn {
				let parsed =
					lockfile::yarn::read(&texts[path]).map_err(|e| unparseable(path, e))?;
				for workspace in parsed.workspaces() {
					let manifest = workspace_manifest(path, workspace);
					if !files.contains(&manifest) {
						return Err(Failure::Unparseable {
							lockfile: path.clone(),
							reason: format!("the workspace {workspace:?} has no {manifest}"),
						});
					}
					second.insert(manifest);
				}
				yarn.insert(path.clone(), parsed);
			}
			if *kind != Kind::Cargo {
				continue;
			}
			let dir = directory(path);
			let root = join(dir, "Cargo.toml");
			let workspace =
				lockfile::cargo::workspace(&texts[&root]).map_err(|e| unparseable(&root, e))?;
			let matching = member_manifests(&files, dir, &workspace);
			second.extend(matching.iter().filter(|m| **m != root).cloned());
			members.insert(path.clone(), matching);
		}
		second.retain(|path| !texts.contains_key(path));
		texts.extend(self.raw_all(slug, commit, &second).await?);

		let mut read: Vec<(String, Vec<RepositoryPackage>)> = Vec::new();
		for (path, kind) in &lockfiles {
			let text = &texts[path];
			let packages = match kind {
				Kind::Npm => lockfile::npm::parse(text, repository.name),
				Kind::Yarn => {
					let manifests = yarn[path]
						.workspaces()
						.into_iter()
						.map(|workspace| {
							let manifest = workspace_manifest(path, workspace);
							(workspace.to_owned(), texts[&manifest].as_str())
						})
						.collect();
					yarn[path].parse(repository.name, &manifests)
				}
				Kind::Uv => lockfile::uv::parse(text),
				Kind::Cargo => {
					let manifests: Vec<&str> = members[path]
						.iter()
						.map(|manifest| texts[manifest].as_str())
						.collect();
					lockfile::cargo::parse(text, &manifests)
				}
			}
			.map_err(|e| unparseable(path, e))?;
			read.push((directory(path).to_owned(), packages));
		}
		Ok(qualified(read))
	}
}

/// The `package.json` of the Yarn workspace at `workspace`, the root being the
/// empty string, of the lockfile at `lockfile`.
fn workspace_manifest(lockfile: &str, workspace: &str) -> String {
	let dir = join(directory(lockfile), workspace);
	let dir = if workspace.is_empty() {
		directory(lockfile)
	} else {
		&dir
	};
	join(dir, "package.json")
}

fn unparseable(lockfile: &str, e: ParseError) -> Failure {
	Failure::Unparseable {
		lockfile: lockfile.to_owned(),
		reason: e.to_string(),
	}
}

/// The `Cargo.toml` of the root of the workspace at `dir`, and of each member
/// of it the tree has, which is every manifest in a directory a member glob
/// matches and no exclusion does.
fn member_manifests(
	files: &[String],
	dir: &str,
	workspace: &lockfile::cargo::Workspace,
) -> Vec<String> {
	let options = glob::MatchOptions {
		case_sensitive: true,
		require_literal_separator: true,
		require_literal_leading_dot: false,
	};
	let patterns = |globs: &[String]| -> Vec<glob::Pattern> {
		globs
			.iter()
			.filter_map(|glob| glob::Pattern::new(glob).ok())
			.collect()
	};
	let (include, exclude) = (patterns(&workspace.members), patterns(&workspace.exclude));

	let root = join(dir, "Cargo.toml");
	let mut manifests = vec![root.clone()];
	for file in files {
		if *file == root || file.rsplit('/').next() != Some("Cargo.toml") {
			continue;
		}
		let member = directory(file);
		let relative = if dir.is_empty() {
			member
		} else if let Some(relative) = member
			.strip_prefix(dir)
			.and_then(|relative| relative.strip_prefix('/'))
		{
			relative
		} else {
			continue;
		};
		if include.iter().any(|p| p.matches_with(relative, options))
			&& !exclude.iter().any(|p| p.matches_with(relative, options))
		{
			manifests.push(file.clone());
		}
	}
	manifests
}

/// The packages of every lockfile together, with a workspace package named by
/// its lockfile's directory as well wherever two lockfiles have one of the same
/// name, so each is told apart.
fn qualified(read: Vec<(String, Vec<RepositoryPackage>)>) -> Vec<RepositoryPackage> {
	let mut lockfiles_by_name: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
	for (dir, packages) in &read {
		for path in packages.iter().flat_map(|package| &package.paths) {
			lockfiles_by_name
				.entry(&path.workspace_package)
				.or_default()
				.insert(dir);
		}
	}
	let shared: BTreeSet<String> = lockfiles_by_name
		.into_iter()
		.filter(|(_, dirs)| dirs.len() > 1)
		.map(|(name, _)| name.to_owned())
		.collect();

	let mut out = Vec::new();
	for (dir, packages) in read {
		for mut package in packages {
			for path in &mut package.paths {
				if shared.contains(&path.workspace_package) {
					path.workspace_package = format!(
						"{} ({})",
						path.workspace_package,
						if dir.is_empty() { "." } else { &dir }
					);
				}
			}
			out.push(package);
		}
	}
	out
}

#[cfg(test)]
mod tests {
	use database::packages::{PackagePath, Scope};

	use super::*;

	fn pkt(line: &str) -> String {
		format!("{:04x}{line}", line.len() + 4)
	}

	#[test]
	fn the_head_is_read_from_a_smart_http_advertisement() {
		let sha = "0123456789abcdef0123456789abcdef01234567";
		let other = "fedcba9876543210fedcba9876543210fedcba98";
		let body = format!(
			"{}0000{}{}{}0000",
			pkt("# service=git-upload-pack\n"),
			pkt(&format!(
				"{sha} HEAD\0multi_ack symref=HEAD:refs/heads/main agent=git/github\n"
			)),
			pkt(&format!("{other} refs/heads/other\n")),
			pkt(&format!("{sha} refs/heads/main\n")),
		);
		assert_eq!(advertised_head(body.as_bytes()).as_deref(), Some(sha));
	}

	#[test]
	fn an_advertisement_with_no_head_or_not_git_names_none() {
		let empty = format!(
			"{}0000{}0000",
			pkt("# service=git-upload-pack\n"),
			pkt(&format!(
				"{} capabilities^{{}}\0agent=git\n",
				"0".repeat(40)
			))
		);
		assert_eq!(advertised_head(empty.as_bytes()), None);
		assert_eq!(advertised_head(b"<html>not git</html>"), None);
		assert_eq!(advertised_head(b""), None);
	}

	#[test]
	fn lockfiles_in_node_modules_do_not_count() {
		assert_eq!(Kind::of("package-lock.json"), Some(Kind::Npm));
		assert_eq!(Kind::of("a/b/Cargo.lock"), Some(Kind::Cargo));
		assert_eq!(Kind::of("ansible/uv.lock"), Some(Kind::Uv));
		assert_eq!(Kind::of("node_modules/x/package-lock.json"), None);
		assert_eq!(Kind::of("web/node_modules/x/package-lock.json"), None);
		assert_eq!(Kind::of("yarn.lock"), Some(Kind::Yarn));
		assert_eq!(Kind::of("web/node_modules/x/yarn.lock"), None);
	}

	#[test]
	fn member_globs_are_resolved_against_the_tree() {
		let files: Vec<String> = [
			"Cargo.toml",
			"crates/a/Cargo.toml",
			"crates/b/Cargo.toml",
			"crates/old/Cargo.toml",
			"crates/a/nested/Cargo.toml",
			"fixtures/Cargo.toml",
			"sub/Cargo.toml",
			"sub/crates/c/Cargo.toml",
		]
		.map(String::from)
		.to_vec();
		let workspace = lockfile::cargo::Workspace {
			members: vec!["crates/*".into()],
			exclude: vec!["crates/old".into()],
		};
		assert_eq!(
			member_manifests(&files, "", &workspace),
			["Cargo.toml", "crates/a/Cargo.toml", "crates/b/Cargo.toml"]
		);
		assert_eq!(
			member_manifests(&files, "sub", &workspace),
			["sub/Cargo.toml", "sub/crates/c/Cargo.toml"]
		);
	}

	#[test]
	fn a_workspace_package_is_qualified_only_where_two_lockfiles_share_its_name() {
		let package = |name: &str, workspace: &str| RepositoryPackage {
			ecosystem: "npm".into(),
			name: name.into(),
			version: "1.0.0".into(),
			paths: vec![PackagePath {
				workspace_package: workspace.into(),
				scope: Scope::Runtime,
				optional: false,
				direct: true,
				chain: vec![name.into()],
			}],
		};
		let out = qualified(vec![
			(
				String::new(),
				vec![package("a", "app"), package("b", "web")],
			),
			("tools".to_owned(), vec![package("c", "app")]),
		]);
		let names: Vec<&str> = out
			.iter()
			.map(|p| p.paths[0].workspace_package.as_str())
			.collect();
		assert_eq!(names, ["app (.)", "web", "app (tools)"]);
	}
}
