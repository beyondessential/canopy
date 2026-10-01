//! Flat-variable payloads for Slack Workflow Builder webhook triggers.
//!
//! Workflow Builder webhooks don't accept Block Kit directly. Each workflow
//! declares a named set of text variables in its trigger; the actual message
//! is composed inside Slack's workflow editor using those variables. So we
//! POST a flat JSON object keyed by the declared variable names — no
//! `blocks`, no `text`.
//!
//! Two incident workflows:
//! - `incident_open`: variables `server`, `severity`, `source_ref`, `message`,
//!   `link`. Openings, escalations, and reminders all post here, each carrying
//!   the incident summary (see [`super::summary`]): `server` is the incident's
//!   target, `severity` its worst live result, `source_ref` the per-result
//!   counts of its live issues, and `message` the list of them.
//! - `incident_resolve`: variables `server`, `by`, `link`. `by` is either the
//!   resolving operator's login, or — when the incident retired on its own —
//!   "the healthcheck recovering" (it describes the event, not an actor: the
//!   check went healthy again, which does not imply nobody intervened).
//!
//! Note: `link` is **not** rendered here. It's pure config (incident_id is
//! already on the row, and `PRIVATE_URL` is operator-set), so the drainer
//! injects it at delivery time. That way only the drainer process needs to
//! see `PRIVATE_URL` — every other process that enqueues outbox rows
//! (public-server, private-server, reachability job) doesn't have to know
//! the operator-facing URL.

use serde_json::{Value, json};

/// Cap on the `message` variable. It's the only unbounded variable a
/// workflow receives — a check filing's message is free text (command
/// output, a stack trace, a list of every stale backup) — and Slack rejects
/// an oversized webhook payload with "The message content exceeded the size
/// limit.", which the drainer then retries to exhaustion so the incident
/// never pages. Cap it well under Slack's message length limit, leaving the
/// workflow room to frame it with the other variables and the `link`.
const MAX_MESSAGE_LEN: usize = 2000;

/// Truncate to at most `max` characters, appending a marker when it had to
/// cut. Counts characters, not bytes, so it never splits a multi-byte
/// character, and the result's character count never exceeds `max`.
pub(crate) fn truncate(text: &str, max: usize) -> String {
	if text.chars().count() <= max {
		return text.to_string();
	}
	const SUFFIX: &str = "… (truncated)";
	let keep = max.saturating_sub(SUFFIX.chars().count());
	let mut out: String = text.chars().take(keep).collect();
	out.push_str(SUFFIX);
	out
}

/// `urgency` is the result-derived label for the workflow's `severity`
/// variable: Critical (escalating failure), Error (failure), Warning.
/// `counts` fills `source_ref`, which the workflow lays out as the incident's
/// headline figures.
pub fn incident_open(server_label: &str, urgency: &str, counts: &str, message: &str) -> Value {
	json!({
		"server": server_label,
		"severity": urgency,
		"source_ref": counts,
		"message": truncate(message, MAX_MESSAGE_LEN),
	})
}

pub fn incident_resolve(server_label: &str, by: Option<&str>) -> Value {
	json!({
		"server": server_label,
		// `None` means no operator was attached to the close: the incident
		// retired because its healthcheck started reporting healthy again.
		// Phrase it as the triggering event, not an actor — "automation"
		// was read as "a bot decided to close this", when in practice an
		// operator was usually involved; canopy just can't attribute it.
		"by": by.unwrap_or("the healthcheck recovering"),
	})
}

/// `maintenance_declared`: variables `target`, `by`, `until`, `note`.
pub fn maintenance_declared(
	target: &str,
	by: Option<&str>,
	until: &str,
	note: Option<&str>,
) -> Value {
	json!({
		"target": target,
		"by": by.unwrap_or("an operator"),
		"until": until,
		// The workflow lays the note out as a field, so an absent one reads
		// as a value rather than leaving the label dangling.
		"note": truncate(note.unwrap_or("not stated"), MAX_MESSAGE_LEN),
	})
}

/// `maintenance_ended`: variables `target`, `by`, `watching_from`. `by` is
/// the operator who lifted the window, or the event where its expected end
/// passed instead.
pub fn maintenance_ended(target: &str, by: Option<&str>, watching_from: &str) -> Value {
	json!({
		"target": target,
		"by": by.unwrap_or("its expected end passing"),
		"watching_from": watching_from,
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn short_message_passes_through_untouched() {
		let payload = incident_open("Prod", "Error", "1 failed", "boom");
		assert_eq!(payload["message"], "boom");
	}

	#[test]
	fn oversized_message_is_truncated_with_marker() {
		let long = "x".repeat(MAX_MESSAGE_LEN * 3);
		let payload = incident_open("Prod", "Error", "1 failed", &long);
		let got = payload["message"].as_str().unwrap();
		assert_eq!(
			got.chars().count(),
			MAX_MESSAGE_LEN,
			"truncated message fills the cap exactly, never exceeds it",
		);
		assert!(got.ends_with("… (truncated)"), "cut is marked; got: {got}");
	}

	#[test]
	fn truncate_respects_char_boundaries() {
		// Multi-byte characters must not be split mid-byte.
		let emoji = "🔥".repeat(MAX_MESSAGE_LEN * 2);
		let got = truncate(&emoji, MAX_MESSAGE_LEN);
		assert!(got.chars().count() <= MAX_MESSAGE_LEN);
		assert!(got.ends_with("… (truncated)"));
	}
}
