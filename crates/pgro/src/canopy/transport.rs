//! The transport pgro reaches Canopy through.
//!
//! Spec: `.workhorse/specs/restore-operator/overview.md` (id `PGRO`).
//!
//! pgro's one path to Canopy is the tailnet: the Tailscale sidecar in its pod
//! carries the connection, and Canopy knows the caller by the sidecar's node.
//! There is no client certificate and no device key, so nothing here renews or
//! falls back.

use std::{net::IpAddr, time::Duration};

use bes_canopy_api::{CanopyRequest, CanopyResponse, CanopyTransport, Error, Result, async_trait};
use reqwest::{Url, redirect::Policy};

/// Where Canopy mounts its device API on the tailnet endpoint. The same routes
/// are served at the root of the public endpoint, which pgro does not use.
const TAILNET_MOUNT: &str = "/public";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// A dead connection is detected by the kernel's own keepalive, not by a cap on
/// how long a call may take: registering a large schema legitimately runs long.
const KEEPALIVE: Duration = Duration::from_secs(30);

/// How the transport finds Canopy's tailnet endpoint.
#[derive(Debug, Clone)]
pub struct TailnetEndpoint {
	/// Canopy's tailnet URL, e.g. `https://canopy.<tailnet>.ts.net`.
	pub url: Url,
	/// SOCKS5 URL of the Tailscale sidecar's userspace proxy. `None` connects
	/// directly, which is only meaningful where the process is itself on the
	/// tailnet, and for tests.
	pub socks5_proxy: Option<Url>,
	/// A tailnet address to connect to in place of resolving the URL's host.
	/// Set where the proxy cannot resolve the host itself; the host is still
	/// what TLS verifies.
	pub resolve_to: Option<IpAddr>,
}

pub struct TailnetTransport {
	http: reqwest::Client,
	base: Url,
}

impl TailnetTransport {
	pub fn new(endpoint: TailnetEndpoint) -> anyhow::Result<Self> {
		let TailnetEndpoint {
			url,
			socks5_proxy,
			resolve_to,
		} = endpoint;

		let mut builder = reqwest::Client::builder()
			.user_agent(concat!("pgro/", env!("CARGO_PKG_VERSION")))
			.redirect(Policy::none())
			.connect_timeout(CONNECT_TIMEOUT)
			.tcp_keepalive(KEEPALIVE);

		if let Some(mut proxy) = socks5_proxy {
			// `socks5` resolves the host here, where tailnet names do not
			// resolve; `socks5h` hands the name to the sidecar. An address
			// override resolves it here on purpose, so it keeps `socks5`.
			if proxy.scheme() == "socks5" && resolve_to.is_none() {
				proxy.set_scheme("socks5h").map_err(|()| {
					anyhow::anyhow!("cannot make the SOCKS5 proxy URL remote-resolving")
				})?;
			}
			builder = builder.proxy(reqwest::Proxy::all(proxy)?);
		}

		if let Some(ip) = resolve_to {
			let host = url
				.host_str()
				.ok_or_else(|| anyhow::anyhow!("Canopy tailnet URL {url} has no host"))?;
			let port = url
				.port_or_known_default()
				.ok_or_else(|| anyhow::anyhow!("Canopy tailnet URL {url} has no port"))?;
			builder = builder.resolve(host, (ip, port).into());
		}

		Ok(Self {
			http: builder.build()?,
			base: url,
		})
	}
}

#[async_trait]
impl CanopyTransport for TailnetTransport {
	async fn call(&self, request: CanopyRequest) -> Result<CanopyResponse> {
		let path = request
			.uri()
			.path_and_query()
			.map_or("/", |path| path.as_str());
		let url = self
			.base
			.join(&format!("{TAILNET_MOUNT}{path}"))
			.map_err(Error::transport)?;

		let mut call = self
			.http
			.request(request.method().clone(), url)
			.headers(request.headers().clone());
		if !request.body().is_empty() {
			call = call.body(request.body().clone());
		}

		let response = call.send().await.map_err(Error::transport)?;
		let status = response.status();
		let version = response.version();
		let headers = response.headers().clone();
		let body = response.bytes().await.map_err(Error::transport)?;

		let mut out = CanopyResponse::new(body);
		*out.status_mut() = status;
		*out.version_mut() = version;
		*out.headers_mut() = headers;
		Ok(out)
	}
}

#[cfg(test)]
mod tests {
	use axum::{
		Router,
		extract::Request,
		http::{StatusCode, header},
		response::{IntoResponse, Redirect},
		routing::any,
	};
	use bes_canopy_api::{bytes::Bytes, http};

	use super::*;

	async fn serve(app: Router) -> Url {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let addr = listener.local_addr().unwrap();
		tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
		format!("http://{addr}").parse().unwrap()
	}

	fn direct(url: Url) -> TailnetTransport {
		TailnetTransport::new(TailnetEndpoint {
			url,
			socks5_proxy: None,
			resolve_to: None,
		})
		.unwrap()
	}

	fn request(method: http::Method, uri: &str, body: &'static [u8]) -> CanopyRequest {
		http::Request::builder()
			.method(method)
			.uri(uri)
			.header(header::CONTENT_TYPE, "application/json")
			.body(Bytes::from_static(body))
			.unwrap()
	}

	async fn echo(request: Request) -> impl IntoResponse {
		let (parts, body) = request.into_parts();
		let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
		format!(
			"{} {} {} {}",
			parts.method,
			parts.uri,
			parts.headers[header::CONTENT_TYPE].to_str().unwrap(),
			String::from_utf8_lossy(&body),
		)
	}

	#[tokio::test]
	async fn sends_calls_under_the_tailnet_mount_with_query_body_and_headers() {
		let url = serve(Router::new().fallback(any(echo))).await;
		let response = direct(url)
			.call(request(
				http::Method::POST,
				"/restore-worklist?a=1&b=two",
				b"{}",
			))
			.await
			.unwrap();

		assert_eq!(response.status(), StatusCode::OK);
		assert_eq!(
			response.body().as_ref(),
			b"POST /public/restore-worklist?a=1&b=two application/json {}"
		);
	}

	#[tokio::test]
	async fn hands_back_a_refusal_for_the_client_to_read() {
		let url =
			serve(Router::new().fallback(any(|| async { (StatusCode::FORBIDDEN, "refused") })))
				.await;
		let response = direct(url)
			.call(request(http::Method::GET, "/restore-worklist", b""))
			.await
			.unwrap();

		assert_eq!(response.status(), StatusCode::FORBIDDEN);
		assert_eq!(response.body().as_ref(), b"refused");
	}

	#[tokio::test]
	async fn does_not_follow_redirects() {
		let url =
			serve(Router::new().fallback(any(|| async { Redirect::temporary("/elsewhere") })))
				.await;
		let response = direct(url)
			.call(request(http::Method::GET, "/restore-worklist", b""))
			.await
			.unwrap();

		assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
	}

	#[tokio::test]
	async fn a_connection_that_cannot_be_made_is_an_error() {
		let closed = {
			let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
			format!("http://{}", listener.local_addr().unwrap())
		};
		let err = direct(closed.parse().unwrap())
			.call(request(http::Method::GET, "/restore-worklist", b""))
			.await
			.unwrap_err();

		assert!(matches!(err, Error::Transport(_)));
	}

	#[test]
	fn builds_with_a_proxy_and_with_an_address_override() {
		let endpoint = |resolve_to| TailnetEndpoint {
			url: "https://canopy.example.ts.net".parse().unwrap(),
			socks5_proxy: Some("socks5://[::1]:1055".parse().unwrap()),
			resolve_to,
		};

		TailnetTransport::new(endpoint(None)).unwrap();
		TailnetTransport::new(endpoint(Some("100.64.0.1".parse().unwrap()))).unwrap();
	}
}
