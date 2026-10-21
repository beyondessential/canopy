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

	// With nothing chosen to begin from, a limit of only exclusions is every
	// machine less those.
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

	#[test]
	fn a_host_a_group_and_all() {
		let f = Fixture::new();
		assert_eq!(f.run("example-central").unwrap(), ["example-central"]);
		assert_eq!(
			f.run("facility").unwrap(),
			[
				"example-facility-a",
				"example-facility-b",
				"outreach.site-network.example.internal"
			]
		);
		assert_eq!(f.run("all").unwrap().len(), 4);
	}

	#[test]
	fn patterns_join_by_colon_or_comma_and_globs_reach_names() {
		let f = Fixture::new();
		assert_eq!(
			f.run("central,example-facility-a").unwrap(),
			["example-central", "example-facility-a"]
		);
		assert_eq!(
			f.run("example-facility-*").unwrap(),
			["example-facility-a", "example-facility-b"]
		);
		assert_eq!(f.run("example-facility-?").unwrap().len(), 2);
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
	fn exclusion_and_intersection() {
		let f = Fixture::new();
		assert_eq!(
			f.run("facility:!example-facility-b").unwrap(),
			[
				"example-facility-a",
				"outreach.site-network.example.internal"
			]
		);
		assert_eq!(f.run("tamanu:&central").unwrap(), ["example-central"]);
		assert_eq!(
			f.run("!central").unwrap().len(),
			3,
			"only exclusions is everything but those"
		);
	}

	#[test]
	fn a_pattern_matching_nothing_is_empty_and_one_it_cannot_read_is_refused() {
		let f = Fixture::new();
		assert!(f.run("nowhere").unwrap().is_empty());
		assert!(f.run("~example-.*").is_err());
		assert!(f.run("facility[0]").is_err());
		assert!(f.run("").is_err());
		assert!(f.run("&").is_err());
	}

	#[test]
	fn names_the_groups_and_hosts_a_limit_can_be_picked_from() {
		let f = Fixture::new();
		let (groups, hosts) = pickable(&f.members());
		assert_eq!(groups, ["central", "facility", "tamanu"]);
		assert_eq!(hosts.len(), 4);
	}
}
