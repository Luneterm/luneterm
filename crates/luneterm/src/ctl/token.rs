//! The instance token: 16 bytes from the OS as 22 base64url characters, new for each run. It
//! names an instance (the socket, `LUNETERM_INSTANCE`); it does not authenticate anyone, the OS
//! does that at the endpoint.

use std::io;

/// The bytes of randomness in a token.
const BYTES: usize = 16;

/// The base64url alphabet (RFC 4648, section 5).
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// A new token, or the error of the OS's random source. There is no fallback to anything
/// guessable.
pub fn new() -> io::Result<String> {
    let mut bytes = [0; BYTES];
    random(&mut bytes)?;
    Ok(encode(&bytes))
}

#[cfg(unix)]
fn random(out: &mut [u8]) -> io::Result<()> {
    use std::io::Read;

    std::fs::File::open("/dev/urandom")?.read_exact(out)
}

#[cfg(not(unix))]
fn random(_out: &mut [u8]) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}

/// `bytes` in base64url without padding.
fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut group = [0; 4];
        group[1..=chunk.len()].copy_from_slice(chunk);
        let bits = u32::from_be_bytes(group);
        // n bytes make n + 1 characters: the six-bit pieces that hold any of their bits.
        for piece in 0..=chunk.len() {
            let index = (bits >> (18 - 6 * piece)) & 63;
            out.push(char::from(ALPHABET[index as usize]));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_encoding_is_base64url_without_padding() {
        // RFC 4648 section 10, without the padding.
        for (bytes, text) in [
            (&b""[..], ""),
            (b"f", "Zg"),
            (b"fo", "Zm8"),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg"),
            (b"fooba", "Zm9vYmE"),
            (b"foobar", "Zm9vYmFy"),
            // The two characters that differ from plain base64.
            (&[0xfb, 0xff][..], "-_8"),
            (&[0xff; 3], "____"),
            (&[0; 3], "AAAA"),
        ] {
            assert_eq!(encode(bytes), text, "{bytes:?}");
        }
        let counted: Vec<u8> = (0..16).collect();
        assert_eq!(encode(&counted), "AAECAwQFBgcICQoLDA0ODw");
    }

    #[cfg(unix)]
    #[test]
    fn a_new_token_is_22_url_safe_characters_and_never_the_same() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            let token = new().expect("the OS gives random bytes");
            assert_eq!(token.len(), 22);
            assert!(crate::ctl::token_ok(&token), "{token}");
            assert!(seen.insert(token));
        }
    }
}
