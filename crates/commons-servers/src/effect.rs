//! How an effect outside Canopy's own records failed, such as a write to a
//! secret store or an object store: whether it is known not to have taken
//! place, or may have. A store reports this for its own calls, since only the
//! call can tell a refusal from no answer; the audit log reads it to settle an
//! entry committed ahead of the effect (see `crate::audit`).

use commons_errors::AppError;

/// An effect outside Canopy's records that did not take, such as a write to a
/// secret store or an object store.
#[derive(Debug)]
pub struct EffectFailed {
	/// Whether it is known that nothing was done: the outside service answered
	/// and refused. An effect that got no answer may have taken place.
	pub nothing_done: bool,
	pub error: AppError,
}

impl EffectFailed {
	/// An effect refused, or never attempted, so known not to have taken place.
	pub fn nothing_done(error: AppError) -> Self {
		Self {
			nothing_done: true,
			error,
		}
	}

	/// An effect whose outcome is unknown.
	pub fn unknown(error: AppError) -> Self {
		Self {
			nothing_done: false,
			error,
		}
	}
}

impl From<EffectFailed> for AppError {
	fn from(failed: EffectFailed) -> Self {
		failed.error
	}
}
