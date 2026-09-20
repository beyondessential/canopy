use std::{net::SocketAddr, time::Duration};

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use axum::{Router, middleware};
use axum_client_ip::{ClientIp, ClientIpSource};
use axum_server_timing::ServerTimingLayer;
use tokio::net::TcpListener;
use tower_http::{
	compression::CompressionLayer, decompression::RequestDecompressionLayer, trace::TraceLayer,
};
use tracing::Span;

pub mod acme;
pub mod artifact_store;
pub mod backup_jobs;
pub mod backup_secrets;
pub mod csr;
pub mod device_auth;
pub mod dns_provider;
pub mod headers;
pub mod health;
pub mod recovery_vault;
pub mod tailnet_directory;
pub mod tailnet_guard;
pub mod tailnet_sweeps;
pub mod tailscale_auth;

pub fn router(routes: Router<()>, client_ip_source: ClientIpSource) -> Router<()> {
	routes
		// ordering of the client ip middlewares is critical, do not change
		.layer(middleware::from_fn(ip_into_response))
		.layer(client_ip_source.into_extension())
		.layer(
			TraceLayer::new_for_http()
				.make_span_with(|request: &http::Request<_>| {
					tracing::info_span!(
						"http",
						req.version = ?request.version(),
						req.uri = %request.uri(),
						req.method = %request.method(),
						req.ip = tracing::field::Empty,
						res.version = tracing::field::Empty,
						res.status = tracing::field::Empty,
						latency = tracing::field::Empty,
					)
				})
				.on_response(
					|response: &http::Response<_>, latency: Duration, span: &Span| {
						if let Some(ip) = response.extensions().get::<ClientIp>().map(|r| &r.0) {
							span.record("req.ip", tracing::field::debug(ip));
						}

						span.record("latency", tracing::field::debug(latency));
						span.record("res.version", tracing::field::debug(response.version()));
						span.record(
							"res.status",
							tracing::field::display(response.status().as_u16()),
						);
						tracing::info!("response");
					},
				),
		)
		.layer(CompressionLayer::new())
		.layer(RequestDecompressionLayer::new())
		.layer(ServerTimingLayer::new("srv"))
		// Merged after the layers, so the probes are the one thing the client-ip
		// middleware does not see: a kubelet reaches the pod directly, with no
		// proxy to set `X-Forwarded-For`, and `RightmostXForwardedFor` refuses a
		// request that carries none.
		.merge(health::routes())
}

/// Content-Encodings the [`RequestDecompressionLayer`] applied in [`router`]
/// transparently decodes on request bodies (courtesy of tower-http's
/// `decompression-full` feature). Advertised in both OpenAPI specs via
/// [`request_compression_extension`] so clients know large request bodies
/// (status pushes, backup reports, event batches) may be — and are encouraged
/// to be — sent compressed.
pub const ACCEPTED_REQUEST_ENCODINGS: &[&str] = &["gzip", "br", "deflate", "zstd"];

/// Value of the `x-request-compression` OpenAPI vendor extension. Built here,
/// beside the decompression layer, so the advertised encodings can't drift from
/// what the server actually accepts.
pub fn request_compression_extension() -> serde_json::Value {
	serde_json::json!({
		"accepted": ACCEPTED_REQUEST_ENCODINGS,
		"recommended": true,
		"header": "Content-Encoding",
	})
}

pub async fn serve(routes: Router<()>, addr: SocketAddr) -> commons_errors::Result<()> {
	let service = routes.into_make_service_with_connect_info::<SocketAddr>();
	let listener = TcpListener::bind(addr).await?;
	tracing::info!("listening on {}", listener.local_addr()?);
	axum::serve(listener, service).await?;
	Ok(())
}

async fn ip_into_response(ip: ClientIp, request: Request, next: Next) -> Response {
	tracing::trace!(?ip, "ip_into_response middleware");
	let mut response = next.run(request).await;
	response.extensions_mut().insert(ip);
	response
}

#[cfg(test)]
mod tests {
	use axum_test::TestServer;

	use super::*;

	/// A kubelet probes the pod directly, so its request carries no
	/// `X-Forwarded-For` for `RightmostXForwardedFor` to read. Behind the layer
	/// the extraction refuses it, and a startup probe that can never pass kills
	/// every pod it is attached to.
	#[tokio::test]
	async fn a_probe_without_a_forwarded_header_is_answered() {
		let app = router(Router::new(), ClientIpSource::RightmostXForwardedFor);
		let server = TestServer::new(app);

		server.get("/livez").await.assert_status_ok();
		server.get("/healthz").await.assert_status_ok();
	}
}
