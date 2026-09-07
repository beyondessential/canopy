//! Device authentication for the public-server and private-server's
//! `/public/...` mount. The presence of an `Option<TailnetDirectory>`
//! in axum state chooses the auth path:
//!
//! - **No directory** (internet-facing public-server binary): use the
//!   [`mtls`] path. Devices present a client certificate; the public
//!   key derived from it keys into `device_keys.key_data`. An unknown
//!   key does not authenticate — no row is auto-created.
//! - **Directory present** (private-server's `/public/...` mount,
//!   reached via the Tailscale Operator's ingress proxy): use the
//!   [`tailnet`] path. The Tailscale ingress terminates the client's
//!   TLS at the proxy, so no client certificate survives — mTLS is
//!   physically not available on this path and isn't even attempted.
//!   The caller's tailnet CGNAT v4 or ULA v6 address is read from
//!   `X-Forwarded-For` (via `axum-client-ip`'s `ClientIp`), resolved
//!   to a node identity through the Tailscale control plane API, then
//!   keyed into `devices.tailscale_node_id`. An unknown node does not
//!   authenticate — no row is auto-created.
//!
//! Exactly one path runs per request. Failure surfaces as
//! `AuthMissingCertificate`.

use std::net::{IpAddr, Ipv6Addr};

use axum::{RequestPartsExt as _, extract::FromRef};
use axum_client_ip::ClientIp;
use commons_errors::AppError;
use commons_types::device::DeviceRole;
use database::{
	Db,
	devices::{Device, NewDeviceConnection},
};

use tracing::warn;

use crate::tailnet_directory::TailnetDirectory;

pub mod keygen;
pub mod mtls;
pub mod pop;
pub mod tailnet;

/// Which path produced this [`AuthDevice`]. Carried alongside the device
/// for connection logging and future audit; handlers don't need to
/// inspect it.
#[derive(Clone, Debug)]
pub enum AuthMethod {
	Mtls,
	Tailnet { node_id: String },
}

#[derive(Debug, Clone)]
pub struct AuthDevice(pub Device, pub AuthMethod);

macro_rules! device_role_struct {
	($name:ident, $allowed_role:expr) => {
		#[derive(Clone, Debug)]
		pub struct $name(#[allow(dead_code)] pub AuthDevice);

		impl<S> axum::extract::FromRequestParts<S> for $name
		where
			Db: FromRef<S>,
			Option<TailnetDirectory>: FromRef<S>,
			$crate::device_auth::mtls::ClientCertHeader: FromRef<S>,
			S: Send + Sync,
		{
			type Rejection = AppError;

			async fn from_request_parts(
				parts: &mut axum::http::request::Parts,
				state: &S,
			) -> Result<Self, Self::Rejection> {
				let auth = AuthDevice::from_request_parts(parts, state).await?;
				if auth.0.role == DeviceRole::Admin || auth.0.role == $allowed_role {
					Ok(Self(auth))
				} else {
					Err(AppError::AuthInsufficientPermissions {
						required: format!("{} or admin", stringify!($name).to_lowercase()),
					})
				}
			}
		}
	};
}

device_role_struct!(AdminDevice, DeviceRole::Admin);
device_role_struct!(ServerDevice, DeviceRole::Machine);
device_role_struct!(ReleaserDevice, DeviceRole::Releaser);
device_role_struct!(BackupRestoreDevice, DeviceRole::BackupRestore);

/// A read that is open to everyone but answers a device for its own group
/// takes `Option<AuthDevice>`: absent identity is not a refusal, it just
/// narrows what the caller is offered.
impl<S> axum::extract::OptionalFromRequestParts<S> for AuthDevice
where
	Db: FromRef<S>,
	Option<TailnetDirectory>: FromRef<S>,
	mtls::ClientCertHeader: FromRef<S>,
	S: Send + Sync,
{
	type Rejection = AppError;

	async fn from_request_parts(
		parts: &mut axum::http::request::Parts,
		state: &S,
	) -> Result<Option<Self>, Self::Rejection> {
		match <Self as axum::extract::FromRequestParts<S>>::from_request_parts(parts, state).await {
			Ok(device) => Ok(Some(device)),
			// A credential Canopy cannot place is anonymous, not refused: a
			// stale certificate must not fail a path that serves everyone.
			Err(_) => Ok(None),
		}
	}
}

impl<S> axum::extract::FromRequestParts<S> for AuthDevice
where
	Db: FromRef<S>,
	Option<TailnetDirectory>: FromRef<S>,
	mtls::ClientCertHeader: FromRef<S>,
	S: Send + Sync,
{
	type Rejection = AppError;

	async fn from_request_parts(
		parts: &mut axum::http::request::Parts,
		state: &S,
	) -> Result<Self, Self::Rejection> {
		let mut db = Db::from_ref(state).get().await?;

		// Directory presence is the toggle. On the private-server's
		// `/public/...` mount it's `Some` and we go tailnet-only;
		// on the public-server binary it's `None` and we go mTLS-only.
		// mTLS is not attempted on the tunnel path because the Tailscale
		// ingress proxy terminates client TLS — there is no cert to
		// forward, and attempting to read one just adds confusion. Each
		// path emits its own "auth failed" error so logs are unambiguous
		// about which mechanism the caller hit.
		let (device, method) = if let Some(directory) = Option::<TailnetDirectory>::from_ref(state)
		{
			tailnet::resolve(parts, &mut db, &directory)
				.await?
				.map(|(device, node_id)| (device, AuthMethod::Tailnet { node_id }))
				.ok_or(AppError::AuthTailnetIdentityMissing)?
		} else {
			mtls::resolve(parts, &mut db, mtls::ClientCertHeader::from_ref(state))
				.await?
				.map(|device| (device, AuthMethod::Mtls))
				.ok_or(AppError::AuthMissingCertificate)?
		};

		let user_agent = parts
			.headers
			.get(axum::http::header::USER_AGENT)
			.and_then(|s| s.to_str().ok())
			.map(|s| s.to_owned());

		let client_ip: Option<ClientIp> = parts.extract().await.ok();
		let ip = client_ip.map_or(IpAddr::V6(Ipv6Addr::UNSPECIFIED), |c| c.0);

		// Recording the connection is part of accepting the request, not a
		// condition of it: the caller authenticated, and history it can't
		// influence must not decide that for it. Connection history is stored
		// in weekly ranges, so an unprovisioned week would otherwise fail this
		// insert and with it every authenticated request on both mounts — a
		// full device-API outage caused by an audit-trail write.
		// spec: HST#recording-a-connection
		if let Err(err) = (NewDeviceConnection {
			device_id: device.id,
			ip: ip.into(),
			user_agent,
		})
		.create(&mut db)
		.await
		{
			warn!(device_id = %device.id, "failed to record device connection: {err}");
		}

		Ok(Self(device, method))
	}
}
