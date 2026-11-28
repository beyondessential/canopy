//! Patterns for `LIKE` and `ILIKE`.

/// The pattern matching text that contains `text`, with the characters `LIKE`
/// and `ILIKE` read specially taken literally.
pub(crate) fn contains_pattern(text: &str) -> String {
	format!(
		"%{}%",
		text.replace('\\', "\\\\")
			.replace('%', "\\%")
			.replace('_', "\\_")
	)
}
