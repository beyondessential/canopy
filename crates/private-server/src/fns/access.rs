//! The levels' lists, and what the tailnet policy grants each level.
//!
//! Spec: `.workhorse/specs/private-server/access-levels.md` (id `ADM`), "The
//! permissions page".

use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{ProblemDetailsSchema, Result};
use commons_servers::{
	audit::Audit,
	tailnet_directory::{CANOPY_CAP, CANOPY_SERVICE_TAG, ConferringSource},
	tailscale_auth::TailscaleAdmin,
};
use database::{
	access_lists::{AccessListEntry, ListLevel},
	audit::Action,
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only, silent: list))
		.routes(routes!(danger(issues), audit(AccessListEntryAdded): add))
		.routes(routes!(danger(invalidates), audit(AccessListEntryRemoved): remove))
}

/// Who holds the levels above all, and how.
#[derive(Debug, Serialize, ToSchema)]
pub struct AccessOverview {
	/// Admin then PM, each with its list and what the policy grants it.
	pub levels: Vec<LevelHolders>,
	/// The tailnet policy Canopy reads levels from.
	pub policy: PolicyStatus,
}

/// The two sources of one level.
#[derive(Debug, Serialize, ToSchema)]
pub struct LevelHolders {
	/// The level.
	pub level: ListLevel,
	/// The key a Canopy capability value sets true to confer the level.
	pub key: String,
	/// The logins on the level's list, alphabetically.
	pub list: Vec<ListedLogin>,
	/// The sources of the policy's grants conferring the level, as the policy
	/// writes them, with who each covers. Empty where the policy has not been
	/// read.
	pub granted_to: Vec<ConferringSource>,
}

/// A login on a list.
#[derive(Debug, Serialize, ToSchema)]
pub struct ListedLogin {
	/// The login, lowercased.
	pub login: String,
	/// When it was put on the list.
	pub added_at: Timestamp,
}

/// Whether and when Canopy read the tailnet policy.
#[derive(Debug, Serialize, ToSchema)]
pub struct PolicyStatus {
	/// Whether Canopy is configured to read the policy. Where it is not, the
	/// lists alone confer levels.
	pub configured: bool,
	/// When the policy was last read successfully; null where it is not
	/// configured or no read has succeeded yet.
	pub read_at: Option<Timestamp>,
	/// The capability whose values confer levels.
	pub capability: String,
	/// The tag a grant's destinations must include to confer a level.
	pub service_tag: String,
}

/// The lists, and what the tailnet policy grants each level.
///
/// Returns admin then PM, each with the logins on its list and the sources the
/// tailnet policy grants it to as the policy writes them, with who each
/// covers; and whether and when Canopy last read the policy.
#[utoipa::path(
	post,
	path = "/list",
	operation_id = "access_list",
	tag = "access",
	security(("tailscale-admin" = [])),
	responses(
		(status = 200, body = AccessOverview),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
// spec: ADM#the-permissions-page
pub async fn list(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
) -> Result<Json<AccessOverview>> {
	let mut conn = state.db.get().await?;
	let entries = AccessListEntry::list(&mut conn).await?;
	let summary = match &state.tailnet_directory {
		Some(directory) => Some(directory.policy_summary().await),
		None => None,
	};

	let levels = ListLevel::ALL
		.into_iter()
		.map(|level| LevelHolders {
			level,
			key: level.as_str().to_owned(),
			list: entries
				.iter()
				.filter(|entry| entry.level == level)
				.map(|entry| ListedLogin {
					login: entry.login.clone(),
					added_at: entry.created_at,
				})
				.collect(),
			granted_to: summary
				.as_ref()
				.map(|summary| match level {
					ListLevel::Admin => summary.admin.clone(),
					ListLevel::Pm => summary.pm.clone(),
				})
				.unwrap_or_default(),
		})
		.collect();

	Ok(Json(AccessOverview {
		levels,
		policy: PolicyStatus {
			configured: summary.is_some(),
			read_at: summary.and_then(|summary| summary.read_at),
			capability: CANOPY_CAP.to_owned(),
			service_tag: CANOPY_SERVICE_TAG.to_owned(),
		},
	}))
}

/// A login on a level's list.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ListEntryArgs {
	/// The level whose list this is.
	pub level: ListLevel,
	/// The tailnet login, in any case.
	pub login: String,
}

/// Put a login on a level's list.
///
/// Has no effect where the login is already on it, whatever its case.
#[utoipa::path(
	post,
	path = "/add",
	operation_id = "access_add",
	tag = "access",
	security(("tailscale-admin" = [])),
	request_body = ListEntryArgs,
	responses(
		(status = 200, description = "On the list."),
		(status = 400, body = ProblemDetailsSchema),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
// spec: ADM#the-permissions-page
pub async fn add(
	audit: Audit,
	_admin: TailscaleAdmin,
	Json(args): Json<ListEntryArgs>,
) -> Result<Json<()>> {
	let mut conn = audit.begin().await?;
	match AccessListEntry::add(&mut conn, args.level, &args.login).await? {
		Some(entry) => {
			conn.record(Action::AccessListEntryAdded {
				level: entry.level,
				login: entry.login,
			})
			.await?;
		}
		None => audit.unchanged(),
	}
	Ok(Json(()))
}

/// Take a login off a level's list.
///
/// Has no effect where the login is not on it, whatever its case.
#[utoipa::path(
	post,
	path = "/remove",
	operation_id = "access_remove",
	tag = "access",
	security(("tailscale-admin" = [])),
	request_body = ListEntryArgs,
	responses(
		(status = 200, description = "Off the list."),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
// spec: ADM#the-permissions-page
pub async fn remove(
	audit: Audit,
	_admin: TailscaleAdmin,
	Json(args): Json<ListEntryArgs>,
) -> Result<Json<()>> {
	let mut conn = audit.begin().await?;
	if AccessListEntry::remove(&mut conn, args.level, &args.login).await? {
		conn.record(Action::AccessListEntryRemoved {
			level: args.level,
			login: database::access_lists::normalise_login(&args.login),
		})
		.await?;
	} else {
		audit.unchanged();
	}
	Ok(Json(()))
}
