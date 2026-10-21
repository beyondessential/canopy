//! The host name each member of a served environment goes by: what a run's
//! limit and output name it by.
// spec: INV#reaching-a-machine

use std::collections::BTreeMap;

use commons_errors::{AppError, Result};
use commons_types::Uuid;

/// A member as far as naming it goes.
pub struct Candidate<'a> {
	pub id: Uuid,
	/// The machine's name in Canopy.
	pub name: &'a str,
	/// The address the member is reached at.
	pub address: Option<&'a str>,
	/// Whether that address is the tailnet name of the device bound to it, as
	/// opposed to an `ansible_host` variable or an application's recorded host.
	pub tailnet: bool,
}

/// The host name of each of `members`, in order.
///
/// A member is named by the first label of its address where that is a tailnet
/// name, and by the address itself otherwise. A member that has no address, or
/// shares its name with another, is named by its Canopy name reduced to what
/// Ansible accepts, or by its identifier where nothing of that remains.
/// Refuses an environment where two members would still share a name, since
/// Ansible would merge them into one host.
pub fn host_names(members: &[Candidate<'_>]) -> Result<Vec<String>> {
	let first: Vec<&str> = members
		.iter()
		.map(|member| match member.address {
			Some(address) if member.tailnet => address.split('.').next().unwrap_or(address),
			Some(address) => address,
			None => "",
		})
		.collect();

	let mut uses: BTreeMap<&str, usize> = BTreeMap::new();
	for name in first.iter().filter(|name| !name.is_empty()) {
		*uses.entry(name).or_default() += 1;
	}

	let names: Vec<String> = members
		.iter()
		.zip(&first)
		.map(|(member, first)| {
			if !first.is_empty() && uses.get(first) == Some(&1) {
				(*first).to_owned()
			} else {
				let slug = slug(member.name);
				if slug.is_empty() {
					member.id.to_string()
				} else {
					slug
				}
			}
		})
		.collect();

	let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
	for (member, name) in members.iter().zip(&names) {
		if let Some(other) = seen.insert(name, member.name) {
			return Err(AppError::Conflict(format!(
				"machines {other:?} and {:?} would both be the host {name:?}; \
				 renaming one of them in Canopy tells a run them apart",
				member.name
			)));
		}
	}
	Ok(names)
}

/// A name reduced to the characters Ansible accepts in a host name, each run
/// of any other replaced by one hyphen and none left at either end.
fn slug(name: &str) -> String {
	let mut slug = String::with_capacity(name.len());
	let mut pending = false;
	for c in name.chars() {
		if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
			if pending && !slug.is_empty() {
				slug.push('-');
			}
			pending = false;
			slug.push(c);
		} else {
			pending = true;
		}
	}
	slug.trim_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
	use super::*;

	fn member<'a>(name: &'a str, address: Option<&'a str>, tailnet: bool) -> Candidate<'a> {
		Candidate {
			id: Uuid::new_v4(),
			name,
			address,
			tailnet,
		}
	}

	#[test]
	fn a_tailnet_name_is_cut_to_its_first_label() {
		let names = host_names(&[member(
			"Central",
			Some("kamaka-prod-central.tail1234.ts.net"),
			true,
		)])
		.unwrap();
		assert_eq!(names, ["kamaka-prod-central"]);
	}

	#[test]
	fn any_other_address_is_kept_whole() {
		let names = host_names(&[
			member("a", Some("10.0.0.7"), false),
			member("b", Some("db.example.org"), false),
		])
		.unwrap();
		assert_eq!(names, ["10.0.0.7", "db.example.org"]);
	}

	#[test]
	fn a_shared_name_falls_back_to_each_machines_own_name() {
		let names = host_names(&[
			member("Kamaka Central", Some("box.east.ts.net"), true),
			member("Kamaka Facility", Some("box.west.ts.net"), true),
			member("Other", Some("other.west.ts.net"), true),
		])
		.unwrap();
		assert_eq!(names, ["Kamaka-Central", "Kamaka-Facility", "other"]);
	}

	#[test]
	fn no_address_falls_back_to_the_name_then_the_identifier() {
		let nameless = member("!!!", None, false);
		let id = nameless.id.to_string();
		let names = host_names(&[member("a b/c", None, false), nameless]).unwrap();
		assert_eq!(names, ["a-b-c".to_owned(), id]);
	}

	#[test]
	fn two_members_still_sharing_a_name_are_refused() {
		let refused = host_names(&[
			member("same name", Some("a.ts.net"), true),
			member("same-name", Some("a.ts.net"), true),
		])
		.unwrap_err();
		assert!(matches!(refused, AppError::Conflict(_)));
	}
}
