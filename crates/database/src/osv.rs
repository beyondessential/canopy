//! OSV advisories: the record as OSV publishes it, the severity Canopy ranks it
//! by, and whether a revision of its affected ranges reaches a package version.
//!
//! Spec: `.workhorse/specs/security/vulnerabilities.md` (id `VUL`) and
//! `.workhorse/specs/security/vulnerability-triage.md` (id `VTR`).
//!
//! OSV does the matching: it says which advisories hit a package version.
//! Evaluating `affected` here is only for deciding whether a revision of an
//! advisory newly reaches something the source carries, which is what lapses a
//! decision.

use std::{cmp::Ordering, str::FromStr};

use serde::{Deserialize, Serialize};

/// The qualitative rating an advisory carries, lowest first.
#[derive(
	Clone,
	Copy,
	Debug,
	Default,
	PartialEq,
	Eq,
	PartialOrd,
	Ord,
	Hash,
	Serialize,
	Deserialize,
	utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
#[schema(as = VulnerabilitySeverity)]
pub enum Severity {
	/// No rating from CVSS or from the advisory's database. Ranks below low.
	#[default]
	Unrated,
	Low,
	Medium,
	High,
	Critical,
}

impl Severity {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Unrated => "unrated",
			Self::Low => "low",
			Self::Medium => "medium",
			Self::High => "high",
			Self::Critical => "critical",
		}
	}

	/// A database's own rating word: GHSA's `MODERATE`, Ubuntu's
	/// `negligible`, and so on.
	pub fn from_rating(word: &str) -> Option<Self> {
		match word.trim().to_ascii_lowercase().as_str() {
			"critical" => Some(Self::Critical),
			"high" | "important" => Some(Self::High),
			"medium" | "moderate" => Some(Self::Medium),
			"low" | "negligible" | "minor" => Some(Self::Low),
			_ => None,
		}
	}

	/// The rating a CVSS vector scores to.
	pub fn from_cvss(vector: &str) -> Option<Self> {
		let vector: cvss::Cvss = vector.trim().parse().ok()?;
		match vector.severity() {
			cvss::Severity::Critical => Some(Self::Critical),
			cvss::Severity::High => Some(Self::High),
			cvss::Severity::Medium => Some(Self::Medium),
			cvss::Severity::Low => Some(Self::Low),
			cvss::Severity::None => None,
		}
	}
}

impl FromStr for Severity {
	type Err = ();

	fn from_str(s: &str) -> Result<Self, ()> {
		match s {
			"unrated" => Ok(Self::Unrated),
			other => Self::from_rating(other).ok_or(()),
		}
	}
}

/// One OSV record, as much of it as Canopy reads.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Record {
	pub id: String,
	#[serde(default)]
	pub aliases: Vec<String>,
	#[serde(default)]
	pub summary: String,
	#[serde(default)]
	pub details: String,
	pub modified: String,
	#[serde(default)]
	pub published: Option<String>,
	#[serde(default)]
	pub severity: Vec<SeverityEntry>,
	#[serde(default)]
	pub affected: Vec<Affected>,
	#[serde(default)]
	pub references: Vec<Reference>,
	#[serde(default)]
	pub database_specific: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeverityEntry {
	pub r#type: String,
	pub score: String,
}

/// A link an advisory gives: an article, a report, a fix.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = OsvReference)]
pub struct Reference {
	/// What kind of link OSV calls it: `ADVISORY`, `ARTICLE`, `REPORT`, `FIX`, `WEB` and so on.
	pub r#type: String,
	/// Where it points.
	pub url: String,
}

/// A package an advisory affects, with the versions and ranges affected.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = OsvAffected)]
pub struct Affected {
	/// The package affected, absent where the advisory names none.
	#[serde(default)]
	pub package: Option<Package>,
	/// The ranges of affected versions.
	#[serde(default)]
	pub ranges: Vec<Range>,
	/// Affected versions listed one by one, alongside the ranges.
	#[serde(default)]
	pub versions: Vec<String>,
	/// Whatever the package's ecosystem adds about it, such as a severity of its own.
	#[serde(default)]
	#[schema(value_type = Object)]
	pub ecosystem_specific: serde_json::Value,
	/// Whatever the advisory's database adds about it, such as a severity of its own.
	#[serde(default)]
	#[schema(value_type = Object)]
	pub database_specific: serde_json::Value,
}

/// A package, named within its ecosystem.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = OsvPackage)]
pub struct Package {
	/// The ecosystem it belongs to, such as `npm` or `Ubuntu:24.04:LTS`.
	pub ecosystem: String,
	/// Its name within the ecosystem.
	pub name: String,
}

/// A range of affected versions, as events in version order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = OsvRange)]
pub struct Range {
	/// How its versions order: `SEMVER`, `ECOSYSTEM` for the ecosystem's own order, or `GIT`.
	pub r#type: String,
	/// The points where versions enter and leave the range.
	#[serde(default)]
	pub events: Vec<Event>,
}

/// One event of a range. OSV holds exactly one of the four per event.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = OsvEvent)]
pub struct Event {
	/// The version the flaw is introduced in; "0" for every version before the next event.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub introduced: Option<String>,
	/// The first version the flaw is fixed in.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub fixed: Option<String>,
	/// The last version the flaw is in; everything after it is unaffected.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub last_affected: Option<String>,
	/// A version the range ends before, whatever events come after.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub limit: Option<String>,
}

impl Record {
	/// The advisory's severity: its CVSS score where it has one, otherwise the
	/// rating its database gives, otherwise unrated.
	pub fn severity(&self) -> Severity {
		let cvss = self
			.severity
			.iter()
			.filter(|entry| entry.r#type.to_ascii_uppercase().starts_with("CVSS"))
			.filter_map(|entry| Severity::from_cvss(&entry.score))
			.max();
		if let Some(severity) = cvss {
			return severity;
		}

		let own_rating = self
			.severity
			.iter()
			.filter(|entry| !entry.r#type.to_ascii_uppercase().starts_with("CVSS"))
			.filter_map(|entry| Severity::from_rating(&entry.score))
			.max();
		let database_specific = self
			.database_specific
			.get("severity")
			.and_then(|rating| rating.as_str())
			.and_then(Severity::from_rating);
		let per_package = self
			.affected
			.iter()
			.filter_map(|affected| {
				affected
					.ecosystem_specific
					.get("severity")
					.or_else(|| affected.database_specific.get("severity"))
					.and_then(|rating| rating.as_str())
					.and_then(Severity::from_rating)
			})
			.max();
		own_rating
			.into_iter()
			.chain(database_specific)
			.chain(per_package)
			.max()
			.unwrap_or_default()
	}

	/// The versions of `name` in `ecosystem` that fix this advisory, from the
	/// `fixed` events of its ranges.
	pub fn fixed_in(&self, ecosystem: &str, name: &str) -> Vec<String> {
		let mut fixed: Vec<String> = self
			.affected
			.iter()
			.filter(|affected| names(affected, ecosystem, name))
			.flat_map(|affected| &affected.ranges)
			.flat_map(|range| &range.events)
			.filter_map(|event| event.fixed.clone())
			.collect();
		let kind = VersionKind::of(ecosystem);
		fixed.sort_by(|a, b| {
			kind.compare(a, b)
				.unwrap_or_else(|| match (kind.parses(a), kind.parses(b)) {
					(true, false) => Ordering::Less,
					(false, true) => Ordering::Greater,
					_ => a.cmp(b),
				})
		});
		fixed.dedup();
		fixed
	}
}

/// The lowest version above `version` that fixes `name`, of the `fixed`
/// events of the affected ranges: where to move a package at `version` to.
pub fn min_fix_version(
	affected: &[Affected],
	ecosystem: &str,
	name: &str,
	version: &str,
) -> Option<String> {
	let kind = VersionKind::of(ecosystem);
	affected
		.iter()
		.filter(|affected| names(affected, ecosystem, name))
		.flat_map(|affected| &affected.ranges)
		.filter(|range| matches!(range.r#type.as_str(), "SEMVER" | "ECOSYSTEM"))
		.flat_map(|range| &range.events)
		.filter_map(|event| event.fixed.as_deref())
		.filter(|fixed| kind.compare(fixed, version) == Some(Ordering::Greater))
		.min_by(|a, b| kind.compare(a, b).unwrap_or(Ordering::Equal))
		.map(str::to_owned)
}

fn names(affected: &Affected, ecosystem: &str, name: &str) -> bool {
	affected
		.package
		.as_ref()
		.is_some_and(|package| package.ecosystem == ecosystem && package.name == name)
}

/// Whether `affected` reaches `name` at `version` in `ecosystem`, by OSV's
/// semantics: an explicit `versions` list, or any `SEMVER` or `ECOSYSTEM`
/// range whose events put the version inside it.
pub fn reaches(affected: &[Affected], ecosystem: &str, name: &str, version: &str) -> bool {
	let kind = VersionKind::of(ecosystem);
	affected
		.iter()
		.filter(|affected| names(affected, ecosystem, name))
		.any(|affected| {
			affected.versions.iter().any(|listed| listed == version)
				|| affected.ranges.iter().any(|range| {
					let kind = match range.r#type.as_str() {
						"SEMVER" => VersionKind::Semver,
						"ECOSYSTEM" => kind,
						_ => return false,
					};
					in_range(&range.events, version, kind)
				})
		})
}

/// Whether the advisory's own ranges and listed versions say `name` at
/// `version` is not affected. OSV does the matching, so this only withholds a
/// match the record contradicts; it says nothing where it cannot tell: no
/// entry names the package, an entry has no range or list to read, a range is
/// of a kind or holds an event this does not evaluate, or the version does
/// not parse.
pub fn rules_out(affected: &[Affected], ecosystem: &str, name: &str, version: &str) -> bool {
	let kind = VersionKind::of(ecosystem);
	if !kind.parses(version) {
		return false;
	}
	let mut named = affected
		.iter()
		.filter(|affected| names(affected, ecosystem, name))
		.peekable();
	if named.peek().is_none() {
		return false;
	}
	let readable = named.all(|affected| {
		(!affected.ranges.is_empty() || !affected.versions.is_empty())
			&& affected.ranges.iter().all(|range| {
				let kind = match range.r#type.as_str() {
					"SEMVER" => VersionKind::Semver,
					"ECOSYSTEM" => kind,
					_ => return false,
				};
				range.events.iter().all(|event| {
					let at = event
						.introduced
						.as_deref()
						.or(event.fixed.as_deref())
						.or(event.last_affected.as_deref())
						.or(event.limit.as_deref());
					at.is_some_and(|at| {
						(event.introduced.is_some() && at == "0") || kind.parses(at)
					})
				})
			})
	});
	readable && !reaches(affected, ecosystem, name, version)
}

/// How versions of an ecosystem order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VersionKind {
	Semver,
	Dpkg,
	Pep440,
}

impl VersionKind {
	fn of(ecosystem: &str) -> Self {
		if ecosystem.starts_with("Ubuntu") || ecosystem.starts_with("Debian") {
			Self::Dpkg
		} else if ecosystem == "PyPI" {
			Self::Pep440
		} else {
			Self::Semver
		}
	}

	fn parses(self, version: &str) -> bool {
		match self {
			Self::Dpkg => true,
			Self::Semver => semver(version).is_some(),
			Self::Pep440 => version.parse::<pep440_rs::Version>().is_ok(),
		}
	}

	fn compare(self, a: &str, b: &str) -> Option<Ordering> {
		match self {
			Self::Dpkg => Some(dpkg_compare(a, b)),
			Self::Semver => Some(semver(a)?.cmp(&semver(b)?)),
			Self::Pep440 => {
				let a = a.parse::<pep440_rs::Version>().ok()?;
				let b = b.parse::<pep440_rs::Version>().ok()?;
				Some(a.cmp(&b))
			}
		}
	}
}

/// A semantic version, reading one of fewer than three numbers as the rest
/// being zero: advisories name `0.65` for `0.65.0`, and a version that did not
/// parse would never close the range it ends.
fn semver(version: &str) -> Option<node_semver::Version> {
	if let Ok(parsed) = node_semver::Version::parse(version) {
		return Some(parsed);
	}
	let (core, rest) = version.split_at(version.find(['-', '+']).unwrap_or(version.len()));
	let numbers = core.split('.').count();
	if numbers >= 3
		|| core
			.split('.')
			.any(|n| n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()))
	{
		return None;
	}
	let padded = format!("{core}{}{rest}", ".0".repeat(3 - numbers));
	node_semver::Version::parse(padded).ok()
}

/// OSV's range evaluation: events in version order, `introduced: "0"` first,
/// each one moving the version in or out of the range.
fn in_range(events: &[Event], version: &str, kind: VersionKind) -> bool {
	struct Step<'a> {
		at: &'a str,
		event: &'a Event,
	}

	let mut steps: Vec<Step<'_>> = events
		.iter()
		.filter_map(|event| {
			let at = event
				.introduced
				.as_deref()
				.or(event.fixed.as_deref())
				.or(event.last_affected.as_deref())
				.or(event.limit.as_deref())?;
			Some(Step { at, event })
		})
		.collect();
	steps.sort_by(|a, b| {
		let zero = |step: &Step<'_>| step.event.introduced.as_deref() == Some("0");
		match (zero(a), zero(b)) {
			(true, false) => Ordering::Less,
			(false, true) => Ordering::Greater,
			(true, true) => Ordering::Equal,
			// Versions that do not parse go last, in a fixed order, so the
			// order stays total and the ones that parse stay in version order.
			(false, false) => kind.compare(a.at, b.at).unwrap_or_else(|| {
				match (kind.parses(a.at), kind.parses(b.at)) {
					(true, false) => Ordering::Less,
					(false, true) => Ordering::Greater,
					_ => a.at.cmp(b.at),
				}
			}),
		}
	});

	let mut inside = false;
	for step in steps {
		let event = step.event;
		if let Some(introduced) = &event.introduced {
			if introduced == "0"
				|| kind
					.compare(version, introduced)
					.is_some_and(|ord| ord != Ordering::Less)
			{
				inside = true;
			}
		}
		if let Some(fixed) = &event.fixed
			&& kind
				.compare(version, fixed)
				.is_some_and(|ord| ord != Ordering::Less)
		{
			inside = false;
		}
		if let Some(last) = &event.last_affected
			&& kind
				.compare(version, last)
				.is_some_and(|ord| ord == Ordering::Greater)
		{
			inside = false;
		}
		if let Some(limit) = &event.limit
			&& kind
				.compare(version, limit)
				.is_some_and(|ord| ord != Ordering::Less)
		{
			return false;
		}
	}
	inside
}

/// Compares two Debian package versions, `[epoch:]upstream[-revision]`, by
/// the dpkg algorithm: the epoch numerically, then the upstream version and
/// the revision each as alternating non-digit and digit runs, where `~` sorts
/// before everything, even the end of the string.
pub fn dpkg_compare(a: &str, b: &str) -> Ordering {
	let (epoch_a, rest_a) = split_epoch(a);
	let (epoch_b, rest_b) = split_epoch(b);
	epoch_a.cmp(&epoch_b).then_with(|| {
		let (upstream_a, revision_a) = split_revision(rest_a);
		let (upstream_b, revision_b) = split_revision(rest_b);
		compare_part(upstream_a, upstream_b).then_with(|| compare_part(revision_a, revision_b))
	})
}

fn split_epoch(version: &str) -> (u64, &str) {
	match version.split_once(':') {
		Some((epoch, rest)) if epoch.bytes().all(|b| b.is_ascii_digit()) && !epoch.is_empty() => {
			(epoch.parse().unwrap_or(0), rest)
		}
		_ => (0, version),
	}
}

fn split_revision(version: &str) -> (&str, &str) {
	version.rsplit_once('-').unwrap_or((version, ""))
}

fn order(c: Option<u8>) -> i32 {
	match c {
		None => 0,
		Some(b'~') => -1,
		Some(c) if c.is_ascii_alphabetic() => i32::from(c),
		Some(c) => i32::from(c) + 256,
	}
}

fn compare_part(a: &str, b: &str) -> Ordering {
	let (mut a, mut b) = (a.as_bytes(), b.as_bytes());
	while !a.is_empty() || !b.is_empty() {
		let mut first_diff = Ordering::Equal;
		while a.first().is_some_and(|c| !c.is_ascii_digit())
			|| b.first().is_some_and(|c| !c.is_ascii_digit())
		{
			let ac = order(a.first().copied().filter(|c| !c.is_ascii_digit()));
			let bc = order(b.first().copied().filter(|c| !c.is_ascii_digit()));
			if ac != bc {
				return ac.cmp(&bc);
			}
			if a.first().is_some_and(|c| !c.is_ascii_digit()) {
				a = &a[1..];
			}
			if b.first().is_some_and(|c| !c.is_ascii_digit()) {
				b = &b[1..];
			}
		}
		while a.first() == Some(&b'0') {
			a = &a[1..];
		}
		while b.first() == Some(&b'0') {
			b = &b[1..];
		}
		while a.first().is_some_and(u8::is_ascii_digit) && b.first().is_some_and(u8::is_ascii_digit)
		{
			if first_diff == Ordering::Equal {
				first_diff = a[0].cmp(&b[0]);
			}
			a = &a[1..];
			b = &b[1..];
		}
		if a.first().is_some_and(u8::is_ascii_digit) {
			return Ordering::Greater;
		}
		if b.first().is_some_and(u8::is_ascii_digit) {
			return Ordering::Less;
		}
		if first_diff != Ordering::Equal {
			return first_diff;
		}
	}
	Ordering::Equal
}

/// What a decision remembers of an advisory, to tell later whether a revision
/// of it changes anything about exposure.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Held {
	pub severity: Severity,
	pub affected: Vec<Affected>,
}

/// A package version the fleet counts: an ecosystem, a package, a version.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Counted {
	pub ecosystem: String,
	pub name: String,
	pub version: String,
}

impl Held {
	/// Why a revision lapses a decision this was held for, or `None` when it
	/// does not: its severity rose, or a counted package version is inside the
	/// revised ranges that was outside the held ones.
	pub fn lapsed_by(
		&self,
		now_severity: Severity,
		now_affected: &[Affected],
		counted: &[Counted],
	) -> Option<LapseReason> {
		if now_severity > self.severity {
			return Some(LapseReason::SeverityRose {
				from: self.severity,
				to: now_severity,
			});
		}
		// Unchanged ranges reach what they reached, so there is nothing to walk.
		if now_affected == self.affected.as_slice() {
			return None;
		}
		let named = |c: &&Counted| {
			now_affected
				.iter()
				.any(|affected| names(affected, &c.ecosystem, &c.name))
		};
		let reached = counted.iter().filter(named).find(|c| {
			reaches(now_affected, &c.ecosystem, &c.name, &c.version)
				&& !reaches(&self.affected, &c.ecosystem, &c.name, &c.version)
		})?;
		Some(LapseReason::RangeWidened {
			package: reached.name.clone(),
			version: reached.version.clone(),
		})
	}
}

/// Why a decision stopped standing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LapseReason {
	/// An accepted risk's review date passed. No longer made, and kept so
	/// that lapses recorded when it was still read.
	ReviewDue,
	/// OSV raised the advisory's severity.
	SeverityRose { from: Severity, to: Severity },
	/// OSV widened the advisory's ranges to reach a package version that counts.
	RangeWidened { package: String, version: String },
	/// A workspace package came to reach the advisory that the decision did
	/// not cover.
	NewWorkspacePackage { name: String },
}

impl LapseReason {
	pub fn describe(&self) -> String {
		match self {
			Self::ReviewDue => "Review date passed".into(),
			Self::SeverityRose { from, to } => {
				format!("Severity rose from {} to {}", from.as_str(), to.as_str())
			}
			Self::RangeWidened { package, version } => {
				format!("Affected range now reaches {package} {version}")
			}
			Self::NewWorkspacePackage { name } => {
				format!("{name} now reaches the advisory and the decision did not cover it")
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn semver_range(events: &[(&str, &str)]) -> Vec<Affected> {
		vec![Affected {
			package: Some(Package {
				ecosystem: "npm".into(),
				name: "left-pad".into(),
			}),
			ranges: vec![Range {
				r#type: "SEMVER".into(),
				events: events
					.iter()
					.map(|(kind, at)| {
						let mut event = Event::default();
						let at = Some((*at).to_owned());
						match *kind {
							"introduced" => event.introduced = at,
							"fixed" => event.fixed = at,
							"last_affected" => event.last_affected = at,
							_ => event.limit = at,
						}
						event
					})
					.collect(),
			}],
			..Default::default()
		}]
	}

	fn hits(affected: &[Affected], version: &str) -> bool {
		reaches(affected, "npm", "left-pad", version)
	}

	#[test]
	fn an_unparseable_event_leaves_the_others_in_version_order() {
		let affected = semver_range(&[
			("fixed", "1.2.5"),
			("introduced", "not-a-version"),
			("introduced", "1.2.0"),
		]);
		assert!(!hits(&affected, "1.1.9"));
		assert!(hits(&affected, "1.2.4"));
		assert!(!hits(&affected, "1.2.5"));
	}

	#[test]
	fn fixed_versions_are_listed_in_version_order() {
		let record = Record {
			affected: semver_range(&[
				("introduced", "0"),
				("fixed", "1.10.0"),
				("introduced", "2.0.0"),
				("fixed", "1.9.0"),
			]),
			..Default::default()
		};
		assert_eq!(record.fixed_in("npm", "left-pad"), ["1.9.0", "1.10.0"]);
	}

	#[test]
	fn a_version_short_of_three_numbers_is_read_with_zeros() {
		let affected = semver_range(&[("introduced", "0"), ("fixed", "0.65")]);
		assert!(hits(&affected, "0.43.0"));
		assert!(hits(&affected, "0.64.1"));
		assert!(!hits(&affected, "0.65.0"));
		assert!(!hits(&affected, "0.70.0"));
		assert_eq!(
			min_fix_version(&affected, "npm", "left-pad", "0.43.0").as_deref(),
			Some("0.65")
		);
	}

	#[test]
	fn a_match_is_ruled_out_only_where_the_ranges_say_so() {
		let affected = semver_range(&[("introduced", "1.2.0"), ("fixed", "1.2.5")]);
		let rules_out = |version| super::rules_out(&affected, "npm", "left-pad", version);
		assert!(rules_out("1.1.9"));
		assert!(rules_out("1.2.5"));
		assert!(rules_out("2.0.0"));
		assert!(!rules_out("1.2.4"));
		assert!(!rules_out("not-a-version"));
		assert!(!super::rules_out(&affected, "npm", "right-pad", "9.9.9"));
	}

	#[test]
	fn a_match_is_never_ruled_out_on_ranges_that_are_not_read() {
		let mut affected = semver_range(&[("introduced", "1.2.0"), ("fixed", "1.2.5")]);
		affected[0].ranges[0].r#type = "GIT".into();
		assert!(!super::rules_out(&affected, "npm", "left-pad", "9.9.9"));

		let affected = semver_range(&[("introduced", "0"), ("fixed", "not-a-version")]);
		assert!(!super::rules_out(&affected, "npm", "left-pad", "9.9.9"));

		let mut bare = semver_range(&[]);
		bare[0].ranges.clear();
		assert!(!super::rules_out(&bare, "npm", "left-pad", "9.9.9"));
	}

	#[test]
	fn semver_introduced_and_fixed() {
		let affected = semver_range(&[("introduced", "1.2.0"), ("fixed", "1.2.5")]);
		assert!(!hits(&affected, "1.1.9"));
		assert!(hits(&affected, "1.2.0"));
		assert!(hits(&affected, "1.2.4"));
		assert!(!hits(&affected, "1.2.5"));
		assert!(!hits(&affected, "2.0.0"));
	}

	#[test]
	fn semver_from_zero_and_last_affected() {
		let affected = semver_range(&[("introduced", "0"), ("last_affected", "3.0.1")]);
		assert!(hits(&affected, "0.0.1"));
		assert!(hits(&affected, "3.0.1"));
		assert!(!hits(&affected, "3.0.2"));
	}

	#[test]
	fn semver_several_intervals_and_unsorted_events() {
		let affected = semver_range(&[
			("fixed", "2.1.0"),
			("introduced", "0"),
			("introduced", "3.0.0"),
			("fixed", "3.2.0"),
		]);
		assert!(hits(&affected, "2.0.0"));
		assert!(!hits(&affected, "2.5.0"));
		assert!(hits(&affected, "3.1.0"));
		assert!(!hits(&affected, "3.2.0"));
	}

	#[test]
	fn semver_limit_closes_the_range() {
		let affected = semver_range(&[("introduced", "1.0.0"), ("limit", "2.0.0")]);
		assert!(hits(&affected, "1.5.0"));
		assert!(!hits(&affected, "2.0.0"));
	}

	#[test]
	fn explicit_versions_match_exactly() {
		let affected = vec![Affected {
			package: Some(Package {
				ecosystem: "npm".into(),
				name: "left-pad".into(),
			}),
			versions: vec!["1.0.0".into(), "1.0.2".into()],
			..Default::default()
		}];
		assert!(hits(&affected, "1.0.0"));
		assert!(!hits(&affected, "1.0.1"));
		assert!(hits(&affected, "1.0.2"));
	}

	#[test]
	fn another_package_or_ecosystem_is_not_reached() {
		let affected = semver_range(&[("introduced", "0")]);
		assert!(!reaches(&affected, "npm", "right-pad", "1.0.0"));
		assert!(!reaches(&affected, "Ubuntu:24.04:LTS", "left-pad", "1.0.0"));
	}

	#[test]
	fn ubuntu_ecosystem_ranges_order_as_dpkg() {
		let affected = vec![Affected {
			package: Some(Package {
				ecosystem: "Ubuntu:24.04:LTS".into(),
				name: "openssl".into(),
			}),
			ranges: vec![Range {
				r#type: "ECOSYSTEM".into(),
				events: vec![
					Event {
						introduced: Some("0".into()),
						..Default::default()
					},
					Event {
						fixed: Some("3.0.13-0ubuntu3.5".into()),
						..Default::default()
					},
				],
			}],
			..Default::default()
		}];
		let at = |v: &str| reaches(&affected, "Ubuntu:24.04:LTS", "openssl", v);
		assert!(at("3.0.13-0ubuntu3.4"));
		assert!(at("3.0.13-0ubuntu3.5~pre1"));
		assert!(!at("3.0.13-0ubuntu3.5"));
		assert!(!at("1:3.0.13-0ubuntu3.5"));
		assert!(!at("3.0.14-0ubuntu1"));
	}

	#[test]
	fn dpkg_ordering() {
		use Ordering::*;
		let cases = [
			("1.0", "1.0", Equal),
			("1.0", "1.1", Less),
			("1.0~rc1", "1.0", Less),
			("1.0~rc1", "1.0~rc2", Less),
			("1.0", "1.0+b1", Less),
			("1.0a", "1.0", Greater),
			("1:0.1", "2.0", Greater),
			("0:1.0", "1.0", Equal),
			("1.10", "1.9", Greater),
			("1.0-1", "1.0-2", Less),
			("1.0-1ubuntu1", "1.0-1", Greater),
			("1.0-0ubuntu0.20.04.1", "1.0-0ubuntu0.20.04.10", Less),
			("2.4.52-1ubuntu4.9", "2.4.52-1ubuntu4.10", Less),
			("1.0.0", "1.0", Greater),
			("1.0-01", "1.0-1", Equal),
		];
		for (a, b, expected) in cases {
			assert_eq!(dpkg_compare(a, b), expected, "{a} vs {b}");
			assert_eq!(dpkg_compare(b, a), expected.reverse(), "{b} vs {a}");
		}
	}

	#[test]
	fn severity_prefers_cvss_over_the_database_rating() {
		let record = Record {
			id: "GHSA-test".into(),
			modified: "2026-01-01T00:00:00Z".into(),
			severity: vec![SeverityEntry {
				r#type: "CVSS_V3".into(),
				score: "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H".into(),
			}],
			database_specific: serde_json::json!({"severity": "LOW"}),
			..Default::default()
		};
		assert_eq!(record.severity(), Severity::Critical);
	}

	#[test]
	fn severity_falls_back_to_the_database_rating() {
		let record = Record {
			id: "GHSA-test".into(),
			modified: "2026-01-01T00:00:00Z".into(),
			database_specific: serde_json::json!({"severity": "MODERATE"}),
			..Default::default()
		};
		assert_eq!(record.severity(), Severity::Medium);

		let ubuntu = Record {
			id: "UBUNTU-CVE-2026-1".into(),
			modified: "2026-01-01T00:00:00Z".into(),
			severity: vec![SeverityEntry {
				r#type: "Ubuntu".into(),
				score: "high".into(),
			}],
			..Default::default()
		};
		assert_eq!(ubuntu.severity(), Severity::High);
	}

	#[test]
	fn unrated_ranks_below_low() {
		let record = Record {
			id: "X".into(),
			modified: "2026-01-01T00:00:00Z".into(),
			..Default::default()
		};
		assert_eq!(record.severity(), Severity::Unrated);
		assert!(Severity::Unrated < Severity::Low);
	}

	#[test]
	fn a_revision_raising_severity_lapses() {
		let held = Held {
			severity: Severity::Medium,
			affected: semver_range(&[("introduced", "0"), ("fixed", "2.0.0")]),
		};
		let affected = held.affected.clone();
		assert!(held.lapsed_by(Severity::Medium, &affected, &[]).is_none());
		assert!(matches!(
			held.lapsed_by(Severity::High, &affected, &[]),
			Some(LapseReason::SeverityRose { .. })
		));
	}

	fn counted(version: &str) -> Counted {
		Counted {
			ecosystem: "npm".into(),
			name: "left-pad".into(),
			version: version.into(),
		}
	}

	#[test]
	fn a_revision_widening_the_range_to_a_counted_version_lapses() {
		let held = Held {
			severity: Severity::High,
			affected: semver_range(&[("introduced", "0"), ("fixed", "1.5.0")]),
		};
		let widened = semver_range(&[("introduced", "0"), ("fixed", "2.0.0")]);
		let lapse = held.lapsed_by(Severity::High, &widened, &[counted("1.8.0")]);
		assert!(matches!(lapse, Some(LapseReason::RangeWidened { .. })));
		assert!(
			held.lapsed_by(Severity::High, &widened, &[counted("2.5.0")])
				.is_none()
		);
	}

	#[test]
	fn a_revision_only_adding_a_fixed_version_leaves_the_decision() {
		let held = Held {
			severity: Severity::High,
			affected: semver_range(&[("introduced", "0")]),
		};
		let fixed = semver_range(&[("introduced", "0"), ("fixed", "3.0.0")]);
		assert!(
			held.lapsed_by(
				Severity::High,
				&fixed,
				&[counted("1.0.0"), counted("2.0.0")]
			)
			.is_none()
		);
	}

	#[test]
	fn a_counted_version_already_inside_the_held_ranges_does_not_lapse() {
		let held = Held {
			severity: Severity::High,
			affected: semver_range(&[("introduced", "0"), ("fixed", "2.0.0")]),
		};
		let affected = held.affected.clone();
		assert!(
			held.lapsed_by(Severity::High, &affected, &[counted("1.9.9")])
				.is_none()
		);
	}
}
