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
	/// Whether a run acts on it: it carries `managed_by_ansible`.
	pub managed: bool,
}

/// The host name of each of an environment's `members`, in order.
///
/// A run acts on the managed members, so they are named among themselves, and
/// two of them that would share a name are refused. The rest are named among
/// themselves too, and where two of them would share a name they are all named
/// by their identifiers, since no run reaches them.
///
/// The served environment, the limit preview and the import all name members
/// through this, so none of them can name a machine differently.
pub fn host_names(members: &[Candidate<'_>]) -> Result<Vec<String>> {
	let mut names = vec![String::new(); members.len()];
	for run_acts_on in [true, false] {
		let among: Vec<usize> = (0..members.len())
			.filter(|index| members[*index].managed == run_acts_on)
			.collect();
		let named = match name_among(among.iter().map(|index| &members[*index])) {
			Ok(named) => named,
			Err(refusal) if run_acts_on => return Err(refusal),
			Err(_) => among
				.iter()
				.map(|index| members[*index].id.to_string())
				.collect(),
		};
		for (index, name) in among.into_iter().zip(named) {
			names[index] = name;
		}
	}
	Ok(names)
}

/// The host name of each of `members`, in order, named among themselves.
///
/// A member is named by the first label of its address where that is a tailnet
/// name, and by the address itself otherwise. A member that has no address, or
/// shares its name with another, is named by its Canopy name reduced to what
/// Ansible accepts, or by its identifier where nothing of that remains.
/// Refuses an environment where two members would still share a name, since
/// Ansible would merge them into one host.
fn name_among<'a>(members: impl Iterator<Item = &'a Candidate<'a>>) -> Result<Vec<String>> {
	let members: Vec<&Candidate<'_>> = members.collect();
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
			managed: true,
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

	#[test]
	fn the_unmanaged_are_named_apart_from_what_a_run_acts_on() {
		let mut unmanaged = member("Spare", Some("box.east.ts.net"), true);
		unmanaged.managed = false;
		// Named among everything, the label "box" the spare shares would send
		// Central to its own name, and nothing would collide. A run sees only
		// Central and "box", which would both be the host "box".
		let refused = host_names(&[
			member("Central", Some("box.west.ts.net"), true),
			member("box", None, false),
			unmanaged,
		])
		.unwrap_err();
		assert!(matches!(refused, AppError::Conflict(_)));
	}

	#[test]
	fn unmanaged_members_sharing_a_name_are_named_by_identifier() {
		let mut a = member("same", Some("a.ts.net"), true);
		let mut b = member("same", Some("a.ts.net"), true);
		a.managed = false;
		b.managed = false;
		let ids = [a.id.to_string(), b.id.to_string()];
		let managed = member("Central", Some("central.ts.net"), true);
		let names = host_names(&[a, managed, b]).unwrap();
		assert_eq!(
			names,
			[ids[0].clone(), "central".to_owned(), ids[1].clone()]
		);
	}
}
