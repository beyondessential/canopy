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

	/// A value placed in the path carried a character that would change which
	/// request is made.
	#[error(
		"the {name} of {path} is {value:?}, which carries a URI delimiter and would change which \
		 request this is"
	)]
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
/// body rather than flattening them into a message.
#[derive(Debug, thiserror::Error)]
#[error("canopy returned {status} for {path}")]
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
}
