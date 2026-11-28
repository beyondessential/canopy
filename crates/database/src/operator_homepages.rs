//! The page each operator has chosen for the operator interface's root.
//!
//! Spec: `.workhorse/specs/private-server/homepage.md` (id `HOME`).
//!
//! A choice belongs to the operator's tailnet login, held lowercased so it
//! follows them whatever case their identity arrives in.

use commons_errors::{AppError, Result};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};

use crate::access_lists::normalise_login;

/// Whether a path can be a homepage: a path within the operator interface,
/// beginning with a single `/`, more than `/` alone, and carrying no query or
/// fragment. A backslash is refused too, since URL parsers read it as a `/`,
/// which would let `/\host` reach another site as `//host` would. So is a `.`
/// or `..` segment, plain or percent-encoded: the path it resolves to is
/// another one, and `/..` resolves to the root, which would open itself.
// spec: HOME#the-default-homepage
pub fn is_valid_path(path: &str) -> bool {
	path.starts_with('/')
		&& !path.starts_with("//")
		&& path != "/"
		&& !path.contains(['?', '#', '\\'])
		&& !path.chars().any(char::is_control)
		&& !path.split('/').any(is_dot_segment)
}

/// Whether a segment is one URL resolution reads as `.` or `..`.
fn is_dot_segment(segment: &str) -> bool {
	matches!(
		segment.to_ascii_lowercase().replace("%2e", ".").as_str(),
		"." | ".."
	)
}

/// The homepage an operator has chosen.
pub struct OperatorHomepage;

impl OperatorHomepage {
	/// The path the login has chosen, if any.
	pub async fn get(db: &mut AsyncPgConnection, login: &str) -> Result<Option<String>> {
		use crate::schema::operator_homepages::dsl;
		dsl::operator_homepages
			.select(dsl::path)
			.filter(dsl::login.eq(normalise_login(login)))
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// Choose a homepage for the login, replacing any earlier choice.
	pub async fn set(db: &mut AsyncPgConnection, login: &str, path: &str) -> Result<()> {
		use crate::schema::operator_homepages::dsl;
		if !is_valid_path(path) {
			return Err(AppError::BadRequest(format!(
				"{path:?} is not a page's path: it must begin with a single `/`, be more than `/` alone, and carry no query or fragment"
			)));
		}
		diesel::insert_into(dsl::operator_homepages)
			.values((dsl::login.eq(normalise_login(login)), dsl::path.eq(path)))
			.on_conflict(dsl::login)
			.do_update()
			.set(dsl::path.eq(path))
			.execute(db)
			.await?;
		Ok(())
	}

	/// Clear the login's choice, so the root opens their default again.
	pub async fn clear(db: &mut AsyncPgConnection, login: &str) -> Result<()> {
		use crate::schema::operator_homepages::dsl;
		diesel::delete(dsl::operator_homepages)
			.filter(dsl::login.eq(normalise_login(login)))
			.execute(db)
			.await?;
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::is_valid_path;

	#[test]
	fn a_homepage_is_a_path_within_the_interface() {
		for good in [
			"/status",
			"/fleet/pm",
			"/fleet/pm/",
			"/a",
			"/v1.2",
			"/a/...b",
		] {
			assert!(is_valid_path(good), "{good:?} should be a homepage");
		}
		for bad in [
			"",
			"/",
			"fleet",
			"//evil.example",
			"/\\evil.example",
			"https://evil.example/status",
			"/status?filter=x",
			"/status#top",
			"/status\n",
			"/..",
			"/.",
			"/fleet/../..",
			"/fleet/./pm",
			"/%2e%2e",
			"/%2E.",
			"/.%2e/x",
		] {
			assert!(!is_valid_path(bad), "{bad:?} should not be a homepage");
		}
	}
}
