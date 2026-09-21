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

	/// Send a request whose body is bytes the caller holds rather than JSON, and
	/// parse a JSON response body into `R`.
	///
	/// Reached only by the generated methods, which are in this crate: a consumer
	/// calls the method for the operation it wants. Keeping it in the crate is
	/// what lets the media type stay a `&'static str` — the header value is built
	/// from it without checking, which is safe for the handful of media types the
	/// generator emits and would not be for a string a consumer composed.
	pub(crate) async fn call_payload_json<R: DeserializeOwned>(
		&self,
		method: http::Method,
		path: &str,
		payload: Option<Bytes>,
		content_type: &'static str,
	) -> Result<R> {
		let response = self
			.call_payload(method, path, payload, content_type)
			.await?;
		serde_json::from_slice(response.body()).map_err(|source| Error::Decode {
			path: path.to_owned(),
			source,
		})
	}

	/// Send a request whose body is bytes the caller holds, discarding the
	/// response body.
	pub(crate) async fn call_payload_empty(
		&self,
		method: http::Method,
		path: &str,
		payload: Option<Bytes>,
		content_type: &'static str,
	) -> Result<()> {
		self.call_payload(method, path, payload, content_type)
			.await
			.map(|_| ())
	}

	/// Send `payload` as the request body, under `content_type`.
	///
	/// The bytes go up as they are, rather than gzipped like a JSON body: canopy
	/// records the digest of what it receives, so leaving them alone keeps what
	/// canopy hashes identical to what the caller passed, without depending on
	/// anything in between to undo a compression first.
	async fn call_payload(
		&self,
		method: http::Method,
		path: &str,
		payload: Option<Bytes>,
		content_type: &'static str,
	) -> Result<http::Response<Bytes>> {
		// Whether there is a body at all is settled where the method is
		// generated, not guessed from the bytes: a method whose envelope carries
		// no body sends none and declares no content type, which is what its
		// published signature always sent, while a body that is present and empty
		// is still a body and still says what it is.
		let (payload, content_type) = match payload {
			Some(payload) => (payload, Some(http::HeaderValue::from_static(content_type))),
			None => (Bytes::new(), None),
		};
		self.send(method, path, payload, content_type, None).await
	}

	/// Send a request, returning the response only if the status is a success.
	async fn call<B: Serialize + ?Sized>(
		&self,
		method: http::Method,
		path: &str,
		body: Option<&B>,
	) -> Result<http::Response<Bytes>> {
		let (payload, content_type, content_encoding) = match body {
			None => (Bytes::new(), None, None),
			Some(body) => {
				let json = serde_json::to_vec(body).map_err(|source| Error::Encode {
					path: path.to_owned(),
					source,
				})?;
				let content_type = Some(http::HeaderValue::from_static("application/json"));
				if json.len() >= COMPRESS_FROM {
					let gzipped = gzip(&json).map_err(|source| Error::Compress {
						path: path.to_owned(),
						source,
					})?;
					(
						Bytes::from(gzipped),
						content_type,
						Some(http::HeaderValue::from_static("gzip")),
					)
				} else {
					(Bytes::from(json), content_type, None)
				}
			}
		};

		self.send(method, path, payload, content_type, content_encoding)
			.await
	}

	/// Put a request on the wire and judge its status.
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
		if let Some(content_encoding) = content_encoding {
			request = request.header(http::header::CONTENT_ENCODING, content_encoding);
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
