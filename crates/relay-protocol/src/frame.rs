//! Length-delimited framing: a four-byte big-endian length followed by that
//! many bytes of JSON.
//!
//! QUIC gives ordered, reliable delivery within a stream but no message
//! boundaries, so the length prefix supplies them. JSON because a harvest
//! filing is already a JSON body (see [`crate::filing`]), and because a
//! protocol between two deployables on separate release cycles is worth being
//! able to read off the wire.
//!
//! Framing is written against the `tokio` IO traits rather than against
//! `quinn`'s stream types, so it exercises over an in-memory pipe in tests and
//! this crate stays free of a transport dependency.

use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

/// The largest frame either end will write or accept.
///
/// A length prefix is read before the body it describes, so without a ceiling
/// a malformed or hostile length makes the reader allocate whatever it was
/// told to. Filings are small (check results and their detail); the ceiling is
/// far above anything the protocol legitimately carries and far below anything
/// that threatens the process.
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
	#[error("relay protocol IO failed: {0}")]
	Io(#[from] std::io::Error),

	#[error("relay protocol frame is not valid JSON: {0}")]
	Malformed(#[from] serde_json::Error),

	#[error("relay protocol frame of {size} bytes exceeds the {MAX_FRAME_BYTES}-byte ceiling")]
	TooLarge { size: usize },

	#[error("relay protocol stream ended mid-frame")]
	Truncated,

	#[error("relay protocol stream ended before the expected message")]
	UnexpectedEnd,
}

/// Write one message as a frame.
///
/// Does not finish the stream: a unidirectional filing stream carries one
/// frame and the caller finishes it, and a bidirectional exchange writes a
/// frame in each direction on the same stream.
pub async fn write_frame<W, T>(w: &mut W, message: &T) -> Result<(), ProtocolError>
where
	W: AsyncWrite + Unpin,
	T: Serialize + ?Sized,
{
	let body = serde_json::to_vec(message)?;
	if body.len() > MAX_FRAME_BYTES {
		return Err(ProtocolError::TooLarge { size: body.len() });
	}

	// Length and body in one write: two writes would let a reader see a
	// length whose body is still in flight, which is legal but pointless
	// churn on a stream that carries one frame.
	let mut frame = Vec::with_capacity(4 + body.len());
	frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
	frame.extend_from_slice(&body);
	w.write_all(&frame).await?;
	w.flush().await?;
	Ok(())
}

/// Read the next frame, or `None` at a clean end of stream.
///
/// `None` is how a reader of a filing stream learns the relay is done, so it
/// is a normal outcome and not an error. A stream that ends *within* a frame
/// is [`ProtocolError::Truncated`] — the distinction is the whole reason this
/// returns an option rather than erroring on EOF.
pub async fn read_frame<R, T>(r: &mut R) -> Result<Option<T>, ProtocolError>
where
	R: AsyncRead + Unpin,
	T: DeserializeOwned,
{
	let mut len = [0u8; 4];
	match r.read_exact(&mut len).await {
		Ok(_) => {}
		Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
		Err(e) => return Err(e.into()),
	}

	let size = u32::from_be_bytes(len) as usize;
	if size > MAX_FRAME_BYTES {
		return Err(ProtocolError::TooLarge { size });
	}

	let mut body = vec![0u8; size];
	match r.read_exact(&mut body).await {
		Ok(_) => {}
		Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
			return Err(ProtocolError::Truncated);
		}
		Err(e) => return Err(e.into()),
	}

	Ok(Some(serde_json::from_slice(&body)?))
}

/// Read a frame that must be there: the request on a stream canopy opened, or
/// the response to it. A clean end of stream is as much a protocol failure as
/// a truncated one here.
pub async fn read_required_frame<R, T>(r: &mut R) -> Result<T, ProtocolError>
where
	R: AsyncRead + Unpin,
	T: DeserializeOwned,
{
	read_frame(r).await?.ok_or(ProtocolError::UnexpectedEnd)
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde::Deserialize;

	#[derive(Debug, PartialEq, Serialize, Deserialize)]
	struct Msg {
		n: u32,
		s: String,
	}

	fn msg(n: u32) -> Msg {
		Msg {
			n,
			s: format!("message {n}"),
		}
	}

	#[tokio::test]
	async fn a_frame_round_trips() {
		let mut buf = Vec::new();
		write_frame(&mut buf, &msg(1)).await.unwrap();
		let read: Option<Msg> = read_frame(&mut buf.as_slice()).await.unwrap();
		assert_eq!(read, Some(msg(1)));
	}

	/// Successive frames on one stream stay separate — the property the
	/// length prefix exists for.
	#[tokio::test]
	async fn frames_do_not_run_together() {
		let mut buf = Vec::new();
		for n in 0..3 {
			write_frame(&mut buf, &msg(n)).await.unwrap();
		}

		let mut cursor = buf.as_slice();
		for n in 0..3 {
			let read: Option<Msg> = read_frame(&mut cursor).await.unwrap();
			assert_eq!(read, Some(msg(n)));
		}
		let end: Option<Msg> = read_frame(&mut cursor).await.unwrap();
		assert_eq!(end, None, "a clean end of stream is not an error");
	}

	#[tokio::test]
	async fn an_empty_stream_reads_as_a_clean_end() {
		let read: Option<Msg> = read_frame(&mut [].as_slice()).await.unwrap();
		assert_eq!(read, None);
	}

	/// The attack the ceiling exists for: a length prefix naming more memory
	/// than the protocol ever legitimately carries must be refused *before*
	/// the body is allocated, so an oversized claim costs the reader nothing.
	#[tokio::test]
	async fn an_oversized_length_prefix_is_refused_without_allocating() {
		let mut wire = (u32::MAX).to_be_bytes().to_vec();
		wire.extend_from_slice(b"not this much, actually");
		let err = read_frame::<_, Msg>(&mut wire.as_slice())
			.await
			.expect_err("must refuse");
		assert!(
			matches!(err, ProtocolError::TooLarge { size } if size == u32::MAX as usize),
			"got {err:?}",
		);
	}

	#[tokio::test]
	async fn a_stream_ending_mid_frame_is_truncated_not_a_clean_end() {
		let mut buf = Vec::new();
		write_frame(&mut buf, &msg(1)).await.unwrap();
		buf.truncate(buf.len() - 3);

		let err = read_frame::<_, Msg>(&mut buf.as_slice())
			.await
			.expect_err("must not read as a clean end");
		assert!(matches!(err, ProtocolError::Truncated), "got {err:?}");
	}

	#[tokio::test]
	async fn a_required_frame_rejects_a_clean_end() {
		let err = read_required_frame::<_, Msg>(&mut [].as_slice())
			.await
			.expect_err("a required message is not optional");
		assert!(matches!(err, ProtocolError::UnexpectedEnd), "got {err:?}");
	}

	#[tokio::test]
	async fn a_body_that_is_not_the_expected_message_is_malformed() {
		let mut buf = Vec::new();
		write_frame(&mut buf, &serde_json::json!({"unexpected": true}))
			.await
			.unwrap();
		let err = read_frame::<_, Msg>(&mut buf.as_slice())
			.await
			.expect_err("must not deserialise");
		assert!(matches!(err, ProtocolError::Malformed(_)), "got {err:?}");
	}
}
