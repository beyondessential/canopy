//! The subset of Ansible host patterns a run's limit is evaluated as: what
//! names hosts and groups, so Canopy can say which machines a limit matches
//! before a run starts.
//!
//! Patterns are separated by `:` or `,`. Each names a host or group, or a glob
//! of them with `*` and `?`, and may be prefixed `&` to intersect with what has
//! been chosen so far, or `!` to exclude from it. Anything else Ansible reads in
//! a pattern, a `~` regular expression or a `[0:2]` subscript, is refused rather
//! than evaluated wrongly.
// spec: ANSRUN#presentation

use std::collections::{BTreeMap, BTreeSet};

/// A machine as a limit sees it.
pub struct Member<'a> {
	pub id: uuid::Uuid,
	/// The name a run addresses it by.
	pub host_name: &'a str,
	/// The Ansible groups a run puts it in.
	pub groups: &'a BTreeSet<String>,
}

/// Why a limit cannot be evaluated.
#[derive(Debug, PartialEq, Eq)]
pub struct Unsupported(pub String);

/// The machines `pattern` matches, in the order given.
pub fn matching(pattern: &str, members: &[Member<'_>]) -> Result<Vec<uuid::Uuid>, Unsupported> {
	let mut chosen: Option<BTreeSet<usize>> = None;
	let mut intersect: Vec<BTreeSet<usize>> = Vec::new();
	let mut exclude: Vec<BTreeSet<usize>> = Vec::new();

	let mut any = false;
	for token in pattern.split([':', ',']) {
		let token = token.trim();
		if token.is_empty() {
			continue;
		}
		any = true;
		if token.starts_with('~') || token.contains('[') {
			return Err(Unsupported(format!(
				"{token:?} is a pattern Canopy cannot evaluate; name hosts and groups, or use * and ?"
			)));
		}
		let (kind, name) = match token.as_bytes()[0] {
			b'&' => ('&', &token[1..]),
			b'!' => ('!', &token[1..]),
			_ => ('+', token),
		};
		if name.is_empty() {
			return Err(Unsupported(format!("{token:?} names nothing")));
		}
		let found = named(name, members);
		match kind {
			'&' => intersect.push(found),
			'!' => exclude.push(found),
			_ => chosen.get_or_insert_with(BTreeSet::new).extend(found),
		}
	}
	if !any {
		return Err(Unsupported("the limit is empty".into()));
	}

	// Ansible applies every plain pattern, then every `&`, then every `!`,
	// whatever order they are written in (`order_patterns` in its inventory
	// manager), so an exclusion written before a union still excludes. With
	// nothing chosen to begin from, a limit of only exclusions is every machine
	// less those.
	let mut result: BTreeSet<usize> = chosen.unwrap_or_else(|| (0..members.len()).collect());
	for other in &intersect {
		result = result.intersection(other).copied().collect();
	}
	for other in &exclude {
		result = result.difference(other).copied().collect();
	}
	Ok(result.into_iter().map(|index| members[index].id).collect())
}

/// The members a host or group name, or a glob of them, reaches.
fn named(name: &str, members: &[Member<'_>]) -> BTreeSet<usize> {
	if name == "all" {
		return (0..members.len()).collect();
	}
	members
		.iter()
		.enumerate()
		.filter(|(_, member)| {
			matches(name, member.host_name)
				|| member.groups.iter().any(|group| matches(name, group))
		})
		.map(|(index, _)| index)
		.collect()
}

/// Whether `text` matches `glob`, where `*` is any run of characters and `?` any
/// one. Case matters, as it does to Ansible.
///
/// Remembers only where the last `*` was, so the work is bounded by the lengths
/// of the two rather than by how many stars the pattern has.
fn matches(glob: &str, text: &str) -> bool {
	let glob: Vec<char> = glob.chars().collect();
	let text: Vec<char> = text.chars().collect();
	let (mut g, mut t) = (0, 0);
	let mut star: Option<(usize, usize)> = None;
	while t < text.len() {
		match glob.get(g) {
			Some('*') => {
				star = Some((g, t));
				g += 1;
			}
			Some('?') => {
				g += 1;
				t += 1;
			}
			Some(c) if *c == text[t] => {
				g += 1;
				t += 1;
			}
			_ => match star {
				Some((star_g, star_t)) => {
					g = star_g + 1;
					t = star_t + 1;
					star = Some((star_g, star_t + 1));
				}
				None => return false,
			},
		}
	}
	glob[g..].iter().all(|c| *c == '*')
}

/// The names a limit can be picked from: every group any member is in, and
/// every host name.
pub fn pickable(members: &[Member<'_>]) -> (Vec<String>, Vec<String>) {
	let groups: BTreeSet<&String> = members.iter().flat_map(|m| m.groups.iter()).collect();
	let hosts: BTreeMap<&str, ()> = members.iter().map(|m| (m.host_name, ())).collect();
	(
		groups.into_iter().cloned().collect(),
		hosts.into_keys().map(str::to_owned).collect(),
	)
}

#[cfg(test)]
mod tests {
	use super::*;
	use uuid::Uuid;

	struct Fixture {
		ids: Vec<Uuid>,
		groups: Vec<BTreeSet<String>>,
		names: Vec<&'static str>,
	}

	impl Fixture {
		fn new() -> Self {
			let spec: [(&str, &[&str]); 4] = [
				("example-central", &["tamanu", "central"]),
				("example-facility-a", &["tamanu", "facility"]),
				("example-facility-b", &["tamanu", "facility"]),
				(
					"outreach.site-network.example.internal",
					&["tamanu", "facility"],
				),
			];
			Self {
				ids: spec.iter().map(|_| Uuid::new_v4()).collect(),
				groups: spec
					.iter()
					.map(|(_, groups)| groups.iter().map(|g| (*g).to_owned()).collect())
					.collect(),
				names: spec.iter().map(|(name, _)| *name).collect(),
			}
		}

		fn members(&self) -> Vec<Member<'_>> {
			(0..self.ids.len())
				.map(|i| Member {
					id: self.ids[i],
					host_name: self.names[i],
					groups: &self.groups[i],
				})
				.collect()
		}

		fn run(&self, pattern: &str) -> Result<Vec<&str>, Unsupported> {
			matching(pattern, &self.members()).map(|ids| {
				ids.iter()
					.map(|id| {
						let at = self.ids.iter().position(|other| other == id).unwrap();
						self.names[at]
					})
					.collect()
			})
		}
	}

	const CENTRAL: &str = "example-central";
	const FACILITY_A: &str = "example-facility-a";
	const FACILITY_B: &str = "example-facility-b";
	const OUTREACH: &str = "outreach.site-network.example.internal";
	const EVERY: &[&str] = &[CENTRAL, FACILITY_A, FACILITY_B, OUTREACH];
	const FACILITIES: &[&str] = &[FACILITY_A, FACILITY_B, OUTREACH];

	type Row = (
		&'static str,
		&'static str,
		Result<&'static [&'static str], &'static str>,
	);

	#[test]
	fn a_limit_matches_exactly_the_machines_it_names() {
		let rows: &[Row] = &[
			("a host", "example-central", Ok(&[CENTRAL])),
			("a group", "facility", Ok(FACILITIES)),
			("a group every host is in", "tamanu", Ok(EVERY)),
			("all", "all", Ok(EVERY)),
			("a name nothing has", "nowhere", Ok(&[])),
			("a host named twice", "central:central", Ok(&[CENTRAL])),
			(
				"joined by a comma",
				"central,example-facility-a",
				Ok(&[CENTRAL, FACILITY_A]),
			),
			(
				"joined by a colon",
				"central:example-facility-a",
				Ok(&[CENTRAL, FACILITY_A]),
			),
			(
				"joined by both",
				"central,example-facility-a:example-facility-b",
				Ok(&[CENTRAL, FACILITY_A, FACILITY_B]),
			),
			(
				"a star at the end of a host name",
				"example-facility-*",
				Ok(&[FACILITY_A, FACILITY_B]),
			),
			(
				"a question mark for one character",
				"example-facility-?",
				Ok(&[FACILITY_A, FACILITY_B]),
			),
			(
				"a question mark is exactly one character",
				"example-facility-??",
				Ok(&[]),
			),
			("a lone star is every host", "*", Ok(EVERY)),
			(
				"a star at the start of a dotted name",
				"*.internal",
				Ok(&[OUTREACH]),
			),
			(
				"a star at the end of a dotted name",
				"outreach.*",
				Ok(&[OUTREACH]),
			),
			("a glob against a group name", "fac*", Ok(FACILITIES)),
			("a glob in the middle of a group name", "t*u", Ok(EVERY)),
			("a question mark in a group name", "?acility", Ok(FACILITIES)),
			("a group named by one glob only", "centr?l", Ok(&[CENTRAL])),
			("a group is case sensitive", "Facility", Ok(&[])),
			("a host is case sensitive", "Example-Central", Ok(&[])),
			("a glob is case sensitive", "EXAMPLE-*", Ok(&[])),
			("all is case sensitive", "ALL", Ok(&[])),
			(
				"whitespace around a token",
				"  central  ",
				Ok(&[CENTRAL]),
			),
			(
				"whitespace around a colon",
				"central : example-facility-a",
				Ok(&[CENTRAL, FACILITY_A]),
			),
			(
				"whitespace around a comma",
				"central , example-facility-a",
				Ok(&[CENTRAL, FACILITY_A]),
			),
			(
				"whitespace before an exclusion",
				"facility: !example-facility-b",
				Ok(&[FACILITY_A, OUTREACH]),
			),
			(
				"an empty token between two is skipped",
				"central::example-facility-a",
				Ok(&[CENTRAL, FACILITY_A]),
			),
			(
				"a trailing separator is skipped",
				"central:",
				Ok(&[CENTRAL]),
			),
			(
				"a leading comma is skipped",
				",central",
				Ok(&[CENTRAL]),
			),
			(
				"an exclusion from a group",
				"facility:!example-facility-b",
				Ok(&[FACILITY_A, OUTREACH]),
			),
			(
				"an intersection with a group",
				"tamanu:&central",
				Ok(&[CENTRAL]),
			),
			(
				"only an exclusion is everything but those",
				"!central",
				Ok(FACILITIES),
			),
			(
				"only an intersection is those of everything",
				"&central",
				Ok(&[CENTRAL]),
			),
			(
				"excluding a name nothing has",
				"!nowhere",
				Ok(EVERY),
			),
			("excluding all", "!all", Ok(&[])),
			(
				"excluding what was chosen",
				"central:!central",
				Ok(&[]),
			),
			(
				"excluding a group from all",
				"all:!facility",
				Ok(&[CENTRAL]),
			),
			(
				"excluding by a glob",
				"facility:!fac*",
				Ok(&[]),
			),
			(
				"two exclusions",
				"!central:!example-facility-a",
				Ok(&[FACILITY_B, OUTREACH]),
			),
			(
				"two intersections",
				"tamanu:&facility:&example-facility-?",
				Ok(&[FACILITY_A, FACILITY_B]),
			),
			(
				"an intersection and an exclusion",
				"tamanu:&facility:!example-facility-?",
				Ok(&[OUTREACH]),
			),
			(
				"an intersection that shares nothing",
				"facility:&central",
				Ok(&[]),
			),
			(
				"an intersection with a glob",
				"example-facility-*:&facility",
				Ok(&[FACILITY_A, FACILITY_B]),
			),
			(
				"an intersection with what matched nothing",
				"nowhere:&central",
				Ok(&[]),
			),
			(
				"an exclusion with what matched nothing",
				"nowhere:!central",
				Ok(&[]),
			),
			(
				"an exclusion written before what it excludes from",
				"!example-facility-b:facility",
				Ok(&[FACILITY_A, OUTREACH]),
			),
			(
				"an exclusion written between two plain patterns",
				"facility:!example-facility-a:tamanu",
				Ok(&[CENTRAL, FACILITY_B, OUTREACH]),
			),
			(
				"an intersection written before what it intersects",
				"&central:tamanu",
				Ok(&[CENTRAL]),
			),
			(
				"whitespace only",
				"   ",
				Err("the limit is empty"),
			),
			("nothing", "", Err("the limit is empty")),
			(
				"separators only",
				":,:",
				Err("the limit is empty"),
			),
			("an intersection of nothing", "&", Err("names nothing")),
			("an exclusion of nothing", "!", Err("names nothing")),
			(
				"an exclusion of nothing among names",
				"central:!",
				Err("names nothing"),
			),
			(
				"a regular expression",
				"~example-.*",
				Err("cannot evaluate"),
			),
			(
				"a regular expression after a name",
				"central:~example-.*",
				Err("cannot evaluate"),
			),
			(
				"a subscript",
				"facility[0]",
				Err("cannot evaluate"),
			),
			(
				"a range subscript",
				"facility[0:1]",
				Err("cannot evaluate"),
			),
			(
				"a subscript in an exclusion",
				"!facility[0]",
				Err("cannot evaluate"),
			),
			(
				"a subscript in an intersection",
				"tamanu:&facility[0]",
				Err("cannot evaluate"),
			),
			(
				"a subscript after a name",
				"central:facility[0]",
				Err("cannot evaluate"),
			),
		];

		let f = Fixture::new();
		let failures: Vec<String> = rows
			.iter()
			.filter_map(|(name, pattern, expected)| {
				let got = f.run(pattern);
				let wrong = match (&got, expected) {
					(Ok(names), Ok(expected)) => names != expected,
					(Err(Unsupported(why)), Err(needle)) => !why.contains(needle),
					_ => true,
				};
				wrong.then(|| format!("{name}: {pattern:?} gave {got:?}, expected {expected:?}"))
			})
			.collect();
		assert!(
			failures.is_empty(),
			"{} of {} rows differ:\n{}",
			failures.len(),
			rows.len(),
			failures.join("\n")
		);
	}

	#[test]
	fn a_pattern_of_many_stars_matches_in_time_that_does_not_depend_on_them() {
		let f = Fixture::new();
		let stars = "*a".repeat(40) + "*x";
		assert!(f.run(&stars).unwrap().is_empty());
		assert!(matches("*a*b*", "xxaxxbxx"));
		assert!(!matches("*a*b*", "xxbxxaxx"));
		assert!(matches("a*", "a"));
		assert!(!matches("a?", "a"));
	}

	#[test]
	fn names_the_groups_and_hosts_a_limit_can_be_picked_from() {
		let f = Fixture::new();
		let (groups, hosts) = pickable(&f.members());
		assert_eq!(groups, ["central", "facility", "tamanu"]);
		assert_eq!(hosts.len(), 4);
	}
}
