//! The key service returns ECDSA signatures DER-encoded, as a `SEQUENCE` of two
//! `INTEGER`s; a JWS carries them as the two 32-byte big-endian values side by
//! side (RFC 7518 §3.4).

use crate::{Error, Result};

/// Convert a DER ECDSA P-256 signature to its JOSE form.
pub(crate) fn ecdsa_der_to_jose(der: &[u8]) -> Result<[u8; 64]> {
	let (body, rest) = read_tlv(der, 0x30)?;
	if !rest.is_empty() {
		return Err(Error::MalformedSignature);
	}
	let (r, body) = read_tlv(body, 0x02)?;
	let (s, body) = read_tlv(body, 0x02)?;
	if !body.is_empty() {
		return Err(Error::MalformedSignature);
	}
	let mut out = [0u8; 64];
	put_scalar(r, &mut out[..32])?;
	put_scalar(s, &mut out[32..])?;
	Ok(out)
}

/// Read one DER element of the expected tag, returning its contents and what
/// follows it. Lengths beyond one byte of long form never occur in a P-256
/// signature, so they are refused rather than handled.
fn read_tlv(input: &[u8], tag: u8) -> Result<(&[u8], &[u8])> {
	let [found, len, rest @ ..] = input else {
		return Err(Error::MalformedSignature);
	};
	if *found != tag {
		return Err(Error::MalformedSignature);
	}
	let (len, rest) = match *len {
		len @ 0..=0x7f => (len as usize, rest),
		0x81 => match rest {
			[len, rest @ ..] => (*len as usize, rest),
			[] => return Err(Error::MalformedSignature),
		},
		_ => return Err(Error::MalformedSignature),
	};
	if rest.len() < len {
		return Err(Error::MalformedSignature);
	}
	Ok(rest.split_at(len))
}

/// Write a DER integer, which may carry a leading zero for its sign and may be
/// shorter than 32 bytes, as exactly 32 big-endian bytes.
fn put_scalar(int: &[u8], out: &mut [u8]) -> Result<()> {
	let int = match int {
		[0, rest @ ..] if !rest.is_empty() => rest,
		int => int,
	};
	if int.len() > out.len() || int.is_empty() {
		return Err(Error::MalformedSignature);
	}
	let pad = out.len() - int.len();
	out[pad..].copy_from_slice(int);
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	fn der(r: &[u8], s: &[u8]) -> Vec<u8> {
		let mut body = vec![0x02, r.len() as u8];
		body.extend_from_slice(r);
		body.extend_from_slice(&[0x02, s.len() as u8]);
		body.extend_from_slice(s);
		let mut out = vec![0x30, body.len() as u8];
		out.extend(body);
		out
	}

	#[test]
	fn full_width_with_sign_byte() {
		let mut r = vec![0u8];
		r.extend([0xffu8; 32]);
		let s = [0x11u8; 32];
		let jose = ecdsa_der_to_jose(&der(&r, &s)).unwrap();
		assert_eq!(&jose[..32], &[0xff; 32]);
		assert_eq!(&jose[32..], &[0x11; 32]);
	}

	#[test]
	fn short_integers_are_left_padded() {
		let jose = ecdsa_der_to_jose(&der(&[0x01, 0x02], &[0x03])).unwrap();
		assert_eq!(&jose[30..32], &[0x01, 0x02]);
		assert!(jose[..30].iter().all(|b| *b == 0));
		assert_eq!(jose[63], 0x03);
		assert!(jose[32..63].iter().all(|b| *b == 0));
	}

	#[test]
	fn malformed_is_refused() {
		assert!(ecdsa_der_to_jose(&[]).is_err());
		assert!(ecdsa_der_to_jose(&[0x31, 0x00]).is_err());
		let mut trailing = der(&[1], &[2]);
		trailing.push(0);
		assert!(ecdsa_der_to_jose(&trailing).is_err());
		assert!(ecdsa_der_to_jose(&der(&[1; 33], &[2])).is_err());
	}
}
