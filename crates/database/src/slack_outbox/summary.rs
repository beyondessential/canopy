//! The incident summary every opening, escalation, and reminder notification
//! carries: the incident as it stands when the notification is sent, rather
//! than the single issue that happened to open it.
//!
//! [`render`] reads the incident's live members and hands the pure
//! [`compose`] what it needs, so the layout (ordering, counts, the cap) is
//! testable without a database.

use std::collections::HashMap;

use commons_errors::Result;
use commons_types::status::CheckResult;
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use serde_json::Value as JsonValue;
use uuid::Uuid;

use super::vars::{self, truncate};
use crate::{
	applications::Application,
	issues::{Incident, IncidentIssue, Issue, format_group_label},
	machines::Machine,
	server_groups::ServerGroup,
};

/// Cap on one issue's headline in the list, so a single long headline can't
/// crowd every other issue out of the message.
const MAX_HEADLINE_LEN: usize = 160;

/// One live member of the incident, as the summary lists it.
#[derive(Clone, Debug)]
pub struct Member {
	pub result: Option<CheckResult>,
	pub escalates: bool,
	pub headline: String,
	/// The application or machine the issue is on; `None` for an issue scoped
	/// to a group or to Canopy as a whole.
	pub location: Option<String>,
	pub joined_at: Timestamp,
}

/// The rendered summary: the open workflow's `severity`, `source_ref`, and
/// `message` variables.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
	pub severity: &'static str,
	pub counts: String,
	pub message: String,
}

/// Render the open workflow payload for `incident_id` from its live members.
///
/// `reminder_at` is when a reminder is being sent: the message then leads with
/// how long the incident has been open by that time. `None` for an opening or
/// an escalation.
// spec: INC#notification
pub async fn render(
	conn: &mut AsyncPgConnection,
	incident_id: Uuid,
	reminder_at: Option<Timestamp>,
) -> Result<JsonValue> {
	use crate::schema::{incident_issues, incidents, issues};

	let incident: Incident = incidents::table
		.select(Incident::as_select())
		.filter(incidents::id.eq(incident_id))
		.first(conn)
		.await?;
	let rows: Vec<(IncidentIssue, Issue)> = incident_issues::table
		.inner_join(issues::table.on(issues::id.eq(incident_issues::issue_id)))
		.select((IncidentIssue::as_select(), Issue::as_select()))
		.filter(incident_issues::incident_id.eq(incident_id))
		.filter(incident_issues::left_at.is_null())
		.load(conn)
		.await?;

	let application_ids: Vec<Uuid> = rows.iter().filter_map(|(_, i)| i.application_id).collect();
	let machine_ids: Vec<Uuid> = rows.iter().filter_map(|(_, i)| i.machine_id).collect();
	let applications: HashMap<Uuid, String> = if application_ids.is_empty() {
		HashMap::new()
	} else {
		Application::get_by_ids(conn, &application_ids)
			.await?
			.into_iter()
			.map(|a| (a.id, a.label()))
			.collect()
	};
	let machines: HashMap<Uuid, String> = Machine::get_by_ids(conn, &machine_ids)
		.await?
		.into_iter()
		.map(|m| (m.id, m.name.unwrap_or_else(|| m.id.to_string())))
		.collect();

	let members: Vec<Member> = rows
		.into_iter()
		.map(|(link, issue)| {
			let location = match (issue.application_id, issue.machine_id) {
				(Some(id), _) => Some(applications.get(&id).cloned().unwrap_or(id.to_string())),
				(None, Some(id)) => Some(machines.get(&id).cloned().unwrap_or(id.to_string())),
				(None, None) => None,
			};
			Member {
				result: issue.effective_result,
				escalates: issue.escalates_now(),
				headline: headline(&issue),
				location,
				joined_at: link.joined_at,
			}
		})
		.collect();

	let server = match incident.server_group_id {
		Some(gid) => format_group_label(&ServerGroup::get_by_id(conn, gid).await?, incident.rank),
		None => "Canopy".to_string(),
	};
	let lead = reminder_at.map(|at| open_for(incident.opened_at, at));
	let summary = compose(members, lead.as_deref());
	Ok(vars::incident_open(
		&server,
		summary.severity,
		&summary.counts,
		&summary.message,
	))
}

/// What an issue is called in the list: its headline, or its check where it
/// was filed without one.
fn headline(issue: &Issue) -> String {
	let text = issue
		.description
		.as_deref()
		.map(str::trim)
		.filter(|d| !d.is_empty())
		.or(issue.check_name.as_deref())
		.unwrap_or(&issue.r#ref);
	truncate(text, MAX_HEADLINE_LEN)
}

/// "Open for 3 days", counting whole days since the incident opened.
pub fn open_for(opened_at: Timestamp, at: Timestamp) -> String {
	let days = (at.duration_since(opened_at).as_secs() / 86_400).max(1);
	if days == 1 {
		"Open for 1 day".to_string()
	} else {
		format!("Open for {days} days")
	}
}

fn rank(result: Option<CheckResult>) -> u8 {
	result.map_or(u8::MAX, CheckResult::urgency_rank)
}

fn result_label(result: Option<CheckResult>) -> &'static str {
	match result {
		Some(CheckResult::Failed) => "Failed",
		Some(CheckResult::Warning) => "Warning",
		Some(CheckResult::Broken) => "Broken",
		Some(CheckResult::Passed) => "Passed",
		Some(CheckResult::Skipped) => "Skipped",
		None => "Ungraded",
	}
}

fn count_label(result: Option<CheckResult>, n: usize) -> String {
	match result {
		Some(CheckResult::Warning) if n != 1 => format!("{n} warnings"),
		result => format!("{n} {}", result_label(result).to_lowercase()),
	}
}

/// The most issues a summary lists; the rest are counted, and read on the
/// incident's page.
pub const MAX_LISTED: usize = 5;

/// Lay out the summary. Members are listed in timeline order: worst effective
/// result first, ungraded last, and most recently joined first among equals.
/// The list shows at most [`MAX_LISTED`] of them, ending with how many it left
/// out.
// spec: INC#notification
pub fn compose(mut members: Vec<Member>, lead: Option<&str>) -> Summary {
	members.sort_by(|a, b| {
		rank(a.result)
			.cmp(&rank(b.result))
			.then(b.joined_at.cmp(&a.joined_at))
	});

	let severity = if members.iter().any(|m| m.escalates) {
		"Critical"
	} else if members
		.iter()
		.any(|m| m.result == Some(CheckResult::Failed))
	{
		"Error"
	} else {
		"Warning"
	};

	let mut counts: Vec<(Option<CheckResult>, usize)> = Vec::new();
	for m in &members {
		match counts.last_mut() {
			Some((result, n)) if *result == m.result => *n += 1,
			_ => counts.push((m.result, 1)),
		}
	}
	let counts = counts
		.into_iter()
		.map(|(result, n)| count_label(result, n))
		.collect::<Vec<_>>()
		.join(", ");

	let mut lines: Vec<String> = members
		.iter()
		.take(MAX_LISTED)
		.map(|m| {
			let label = result_label(m.result);
			match &m.location {
				Some(location) => format!("• {label}: {} on {location}", m.headline),
				None => format!("• {label}: {}", m.headline),
			}
		})
		.collect();
	if members.len() > MAX_LISTED {
		lines.push(format!("and {} more", members.len() - MAX_LISTED));
	}

	let list = lines.join("\n");
	let message = match lead {
		Some(lead) => format!("{lead}\n\n{list}"),
		None => list,
	};
	Summary {
		severity,
		counts,
		message,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use jiff::SignedDuration;

	fn member(result: Option<CheckResult>, headline: &str, mins_ago: i64) -> Member {
		Member {
			result,
			escalates: false,
			headline: headline.into(),
			location: Some("central-1".into()),
			joined_at: Timestamp::now() - SignedDuration::from_mins(mins_ago),
		}
	}

	#[test]
	fn lists_worst_first_then_most_recent() {
		let summary = compose(
			vec![
				member(Some(CheckResult::Warning), "disk filling", 1),
				member(Some(CheckResult::Failed), "old failure", 30),
				member(None, "legacy", 0),
				member(Some(CheckResult::Failed), "new failure", 5),
			],
			None,
		);
		assert_eq!(
			summary.message,
			"• Failed: new failure on central-1\n\
			 • Failed: old failure on central-1\n\
			 • Warning: disk filling on central-1\n\
			 • Ungraded: legacy on central-1",
		);
		assert_eq!(summary.counts, "2 failed, 1 warning, 1 ungraded");
		assert_eq!(summary.severity, "Error");
	}

	#[test]
	fn severity_is_the_worst_live_result() {
		let warnings = compose(
			vec![
				member(Some(CheckResult::Warning), "a", 1),
				member(Some(CheckResult::Warning), "b", 2),
			],
			None,
		);
		assert_eq!(warnings.severity, "Warning");
		assert_eq!(warnings.counts, "2 warnings");

		let mut escalating = member(Some(CheckResult::Failed), "down", 1);
		escalating.escalates = true;
		let critical = compose(
			vec![member(Some(CheckResult::Failed), "x", 2), escalating],
			None,
		);
		assert_eq!(critical.severity, "Critical");
	}

	#[test]
	fn scopeless_issue_has_no_location() {
		let mut m = member(Some(CheckResult::Failed), "backup corrupt", 1);
		m.location = None;
		assert_eq!(compose(vec![m], None).message, "• Failed: backup corrupt");
	}

	#[test]
	fn reminder_leads_with_how_long_it_has_been_open() {
		let summary = compose(
			vec![member(Some(CheckResult::Failed), "down", 1)],
			Some("Open for 2 days"),
		);
		assert_eq!(
			summary.message,
			"Open for 2 days\n\n• Failed: down on central-1"
		);
	}

	#[test]
	fn open_for_counts_whole_days() {
		let opened = Timestamp::now();
		let day = SignedDuration::from_hours(24);
		assert_eq!(open_for(opened, opened + day), "Open for 1 day");
		assert_eq!(
			open_for(opened, opened + day * 2 + SignedDuration::from_hours(23)),
			"Open for 2 days"
		);
	}

	#[test]
	fn list_is_capped_with_a_count_of_the_rest() {
		let members: Vec<Member> = (0..8)
			.map(|i| member(Some(CheckResult::Failed), &format!("check {i}"), i))
			.collect();
		let summary = compose(members, Some("Open for 1 day"));
		assert_eq!(
			summary.message,
			"Open for 1 day\n\n\
			 • Failed: check 0 on central-1\n\
			 • Failed: check 1 on central-1\n\
			 • Failed: check 2 on central-1\n\
			 • Failed: check 3 on central-1\n\
			 • Failed: check 4 on central-1\n\
			 and 3 more",
			"the newest five, then a count of the rest",
		);
		assert_eq!(summary.counts, "8 failed", "counts cover every live issue");
	}

	#[test]
	fn worst_issues_are_listed_ahead_of_newer_lesser_ones() {
		let mut members: Vec<Member> = (0..5)
			.map(|i| member(Some(CheckResult::Warning), "warn", i))
			.collect();
		members.push(member(Some(CheckResult::Failed), "old failure", 600));
		let summary = compose(members, None);
		assert!(summary.message.starts_with("• Failed: old failure"));
		assert!(summary.message.ends_with("and 1 more"));
	}

	#[test]
	fn list_that_fits_has_no_tail() {
		let members: Vec<Member> = (0..MAX_LISTED as i64)
			.map(|i| member(Some(CheckResult::Failed), "x", i))
			.collect();
		assert!(!compose(members, None).message.contains("more"));
	}
}
