use axum::extract::{FromRef, FromRequestParts, OptionalFromRequestParts};
use commons_errors::AppError;
use database::{
	Db,
	access_lists::{AccessLevel, AccessListEntry},
	tailscale_users::TailscaleUser as CachedTailscaleUser,
};
use diesel_async::AsyncPgConnection;
use http::request::Parts;

use crate::tailnet_directory::TailnetDirectory;

const TAILSCALE_USER_LOGIN: &str = "Tailscale-User-Login";
const TAILSCALE_USER_NAME: &str = "Tailscale-User-Name";
const TAILSCALE_USER_PROFILE_PIC: &str = "Tailscale-User-Profile-Pic";

/// Debug-only opt-in that makes the extractors authenticate from the real
/// `Tailscale-User-*` request headers instead of substituting a fixed
/// development identity. Setting it lets a test choose its own login per
/// request and resolve their access level through the real access lists and
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
///
/// Public because the safety-mode boundary defers to it the same way the
/// extractors do: with the dev identity in play a request is treated as holding
/// a danger-mode session, so the existing test suite
/// reaches graded handlers without driving a session. Compiled out of release
/// builds by the same guard, so nothing can reach it in production.
pub fn use_dev_identity() -> bool {
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
/// trusted. Its login is on no access list; the extractors give it the admin
/// level by skipping the check, so a dev build is an administrator without any
/// seeding.
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
	/// The caller's access level: the highest their login's place on the access
	/// lists or the tailnet policy gives them (see
	/// [`crate::tailnet_directory`]), and all where neither gives one. The lists
	/// are read whatever the policy says, so an unreachable control plane
	/// never withdraws a listed level.
	// spec: ADM#how-a-level-is-held
	pub async fn level(
		&self,
		db: &mut AsyncPgConnection,
		directory: Option<&TailnetDirectory>,
	) -> Result<AccessLevel, AppError> {
		let listed = AccessListEntry::highest_for(db, &self.login).await?;
		let granted = match directory {
			Some(directory) => directory.level_by_policy(&self.login).await,
			None => None,
		};
		Ok(listed
			.max(granted)
			.map(AccessLevel::from)
			.unwrap_or(AccessLevel::All))
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

/// Authenticate the caller and resolve their level, refusing one below
/// `required`. The development identity is an administrator.
///
/// The caller's name and picture are cached, so endpoints that record human
/// actions (issue/incident resolve, notes) and views naming people can render
/// them without round-tripping to Tailscale. Centralised here so every gated
/// handler gets it for free.
async fn authorise<S>(
	parts: &mut Parts,
	state: &S,
	required: AccessLevel,
) -> Result<(TailscaleUser, AccessLevel), AppError>
where
	Db: FromRef<S>,
	Option<TailnetDirectory>: FromRef<S>,
	S: Send + Sync,
{
	let (user, level, mut db) = if use_dev_identity() {
		let db = Db::from_ref(state).get().await?;
		(dev_identity(), AccessLevel::Admin, db)
	} else {
		let user = <TailscaleUser as FromRequestParts<S>>::from_request_parts(parts, state).await?;
		let mut db = Db::from_ref(state).get().await?;
		let directory = Option::<TailnetDirectory>::from_ref(state);
		let level = user.level(&mut db, directory.as_ref()).await?;
		if level < required {
			return Err(AppError::AuthInsufficientPermissions {
				required: match required {
					AccessLevel::Admin => "admin",
					AccessLevel::Pm => "pm",
					AccessLevel::All => "all",
				}
				.into(),
			});
		}
		(user, level, db)
	};

	CachedTailscaleUser::upsert(
		&mut db,
		&user.login,
		&user.name,
		user.profile_pic.as_deref(),
	)
	.await?;

	Ok((user, level))
}

/// A caller with a tailnet user identity, and the level they hold.
#[derive(Debug, Clone)]
pub struct TailscaleLevel(pub TailscaleUser, pub AccessLevel);

impl<S> FromRequestParts<S> for TailscaleLevel
where
	Db: FromRef<S>,
	Option<TailnetDirectory>: FromRef<S>,
	S: Send + Sync,
{
	type Rejection = AppError;

	async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
		let (user, level) = authorise(parts, state, AccessLevel::All).await?;
		Ok(Self(user, level))
	}
}

impl<S> OptionalFromRequestParts<S> for TailscaleLevel
where
	Db: FromRef<S>,
	Option<TailnetDirectory>: FromRef<S>,
	S: Send + Sync,
{
	type Rejection = AppError;

	/// `None` for a caller bearing no tailnet user identity; any other
	/// failure is an error, never a lower level.
	async fn from_request_parts(
		parts: &mut Parts,
		state: &S,
	) -> Result<Option<Self>, Self::Rejection> {
		match <Self as FromRequestParts<S>>::from_request_parts(parts, state).await {
			Ok(caller) => Ok(Some(caller)),
			Err(AppError::AuthMissingHeader(_)) => Ok(None),
			Err(err) => Err(err),
		}
	}
}

/// A caller at the PM level or above.
#[derive(Debug, Clone)]
pub struct TailscalePm(pub TailscaleUser);

impl<S> FromRequestParts<S> for TailscalePm
where
	Db: FromRef<S>,
	Option<TailnetDirectory>: FromRef<S>,
	S: Send + Sync,
{
	type Rejection = AppError;

	async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
		let (user, _) = authorise(parts, state, AccessLevel::Pm).await?;
		Ok(Self(user))
	}
}

/// A caller at the admin level.
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
		let (user, _) = authorise(parts, state, AccessLevel::Admin).await?;
		Ok(Self(user))
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
