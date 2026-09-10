use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use commons_errors::{AppError, Result};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::versions::Version;

/// Which artifacts a read may see.
// spec: ART#who-is-offered-a-group-scoped-artifact
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
	/// The unscoped artifacts alone: a read carrying no identity, or one whose
	/// caller has no group of its own.
	Unscoped,
	/// The unscoped artifacts plus the named group's.
	Group(Uuid),
	/// Every artifact, whatever group it belongs to. Operator views only.
	Fleet,
}

impl Scope {
	/// What a caller resolving to this group may see.
	pub fn for_caller(group: Option<Uuid>) -> Self {
		match group {
			Some(group) => Self::Group(group),
			None => Self::Unscoped,
		}
	}

	/// Whether an artifact of this group is in scope.
	fn sees(self, group: Option<Uuid>) -> bool {
		match self {
			Self::Unscoped => group.is_none(),
			Self::Group(caller) => group.is_none() || group == Some(caller),
			Self::Fleet => true,
		}
	}
}

/// A downloadable artifact belonging to a release version: an installer,
/// package, or other file published for a given type and platform.
///
/// The bytes of a group-scoped artifact are not loaded here; they are large,
/// and every listing would carry them. Read them with [`Artifact::content_for`].
#[derive(Debug, Clone, Deserialize, Queryable, Selectable, Associations)]
#[diesel(belongs_to(Version))]
#[diesel(table_name = crate::schema::artifacts)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Artifact {
	/// Unique identifier of the artifact.
	pub id: Uuid,
	/// The exact version this artifact belongs to. `null` for range
	/// artifacts, which apply to every version matching
	/// `version_range_pattern` instead.
	pub version_id: Option<Uuid>,
	/// What kind of artifact this is (e.g. an installer or package name).
	pub artifact_type: String,
	/// The platform the artifact targets (e.g. an OS or architecture name).
	pub platform: String,
	/// URL the artifact can be downloaded from. `null` for a group-scoped
	/// artifact, whose bytes Canopy holds instead.
	pub download_url: Option<String>,
	/// The device that registered this artifact, if it was registered by a
	/// releaser device rather than created by an operator.
	pub device_id: Option<Uuid>,
	/// Semver range this artifact applies to (e.g. `^2.10.0`), for artifacts
	/// shared across a range of versions rather than pinned to one. `null`
	/// for exact-version artifacts.
	pub version_range_pattern: Option<String>,
	/// The group this artifact is for. `null` for an artifact that is for
	/// every group.
	pub group_id: Option<Uuid>,
	/// Media type of the bytes Canopy holds, where the registration named one.
	pub content_type: Option<String>,
	/// SHA-256 of the artifact's bytes. Always set for a group-scoped
	/// artifact.
	pub digest: Option<Vec<u8>>,
	/// The run that produced this artifact, where the registration named one.
	pub run_id: Option<Uuid>,
	/// When this artifact was last registered.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub updated_at: jiff::Timestamp,
}

#[derive(Debug, Deserialize, Insertable)]
#[diesel(belongs_to(Version))]
#[diesel(table_name = crate::schema::artifacts)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct NewArtifact {
	pub version_id: Option<Uuid>,
	pub artifact_type: String,
	pub platform: String,
	pub download_url: Option<String>,
	pub device_id: Option<Uuid>,
	pub version_range_pattern: Option<String>,
	pub group_id: Option<Uuid>,
	pub content: Option<Vec<u8>>,
	pub content_type: Option<String>,
	pub digest: Option<Vec<u8>>,
	pub run_id: Option<Uuid>,
}

/// The bytes Canopy holds for a group-scoped artifact.
pub struct ArtifactContent {
	pub bytes: Vec<u8>,
	pub content_type: Option<String>,
	pub digest: Vec<u8>,
}

/// Cap on the bytes Canopy will hold for one artifact. A reporting schema is a
/// SQL file; anything approaching this is not one, and the rows live in Postgres
/// alongside everything else.
pub const MAX_HELD_ARTIFACT_BYTES: usize = 32 * 1024 * 1024;

/// The digest Canopy records and verifies bytes against.
pub fn digest_of(bytes: &[u8]) -> Vec<u8> {
	Sha256::digest(bytes).to_vec()
}

/// A digest as Subresource Integrity writes it, which is the form every
/// interface carries it in.
// spec: ART#digests
pub fn sri(digest: &[u8]) -> String {
	format!("sha256-{}", BASE64.encode(digest))
}

/// The digest an SRI string names, refusing anything that cannot be one.
///
/// A value nothing can check the bytes against is worse than none: it says the
/// bytes were verified when they cannot be.
// spec: ART#digests
pub fn parse_sri(value: &str) -> Result<Vec<u8>> {
	let refuse = || AppError::BadRequest(format!("{value:?} is not a sha256 SRI digest"));

	let encoded = value.trim().strip_prefix("sha256-").ok_or_else(refuse)?;
	let digest = BASE64.decode(encoded).map_err(|_| refuse())?;
	if digest.len() != 32 {
		return Err(refuse());
	}

	Ok(digest)
}

/// A blank URL is no location at all. The constraint only tests for NULL, so an
/// empty string passes it and leaves an artifact nothing can be fetched from.
///
/// The trimmed value is what is kept: a plain-text body picks up whatever
/// newline the shell that sent it added, and a URL nothing can parse is offered
/// to every device that asks.
// spec: ART#where-an-artifact-rests
pub fn location(url: Option<String>) -> Option<String> {
	url.map(|url| url.trim().to_owned())
		.filter(|url| !url.is_empty())
}

/// The digest a registration names, where it names one at all.
///
/// A blank digest is no digest: recorded, it says the bytes were checked
/// against something when nothing was.
// spec: ART#digests
pub fn parse_sri_opt(value: Option<&str>) -> Result<Option<Vec<u8>>> {
	value
		.map(str::trim)
		.filter(|value| !value.is_empty())
		.map(parse_sri)
		.transpose()
}

impl NewArtifact {
	/// Settle where this artifact rests, refusing a registration that names
	/// neither place or both.
	///
	/// The database constrains the same shape, so a write that skips this
	/// answers a caller with a 500 rather than a refusal.
	// spec: ART#where-an-artifact-rests
	fn resting(mut self) -> Result<Self> {
		self.download_url = location(self.download_url);

		match (self.group_id.is_some(), self.download_url.is_some()) {
			(true, true) => Err(AppError::BadRequest(
				"an artifact Canopy holds has no download URL".into(),
			)),
			(false, false) => Err(AppError::BadRequest(
				"an artifact needs a download URL or a group".into(),
			)),
			(true, false) if self.content.is_none() || self.digest.is_none() => {
				Err(AppError::BadRequest(
					"a group-scoped artifact must carry its bytes and their digest".into(),
				))
			}
			(false, true) if self.content_type.is_some() || self.content.is_some() => Err(
				AppError::BadRequest("only a group-scoped artifact carries bytes".into()),
			),
			_ => Ok(self),
		}
	}
}

impl Artifact {
	/// The artifacts of a version that `scope` may see, one per type and
	/// platform, most specific first.
	// spec: ART#what-a-version-offers
	pub async fn get_for_version(
		db: &mut AsyncPgConnection,
		target_version_id: Uuid,
		scope: Scope,
	) -> Result<Vec<Self>> {
		let artifacts = Self::get_for_version_all_matches(db, target_version_id, scope).await?;
		Ok(Self::offered(artifacts, scope))
	}

	/// The artifacts of a sorted match set that `scope` is actually served:
	/// the most specific of each type and platform it can see.
	///
	/// Not `dedup_by_key`: that only drops *consecutive* duplicates, and the
	/// specificity sort has destroyed the adjacency the SQL `ORDER BY` gave us
	/// — every exact artifact now precedes every range one, so two artifacts of
	/// the same type+platform are only neighbours when they happen to be
	/// equally specific.
	// spec: ART#what-a-version-offers
	fn offered(artifacts: Vec<Self>, scope: Scope) -> Vec<Self> {
		let offered = Self::offered_ids(&artifacts, scope);
		artifacts
			.into_iter()
			.filter(|a| offered.contains(&a.id))
			.collect()
	}

	/// The ids of the artifacts a sorted match set offers `scope`.
	///
	/// The operator view answers the same question per resolved scope, so the
	/// rule lives here rather than being written out again beside it: the two
	/// disagreeing would have the listing mark an artifact as served that the
	/// public path passes over.
	// spec: ART#what-a-version-offers
	fn offered_ids(artifacts: &[Self], scope: Scope) -> std::collections::HashSet<Uuid> {
		let mut seen = std::collections::HashSet::new();
		artifacts
			.iter()
			.filter(|a| scope.sees(a.group_id))
			.filter(|a| seen.insert((a.artifact_type.as_str(), a.platform.as_str())))
			.map(|a| a.id)
			.collect()
	}

	/// One of a version's artifacts, as `scope` may see it.
	///
	/// The version and the scope are both part of the read: an artifact of
	/// another version, or of a group this caller is not in, is missing in
	/// exactly the way one that never existed is. Read by id rather than by
	/// taking the version's whole match set, which is every range artifact in
	/// the table on the path every machine fetches from.
	// spec: ART#who-is-offered-a-group-scoped-artifact
	pub async fn of_version(
		db: &mut AsyncPgConnection,
		artifact_id: Uuid,
		version: &Version,
		scope: Scope,
	) -> Result<Option<Self>> {
		use crate::schema::artifacts::dsl::*;

		let mut query = artifacts.filter(id.eq(artifact_id)).into_boxed();
		query = match scope {
			Scope::Unscoped => query.filter(group_id.is_null()),
			Scope::Group(caller) => query.filter(group_id.is_null().or(group_id.eq(caller))),
			Scope::Fleet => query,
		};

		let row: Option<Self> = query
			.select(Self::as_select())
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)?;

		Ok(row.filter(|row| row.belongs_to_version(version)))
	}

	/// Whether this artifact is one of `version`'s: named by it, or registered
	/// for a range that covers it.
	fn belongs_to_version(&self, version: &Version) -> bool {
		if self.version_id == Some(version.id) {
			return true;
		}

		// An unparseable pattern matches nothing rather than everything, so a
		// malformed range withholds a file instead of offering it to the whole
		// fleet.
		self.version_range_pattern
			.as_deref()
			.and_then(|pattern| node_semver::Range::parse(pattern).ok())
			.is_some_and(|range| range.satisfies(&version.as_semver()))
	}

	/// Every artifact of a version that `scope` may see, sorted most specific
	/// first and not deduplicated. For operator views.
	// spec: ART#what-a-version-offers
	pub async fn get_for_version_all_matches(
		db: &mut AsyncPgConnection,
		target_version_id: Uuid,
		scope: Scope,
	) -> Result<Vec<Self>> {
		use crate::schema::artifacts::*;

		let version = crate::versions::Version::get_by_id(db, target_version_id).await?;
		let semver = version.as_semver();

		let mut query = table
			.select(Self::as_select())
			.filter(
				version_id
					.eq(Some(target_version_id))
					.or(version_range_pattern.is_not_null()),
			)
			.into_boxed();

		query = match scope {
			Scope::Unscoped => query.filter(group_id.is_null()),
			Scope::Group(caller) => query.filter(group_id.is_null().or(group_id.eq(caller))),
			Scope::Fleet => query,
		};

		let mut artifacts: Vec<Self> = query
			.order_by(artifact_type.asc())
			.then_order_by(platform.asc())
			.load(db)
			.await
			.map_err(AppError::from)?;

		artifacts.retain(|artifact| {
			if artifact.version_id == Some(target_version_id) {
				true
			} else if let Some(pattern) = &artifact.version_range_pattern {
				// An unparseable pattern matches nothing rather than
				// everything, so a malformed range withholds a file instead of
				// offering it to the whole fleet.
				match node_semver::Range::parse(pattern) {
					Ok(range) => range.satisfies(&semver),
					Err(_) => false,
				}
			} else {
				false
			}
		});

		Self::sort_by_specificity(&mut artifacts);

		Ok(artifacts)
	}

	/// Sort artifacts by specificity, with most specific first.
	/// Priority:
	/// 1. Group-scoped artifacts over unscoped ones
	/// 2. Exact version matches (version_id set)
	/// 3. More specific ranges (range that allows_all of other matching ranges)
	/// 4. When ranges are incomparable, use pattern specificity: ^ > ~ > .x > others
	// spec: ART#what-a-version-offers
	fn sort_by_specificity(artifacts: &mut [Self]) {
		artifacts.sort_by(|a, b| {
			// An artifact scoped to the caller's group is more specific than one
			// belonging to no group. A deduplicating read resolves one scope, so
			// only one group's artifacts are ever in play here.
			let a_is_scoped = a.group_id.is_some();
			let b_is_scoped = b.group_id.is_some();

			if a_is_scoped != b_is_scoped {
				return if a_is_scoped {
					std::cmp::Ordering::Less
				} else {
					std::cmp::Ordering::Greater
				};
			}

			// Exact match always wins
			let a_is_exact = a.version_id.is_some();
			let b_is_exact = b.version_id.is_some();

			if a_is_exact && !b_is_exact {
				return std::cmp::Ordering::Less; // a is more specific
			}
			if !a_is_exact && b_is_exact {
				return std::cmp::Ordering::Greater; // b is more specific
			}

			// Both exact or both range: compare range specificity
			if !a_is_exact
				&& let (Some(pattern_a), Some(pattern_b)) =
					(&a.version_range_pattern, &b.version_range_pattern)
				&& let (Ok(range_a), Ok(range_b)) = (
					node_semver::Range::parse(pattern_a),
					node_semver::Range::parse(pattern_b),
				) {
				// If range_a allows_all of range_b, then range_b is more specific
				if range_a.allows_all(&range_b) && !range_b.allows_all(&range_a) {
					return std::cmp::Ordering::Greater; // b is more specific
				}
				// If range_b allows_all of range_a, then range_a is more specific
				if range_b.allows_all(&range_a) && !range_a.allows_all(&range_b) {
					return std::cmp::Ordering::Less; // a is more specific
				}
				return Self::compare_pattern_specificity(pattern_a, pattern_b);
			}

			// Can't determine specificity, maintain order
			std::cmp::Ordering::Equal
		});
	}

	/// Compare specificity of two range patterns when the ranges themselves are incomparable.
	/// Ranks patterns by explicitness: ^ (caret) > ~ (tilde) > .x (wildcard) > others
	fn compare_pattern_specificity(pattern_a: &str, pattern_b: &str) -> std::cmp::Ordering {
		fn pattern_rank(pattern: &str) -> u8 {
			if pattern.starts_with('^') {
				3 // Caret is most specific
			} else if pattern.starts_with('~') {
				2 // Tilde is more specific than wildcard
			} else if pattern.ends_with(".x") {
				1 // Wildcard .x
			} else {
				0 // Other patterns (least specific)
			}
		}

		pattern_rank(pattern_b).cmp(&pattern_rank(pattern_a))
	}

	/// When any artifact a build reads was last registered for each of these
	/// versions, in one query however many versions are asked about.
	///
	/// A schema built from a superseded release of a version is not the schema
	/// that version describes, so this is what a build is held against. Only
	/// the unscoped artifacts count: a group-scoped one is a build's own output,
	/// and registering it would put every group's pair for the version back on
	/// the worklist, including the pair that just produced it. A range artifact
	/// counts for every version it covers, since that is how one is resolved
	/// for a build.
	// spec: RPT#pairs
	pub async fn newest_change_for_versions(
		db: &mut AsyncPgConnection,
		versions: &[Version],
		ranges: &RangeChanges,
	) -> Result<std::collections::HashMap<Uuid, jiff::Timestamp>> {
		use crate::schema::artifacts::dsl;

		let ids: Vec<Uuid> = versions.iter().map(|v| v.id).collect();
		let exact: Vec<(Option<Uuid>, Option<jiff_diesel::Timestamp>)> = dsl::artifacts
			.filter(dsl::version_id.eq_any(&ids))
			.filter(dsl::group_id.is_null())
			.group_by(dsl::version_id)
			.select((dsl::version_id, diesel::dsl::max(dsl::updated_at)))
			.load(db)
			.await
			.map_err(AppError::from)?;

		let mut newest: std::collections::HashMap<Uuid, jiff::Timestamp> = exact
			.into_iter()
			.filter_map(|(id, at)| Some((id?, at?.into())))
			.collect();

		for (range, at) in &ranges.0 {
			for version in versions.iter().filter(|v| range.satisfies(&v.as_semver())) {
				newest
					.entry(version.id)
					.and_modify(|held| *held = (*held).max(*at))
					.or_insert(*at);
			}
		}

		Ok(newest)
	}

	/// The bytes Canopy holds for an artifact, where it holds any.
	pub async fn content_for(
		db: &mut AsyncPgConnection,
		artifact_id: Uuid,
		scope: Scope,
	) -> Result<Option<ArtifactContent>> {
		use crate::schema::artifacts::dsl::*;

		// The scope is part of the read rather than the caller's to remember:
		// the bytes of a group's artifact are the thing the boundary exists to
		// keep, and an id is guessable in a way a query is not.
		// spec: ART#who-is-offered-a-group-scoped-artifact
		let mut query = artifacts.filter(id.eq(artifact_id)).into_boxed();
		query = match scope {
			Scope::Unscoped => query.filter(group_id.is_null()),
			Scope::Group(caller) => query.filter(group_id.is_null().or(group_id.eq(caller))),
			Scope::Fleet => query,
		};

		let row: Option<(Option<Vec<u8>>, Option<String>, Option<Vec<u8>>)> = query
			.select((content, content_type, digest))
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)?;

		Ok(match row {
			Some((Some(bytes), media_type, Some(recorded))) => Some(ArtifactContent {
				bytes,
				content_type: media_type,
				digest: recorded,
			}),
			_ => None,
		})
	}

	/// Register an artifact, replacing whatever is already registered for the
	/// same version or range, type, platform, and group.
	// spec: ART#registration
	pub async fn register(db: &mut AsyncPgConnection, input: NewArtifact) -> Result<Self> {
		use crate::schema::artifacts::dsl::*;

		let input = input.resting()?;

		diesel::insert_into(artifacts)
			.values(&input)
			.on_conflict((
				artifact_type,
				platform,
				version_id,
				version_range_pattern,
				group_id,
			))
			.do_update()
			.set((
				download_url.eq(&input.download_url),
				device_id.eq(input.device_id),
				content.eq(&input.content),
				content_type.eq(&input.content_type),
				digest.eq(&input.digest),
				run_id.eq(input.run_id),
			))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(|error| match error {
				// A registration naming a group or version Canopy does not
				// hold is the caller's own input, so it is refused rather than
				// left to surface as a database fault.
				diesel::result::Error::DatabaseError(
					diesel::result::DatabaseErrorKind::ForeignKeyViolation,
					_,
				) => AppError::BadRequest(
					"the registration names a group or version Canopy does not hold".into(),
				),
				error => AppError::from(error),
			})
	}

	pub async fn update(
		db: &mut AsyncPgConnection,
		artifact_id: Uuid,
		new_type: String,
		new_platform: String,
		new_url: Option<String>,
	) -> Result<()> {
		use crate::schema::artifacts::dsl::*;

		// An artifact Canopy holds has no location to change. Replacing its
		// bytes is a registration, which is what carries the digest.
		// spec: ART#where-an-artifact-rests
		let (scoped, current_url): (Option<Uuid>, Option<String>) = artifacts
			.filter(id.eq(artifact_id))
			.select((group_id, download_url))
			.first(db)
			.await
			.map_err(AppError::from)?;
		let new_url = location(new_url);
		match (scoped.is_some(), new_url.is_some()) {
			(true, true) => {
				return Err(AppError::Conflict(
					"an artifact Canopy holds has no download URL".into(),
				));
			}
			(false, false) => {
				return Err(AppError::Conflict(
					"an artifact Canopy does not hold needs a download URL".into(),
				));
			}
			_ => {}
		}

		// A digest describes the bytes at a location, so it does not survive
		// the location changing: kept, it has every device that honours it
		// refuse a file that is the right one.
		// spec: ART#digests
		let moved = new_url != current_url;

		match diesel::update(artifacts.filter(id.eq(artifact_id)))
			.set((
				artifact_type.eq(new_type),
				platform.eq(new_platform),
				download_url.eq(new_url),
				moved.then_some(digest.eq(None::<Vec<u8>>)),
			))
			.execute(db)
			.await
		{
			Ok(_) => Ok(()),
			// A rename onto an identity another artifact already holds is the
			// operator's own input, so it is refused as a conflict rather than
			// left to surface as a database fault.
			Err(diesel::result::Error::DatabaseError(
				diesel::result::DatabaseErrorKind::UniqueViolation,
				_,
			)) => Err(AppError::Conflict(
				"an artifact of that type and platform is already registered".into(),
			)),
			Err(e) => Err(AppError::from(e)),
		}
	}

	pub async fn delete(db: &mut AsyncPgConnection, artifact_id: Uuid) -> Result<()> {
		use crate::schema::artifacts::dsl::*;

		diesel::delete(artifacts.filter(id.eq(artifact_id)))
			.execute(db)
			.await?;

		Ok(())
	}

	/// Get artifacts with metadata for a version, including all matches (not deduplicated).
	/// Also indicates which artifact is actually used in the public API.
	/// This is for private/admin views where you want to see all configured artifacts
	/// and understand which ones are actually being served.
	pub async fn get_for_version_all_matches_with_metadata(
		db: &mut AsyncPgConnection,
		target_version_id: Uuid,
		scope: Scope,
	) -> Result<Vec<(Self, bool, bool, bool)>> {
		let matching_artifacts =
			Self::get_for_version_all_matches(db, target_version_id, scope).await?;

		// An artifact is offered where it wins inside a scope that is actually
		// resolved, so the fleet-wide answer is the union over the unscoped read
		// and each group present rather than one deduplication across them all:
		// two groups' artifacts of one type and platform are both served.
		// spec: ART#what-a-version-offers
		let scopes: Vec<Scope> = match scope {
			Scope::Fleet => {
				let mut groups: Vec<Uuid> = matching_artifacts
					.iter()
					.filter_map(|a| a.group_id)
					.collect();
				groups.sort_unstable();
				groups.dedup();
				std::iter::once(Scope::Unscoped)
					.chain(groups.into_iter().map(Scope::Group))
					.collect()
			}
			resolved => vec![resolved],
		};

		let public_api_ids: std::collections::HashSet<Uuid> = scopes
			.into_iter()
			.flat_map(|scope| Self::offered_ids(&matching_artifacts, scope))
			.collect();

		// Only a range artifact can be the one an exact artifact displaces, and
		// a range only reached this set by matching this version, so what
		// displaces what is answerable from the set itself.
		let ranges: Vec<&Self> = matching_artifacts
			.iter()
			.filter(|a| a.version_range_pattern.is_some())
			.collect();

		let result = matching_artifacts
			.iter()
			.map(|a| {
				let is_exact = a.version_id == Some(target_version_id);
				let has_range_override = Self::overridden_range(&ranges, a);
				let is_used_in_public_api = public_api_ids.contains(&a.id);

				(
					a.clone(),
					is_exact,
					has_range_override,
					is_used_in_public_api,
				)
			})
			.collect();

		Ok(result)
	}

	/// Whether an exact artifact displaces a range artifact that also matches.
	///
	/// `ranges` are the range artifacts of the same match set, which are there
	/// only because they match this version already.
	fn overridden_range(ranges: &[&Self], artifact: &Self) -> bool {
		if artifact.version_id.is_none() {
			return false;
		}

		ranges.iter().any(|other| {
			other.artifact_type == artifact.artifact_type
				&& other.platform == artifact.platform
				// A range this artifact's own scope cannot see is not one it
				// displaces, and a group's range outranks an unscoped exact, so
				// what an exact artifact overrides is a range of its own group
				// or an unscoped one.
				// spec: ART#what-a-version-offers
				&& (other.group_id.is_none() || other.group_id == artifact.group_id)
		})
	}
}

/// When each unscoped range artifact last changed, with its pattern parsed.
///
/// A range covers versions rather than naming one, so which of them it answers
/// for is decided in memory. Loaded once and handed to each version it is asked
/// about: the patterns do not vary by group, and a worklist poll asks the same
/// question of every group it covers.
// spec: RPT#pairs
pub struct RangeChanges(Vec<(node_semver::Range, jiff::Timestamp)>);

impl RangeChanges {
	/// One row per distinct pattern rather than per artifact: the answer only
	/// needs the newest change under each, and every row returned costs a
	/// semver parse.
	pub async fn load(db: &mut AsyncPgConnection) -> Result<Self> {
		use crate::schema::artifacts::dsl;

		let rows: Vec<(Option<String>, Option<jiff_diesel::Timestamp>)> = dsl::artifacts
			.filter(dsl::version_id.is_null())
			.filter(dsl::group_id.is_null())
			.group_by(dsl::version_range_pattern)
			.select((
				dsl::version_range_pattern,
				diesel::dsl::max(dsl::updated_at),
			))
			.load(db)
			.await
			.map_err(AppError::from)?;

		Ok(Self(
			rows.into_iter()
				.filter_map(|(pattern, at)| {
					// An unparseable pattern matches nothing rather than
					// everything, as it does where the artifact is offered.
					let range = node_semver::Range::parse(pattern?).ok()?;
					Some((range, at?.into()))
				})
				.collect(),
		))
	}
}
