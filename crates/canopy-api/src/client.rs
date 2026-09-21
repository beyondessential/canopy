//! The typed client the generated per-endpoint methods hang off.

use std::io::Write as _;

use bytes::Bytes;
use flate2::{Compression, write::GzEncoder};
use serde::{Serialize, de::DeserializeOwned};

use crate::{CanopyHttpError, Error, Result, transport::CanopyTransport};

/// Bodies at or above this size are gzipped. Below it the compression costs
/// more than the transfer saves.
const COMPRESS_FROM: usize = 1024;

/// Typed client for canopy's public API.
///
/// Carries one method per endpoint, generated from canopy's OpenAPI document and
/// taking and returning the wire types declared there. Those methods handle the
/// parts that don't vary by endpoint (serialising and gzipping the request body,
/// mapping a non-2xx to [`CanopyHttpError`], parsing the response) and hand the
/// actual HTTP to a [`CanopyTransport`].
///
/// The transport is the consumer's: this crate depends on no HTTP client, and
/// every generated method works over whichever transport is supplied.
pub struct CanopyClient<T> {
	transport: T,
}

impl<T> std::fmt::Debug for CanopyClient<T> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("CanopyClient").finish_non_exhaustive()
	}
}

impl<T: CanopyTransport> CanopyClient<T> {
	/// Build a client over `transport`.
	pub fn new(transport: T) -> Self {
		Self { transport }
	}

	/// The transport underneath, for a consumer that needs to inspect or
	/// refresh its own.
	pub fn transport(&self) -> &T {
		&self.transport
	}

	/// Send a request and parse a JSON response body into `R`.
	///
	/// Used by the generated methods for operations that answer with a body.
	pub async fn call_json<B: Serialize + ?Sized, R: DeserializeOwned>(
		&self,
		method: http::Method,
		path: &str,
		body: Option<&B>,
	) -> Result<R> {
		let response = self.call(method, path, body).await?;
		serde_json::from_slice(response.body()).map_err(|source| Error::Decode {
			path: path.to_owned(),
			source,
		})
	}

	/// Send a request that answers with no body.
	///
	/// Used by the generated methods for operations that declare no response
	/// body. The body canopy sends, if any, is discarded.
	pub async fn call_empty<B: Serialize + ?Sized>(
		&self,
		method: http::Method,
		path: &str,
		body: Option<&B>,
	) -> Result<()> {
		self.call(method, path, body).await.map(|_| ())
	}

	/// Send a request whose body is bytes canopy holds rather than JSON, and
	/// parse a JSON response body into `R`.
	///
	/// Used by the generated methods for operations that take an
	/// `application/octet-stream` body. The bytes go up as they are: canopy
	/// records their digest, so anything done to them here would have to be
	/// undone before it could.
	pub async fn call_octets<R: DeserializeOwned>(
		&self,
		method: http::Method,
		path: &str,
		body: Bytes,
	) -> Result<R> {
		let response = self
			.send(
				method,
				path,
				body,
				Some(http::HeaderValue::from_static("application/octet-stream")),
				None,
			)
			.await?;
		serde_json::from_slice(response.body()).map_err(|source| Error::Decode {
			path: path.to_owned(),
			source,
		})
	}

	/// Send a request, returning the response only if the status is a success.
	async fn call<B: Serialize + ?Sized>(
		&self,
		method: http::Method,
		path: &str,
		body: Option<&B>,
	) -> Result<http::Response<Bytes>> {
		let (payload, content_type, encoding) = match body {
			None => (Bytes::new(), None, None),
			Some(body) => {
				let json = serde_json::to_vec(body).map_err(|source| Error::Encode {
					path: path.to_owned(),
					source,
				})?;
				let content_type = Some(http::HeaderValue::from_static("application/json"));
				if json.len() >= COMPRESS_FROM {
					(
						Bytes::from(gzip(&json).map_err(|source| Error::Compress {
							path: path.to_owned(),
							source,
						})?),
						content_type,
						Some(http::HeaderValue::from_static("gzip")),
					)
				} else {
					(Bytes::from(json), content_type, None)
				}
			}
		};

		self.send(method, path, payload, content_type, encoding).await
	}

	/// Send `payload` as it is, returning the response only if the status is a
	/// success.
	async fn send(
		&self,
		method: http::Method,
		path: &str,
		payload: Bytes,
		content_type: Option<http::HeaderValue>,
		content_encoding: Option<http::HeaderValue>,
	) -> Result<http::Response<Bytes>> {
		let mut request = http::Request::builder().method(method).uri(path);
		if let Some(content_type) = content_type {
			request = request.header(http::header::CONTENT_TYPE, content_type);
		}
		if let Some(encoding) = content_encoding {
			request = request.header(http::header::CONTENT_ENCODING, encoding);
		}

		let request = request.body(payload).map_err(|source| Error::Request {
			path: path.to_owned(),
			source,
		})?;

		let response = self.transport.call(request).await?;
		if response.status().is_success() {
			Ok(response)
		} else {
			Err(CanopyHttpError {
				status: response.status(),
				path: path.to_owned(),
				body: response.into_body(),
			}
			.into())
		}
	}
}

fn gzip(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
	let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
	encoder.write_all(bytes)?;
	encoder.finish()
}
