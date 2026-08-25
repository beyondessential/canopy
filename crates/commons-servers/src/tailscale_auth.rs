use axum::extract::{FromRef, FromRequestParts, OptionalFromRequestParts};
use commons_errors::AppError;
use database::{Db, admins::Admin, tailscale_users::TailscaleUser as CachedTailscaleUser};
use diesel_async::AsyncPgConnection;
use http::request::Parts;

use crate::tailnet_directory::TailnetDirectory;

const TAILSCALE_USER_LOGIN: &str = "Tailscale-User-Login";
const TAILSCALE_USER_NAME: &str = "Tailscale-User-Name";
const TAILSCALE_USER_PROFILE_PIC: &str = "Tailscale-User-Profile-Pic";

/// Debug-only opt-in that makes the extractors authenticate from the real
/// `Tailscale-User-*` request headers instead of substituting a fixed
/// development identity. Setting it lets a test choose its own login per
/// request and resolve administrative status through the real allow-list and
/// tailnet policy path.
///
/// The `cfg!(debug_assertions)` guard around every use is compiled out of
/// release builds, so this variable has no effect there: production always
/// authenticates, and no misconfiguration can bypass it.
const TRUST_HEADERS_ENV: &str = "CANOPY_TRUST_TAILSCALE_HEADERS";

/// Whether to skip authentication and act as the fixed development identity.
///
/// Only ever true in debug builds, and only when the caller has not opted into
/// trusting real headers via [`TRUST_HEADERS_ENV`]. Always false in release.
fn use_dev_identity() -> bool {
	cfg!(debug_assertions) && !trust_real_headers()
}

/// Whether [`TRUST_HEADERS_ENV`] opts into trusting real request headers. A
/// present-but-empty (or whitespace-only) value counts as unset, matching the
/// other environment toggles in this crate.
fn trust_real_headers() -> bool {
	std::env::var(TRUST_HEADERS_ENV)
		.map(|v| !v.trim().is_empty())
		.unwrap_or(false)
}

/// The fixed identity substituted in debug builds when real headers aren't
/// trusted. Its login is on no allow-list; the extractors grant it admin by
/// skipping the check, so a dev build is an administrator without any seeding.
fn dev_identity() -> TailscaleUser {
	TailscaleUser {
		login: "admin@localhost".into(),
		name: "You".into(),
		profile_pic: None,
	}
}

#[derive(Debug, Clone, Default)]
pub struct TailscaleUser {
	pub login: String,
	pub name: String,
	pub profile_pic: Option<String>,
}

impl TailscaleUser {
	/// Admin if the login is on the recorded allowlist, or the tailnet policy
	/// grants it admin (see [`crate::tailnet_directory`]). The allowlist is
	/// checked first, so an unreachable control plane can't lock out an
	/// allowlisted admin.
	pub async fn is_admin(
		&self,
		db: &mut AsyncPgConnection,
		directory: Option<&TailnetDirectory>,
	) -> Result<bool, AppError> {
		if Admin::check_email(db, &self.login).await? {
			return Ok(true);
		}
		if let Some(directory) = directory
			&& directory.is_admin_by_policy(&self.login).await
		{
			return Ok(true);
		}
		Ok(false)
	}
}

impl<S> FromRequestParts<S> for TailscaleUser
where
	S: Send + Sync,
{
	type Rejection = AppError;

	async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
		// Dev / test convenience: mirrors `TailscaleAdmin`. Without it, every
		// integration test would need to set the three Tailscale headers on
		// every authenticated request, even for endpoints that only need
		// "any user". A test opts into the real header path with
		// `TRUST_HEADERS_ENV` when it needs to vary the login per request.
		if use_dev_identity() {
			return Ok(dev_identity());
		}
		let login = parts
			.headers
			.get(TAILSCALE_USER_LOGIN)
			.ok_or(AppError::AuthMissingHeader(TAILSCALE_USER_LOGIN))
			.and_then(|value| {
				rfc2047_decoder::decode(value.as_bytes()).or_else(|_| {
					value
						.to_str()
						.map_err(|err| AppError::custom(format!("invalid header format: {err}")))
						.map(ToOwned::to_owned)
				})
			})?;
		let name = parts
			.headers
			.get(TAILSCALE_USER_NAME)
			.ok_or(AppError::AuthMissingHeader(TAILSCALE_USER_NAME))
			.and_then(|value| {
				rfc2047_decoder::decode(value.as_bytes()).or_else(|_| {
					value
						.to_str()
						.map_err(|err| AppError::custom(format!("invalid header format: {err}")))
						.map(ToOwned::to_owned)
				})
			})?;
		let profile_pic = parts
			.headers
			.get(TAILSCALE_USER_PROFILE_PIC)
			.map(|value| {
				rfc2047_decoder::decode(value.as_bytes()).or_else(|_| {
					value
						.to_str()
						.map_err(|err| AppError::custom(format!("invalid header format: {err}")))
						.map(ToOwned::to_owned)
				})
			})
			.transpose()?;

		Ok(TailscaleUser {
			login,
			name,
			profile_pic,
		})
	}
}

impl<S> OptionalFromRequestParts<S> for TailscaleUser
where
	S: Send + Sync,
{
	type Rejection = AppError;

	async fn from_request_parts(
		parts: &mut Parts,
		state: &S,
	) -> Result<Option<Self>, Self::Rejection> {
		<Self as FromRequestParts<S>>::from_request_parts(parts, state)
			.await
			.map(Some)
			.or_else(|err| {
				if let AppError::AuthMissingHeader(_) = err {
					Ok(None)
				} else {
					Err(err)
				}
			})
	}
}

#[derive(Debug, Clone)]
pub struct TailscaleAdmin(pub TailscaleUser);

impl<S> FromRequestParts<S> for TailscaleAdmin
where
	Db: FromRef<S>,
	Option<TailnetDirectory>: FromRef<S>,
	S: Send + Sync,
{
	type Rejection = AppError;

	async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
		let user = if use_dev_identity() {
			dev_identity()
		} else {
			let user =
				<TailscaleUser as FromRequestParts<S>>::from_request_parts(parts, state).await?;
			let mut db = Db::from_ref(state).get().await?;
			let directory = Option::<TailnetDirectory>::from_ref(state);
			if !user.is_admin(&mut db, directory.as_ref()).await? {
				return Err(AppError::AuthInsufficientPermissions {
					required: "admin".into(),
				});
			}
			user
		};

		// Cache the user's name + pic so endpoints that record human actions
		// (issue/incident resolve, notes) can render avatars without
		// round-tripping to Tailscale. Centralised here so every admin
		// handler gets it for free.
		let mut db = Db::from_ref(state).get().await?;
		CachedTailscaleUser::upsert(
			&mut db,
			&user.login,
			&user.name,
			user.profile_pic.as_deref(),
		)
		.await?;

		Ok(TailscaleAdmin(user))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	// Nextest runs each test in its own process, so mutating this process-wide
	// env var can't leak into another test. The key is read only here.
	#[test]
	fn opt_in_switches_off_the_dev_identity() {
		let key = TRUST_HEADERS_ENV;

		// Unset: debug builds fall back to the fixed development identity.
		unsafe { std::env::remove_var(key) };
		assert!(use_dev_identity());
		assert!(!trust_real_headers());

		// Any non-blank value opts into trusting real headers.
		for raw in ["1", "true", " yes "] {
			unsafe { std::env::set_var(key, raw) };
			assert!(trust_real_headers(), "{raw:?} should opt in");
			assert!(
				!use_dev_identity(),
				"{raw:?} should disable the dev identity"
			);
		}

		// A present-but-blank value counts as unset.
		for raw in ["", "   "] {
			unsafe { std::env::set_var(key, raw) };
			assert!(!trust_real_headers(), "{raw:?} should count as unset");
			assert!(use_dev_identity(), "{raw:?} should keep the dev identity");
		}

		unsafe { std::env::remove_var(key) };
	}
}
