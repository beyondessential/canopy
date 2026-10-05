//! Errors this client produces.

use bytes::Bytes;

/// Result alias used throughout the crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Anything that can go wrong calling canopy.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
	/// Canopy answered with a status outside the success range.
	#[error(transparent)]
	Http(#[from] CanopyHttpError),

	/// The response body was not the JSON the endpoint declares.
	#[error("decoding the response of {path}: {source}")]
	Decode {
		/// Path that was called.
		path: String,
		/// Underlying serde error.
		source: serde_json::Error,
	},

	/// The request body could not be serialised.
	#[error("encoding the request body for {path}: {source}")]
	Encode {
		/// Path that was called.
		path: String,
		/// Underlying serde error.
		source: serde_json::Error,
	},

	/// A value placed in the path would have changed which request is made.
	#[error("the {name} of {path} is {value:?}, which would change which request this is")]
	PathValue {
		/// Template of the path that was being built.
		path: String,
		/// Name of the path parameter the value was given for.
		name: String,
		/// The value, as it was given.
		value: String,
	},

	/// Building the HTTP request failed.
	#[error("building the request for {path}: {source}")]
	Request {
		/// Path that was called.
		path: String,
		/// Underlying `http` error.
		source: http::Error,
	},

	/// Compressing the request body failed.
	#[error("compressing the request body for {path}: {source}")]
	Compress {
		/// Path that was called.
		path: String,
		/// Underlying IO error.
		source: std::io::Error,
	},

	/// The transport could not obtain any response.
	///
	/// Distinct from [`Error::Http`], which is a response that reports failure.
	#[error("reaching canopy: {0}")]
	Transport(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl Error {
	/// Wrap a transport-side failure to obtain a response.
	pub fn transport<E: std::error::Error + Send + Sync + 'static>(err: E) -> Self {
		Self::Transport(Box::new(err))
	}

	/// The [`CanopyHttpError`] this error carries, if it is one.
	///
	/// Endpoints give particular statuses meaning, so a caller branching on a
	/// documented status reads it through here.
	pub fn http(&self) -> Option<&CanopyHttpError> {
		match self {
			Self::Http(err) => Some(err),
			_ => None,
		}
	}

	/// The status canopy answered with, if this is an unsuccessful response.
	pub fn status(&self) -> Option<http::StatusCode> {
		self.http().map(|err| err.status)
	}
}

/// A non-2xx response from a canopy endpoint.
///
/// Endpoints give meaning to specific codes, so this carries the status and the
/// body rather than flattening them into a message. The message does include
/// the reason canopy gave for a refusal whose body is a problem document, so a
/// consumer that reports the error as text says why the request was refused.
// spec: APIC#the-consumer-supplies-the-transport
#[derive(Debug, thiserror::Error)]
#[error("canopy returned {status} for {path}{}", self.reason().map(|r| format!(": {r}")).unwrap_or_default())]
pub struct CanopyHttpError {
	/// HTTP status returned by canopy.
	pub status: http::StatusCode,
	/// The endpoint path that was called.
	pub path: String,
	/// Response body, as returned.
	pub body: Bytes,
}

impl CanopyHttpError {
	/// The response body as UTF-8 text, lossily.
	pub fn body_text(&self) -> std::borrow::Cow<'_, str> {
		String::from_utf8_lossy(&self.body)
	}

	/// The reason canopy gave for refusing the request, where the body is a
	/// problem document.
	///
	/// Canopy puts the occurrence's own message in the document's title, so that
	/// is what this reads, falling back to the detail for a document without one.
	/// Only a refusal (a 4xx) has a reason: a server fault's message describes
	/// canopy's internals, which stay with canopy.
	pub fn reason(&self) -> Option<String> {
		if !self.status.is_client_error() {
			return None;
		}
		let document: serde_json::Value = serde_json::from_slice(&self.body).ok()?;
		["title", "detail"].iter().find_map(|key| {
			let reason = document.get(key)?.as_str()?.trim();
			(!reason.is_empty()).then(|| reason.to_owned())
		})
	}
}
