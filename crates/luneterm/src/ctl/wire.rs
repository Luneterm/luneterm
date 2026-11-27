//! The framing. A request is `LTC1`, a u32 word count, then each word as a u32 byte length and
//! that many bytes of UTF-8. A reply is one status byte, a u32 byte length and that many bytes of
//! UTF-8: the JSON line without its newline, or the reason. Every u32 is little-endian.
//!
//! The decoders take any reader and bound what they take from it: no length is allocated before
//! it is checked against the caps, and nothing past the frame is read. A deadline is the
//! reader's to enforce.

use std::fmt;
use std::io::{self, Read};

use super::Code;

/// The first bytes of a request.
const MAGIC: [u8; 4] = *b"LTC1";
/// The magic and the word count.
const REQUEST_HEADER: usize = 8;
/// The most words in a request.
pub const MAX_WORDS: usize = 300;
/// The largest request, every byte of the frame counted.
pub const MAX_REQUEST_BYTES: usize = 128 * 1024;
/// The largest reply body.
pub const MAX_REPLY_BODY: usize = 4 * 1024 * 1024;

/// Why a frame is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireError {
    /// A request does not start with the magic.
    Magic,
    /// A count or a length is over its cap.
    OverLimit,
    /// A word or a body is not UTF-8.
    Utf8,
    /// A status byte no reply carries.
    Status,
    /// The reader failed; a frame cut short is `UnexpectedEof`.
    Read(io::ErrorKind),
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WireError::Magic => f.write_str("not a ctl request"),
            WireError::OverLimit => f.write_str("a message is over its size limit"),
            WireError::Utf8 => f.write_str("a message is not UTF-8"),
            WireError::Status => f.write_str("a reply has an unknown status"),
            WireError::Read(kind) => write!(f, "a message could not be read ({kind})"),
        }
    }
}

/// The frame that carries `words`.
pub fn encode_request(words: &[String]) -> Result<Vec<u8>, WireError> {
    let len = words.iter().fold(REQUEST_HEADER, |n, w| {
        n.saturating_add(4).saturating_add(w.len())
    });
    if words.len() > MAX_WORDS || len > MAX_REQUEST_BYTES {
        return Err(WireError::OverLimit);
    }
    let mut out = Vec::with_capacity(len);
    out.extend_from_slice(&MAGIC);
    put_len(&mut out, words.len())?;
    for word in words {
        put_len(&mut out, word.len())?;
        out.extend_from_slice(word.as_bytes());
    }
    Ok(out)
}

/// The words of the one request at the front of `reader`.
pub fn decode_request(reader: &mut impl Read) -> Result<Vec<String>, WireError> {
    if bytes::<4>(reader)? != MAGIC {
        return Err(WireError::Magic);
    }
    let count = length(reader)?;
    if count > MAX_WORDS {
        return Err(WireError::OverLimit);
    }
    // What the frame may still hold.
    let mut left = MAX_REQUEST_BYTES - REQUEST_HEADER;
    let mut words = Vec::with_capacity(count);
    for _ in 0..count {
        left = left.checked_sub(4).ok_or(WireError::OverLimit)?;
        let len = length(reader)?;
        left = left.checked_sub(len).ok_or(WireError::OverLimit)?;
        words.push(text(reader, len)?);
    }
    Ok(words)
}

/// The frame of a reply. [`Code::Unreachable`] is never sent.
pub fn encode_reply(code: Code, body: &str) -> Result<Vec<u8>, WireError> {
    let status = code as u8;
    reply_code(status)?;
    if body.len() > MAX_REPLY_BODY {
        return Err(WireError::OverLimit);
    }
    let mut out = Vec::with_capacity(5 + body.len());
    out.push(status);
    put_len(&mut out, body.len())?;
    out.extend_from_slice(body.as_bytes());
    Ok(out)
}

/// The code and body of the one reply at the front of `reader`.
pub fn decode_reply(reader: &mut impl Read) -> Result<(Code, String), WireError> {
    let [status] = bytes::<1>(reader)?;
    let code = reply_code(status)?;
    let len = length(reader)?;
    if len > MAX_REPLY_BODY {
        return Err(WireError::OverLimit);
    }
    Ok((code, text(reader, len)?))
}

/// The code a reply's status byte names.
fn reply_code(status: u8) -> Result<Code, WireError> {
    match status {
        0 => Ok(Code::Done),
        1 => Ok(Code::Failed),
        2 => Ok(Code::Usage),
        3 => Ok(Code::ControlOff),
        5 => Ok(Code::NoSuchTab),
        6 => Ok(Code::Refused),
        _ => Err(WireError::Status),
    }
}

fn put_len(out: &mut Vec<u8>, len: usize) -> Result<(), WireError> {
    let len = u32::try_from(len).map_err(|_| WireError::OverLimit)?;
    out.extend_from_slice(&len.to_le_bytes());
    Ok(())
}

fn bytes<const N: usize>(reader: &mut impl Read) -> Result<[u8; N], WireError> {
    let mut out = [0; N];
    reader
        .read_exact(&mut out)
        .map_err(|e| WireError::Read(e.kind()))?;
    Ok(out)
}

fn length(reader: &mut impl Read) -> Result<usize, WireError> {
    let len = u32::from_le_bytes(bytes::<4>(reader)?);
    usize::try_from(len).map_err(|_| WireError::OverLimit)
}

/// `len` bytes of UTF-8. The caller has checked `len` against its cap.
fn text(reader: &mut impl Read, len: usize) -> Result<String, WireError> {
    let mut out = vec![0; len];
    reader
        .read_exact(&mut out)
        .map_err(|e| WireError::Read(e.kind()))?;
    String::from_utf8(out).map_err(|_| WireError::Utf8)
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;

    use super::*;

    const SHORT: WireError = WireError::Read(ErrorKind::UnexpectedEof);

    fn owned(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_owned()).collect()
    }

    fn decode(bytes: &[u8]) -> Result<Vec<String>, WireError> {
        decode_request(&mut &bytes[..])
    }

    /// A request frame with whatever count and word lengths the test wants.
    fn raw(count: u32, words: &[(u32, &[u8])]) -> Vec<u8> {
        let mut out = b"LTC1".to_vec();
        out.extend_from_slice(&count.to_le_bytes());
        for (len, bytes) in words {
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(bytes);
        }
        out
    }

    #[test]
    fn the_request_frame_is_magic_count_and_prefixed_words() {
        let frame = encode_request(&owned(&["tab", "é"])).expect("encodes");
        assert_eq!(
            frame,
            b"LTC1\x02\0\0\0\x03\0\0\0tab\x02\0\0\0\xc3\xa9".to_vec()
        );
        assert_eq!(decode(&frame), Ok(owned(&["tab", "é"])));
    }

    #[test]
    fn requests_round_trip_and_nothing_past_the_frame_is_read() {
        for words in [
            owned(&[]),
            owned(&[""]),
            owned(&["status"]),
            owned(&["tab", "new", "--", "a b", "", "$(x)", "日本\n\0"]),
            vec![String::new(); MAX_WORDS],
        ] {
            let mut frame = encode_request(&words).expect("encodes");
            frame.extend_from_slice(b"next");
            let mut rest = &frame[..];
            assert_eq!(decode_request(&mut rest), Ok(words));
            assert_eq!(rest, b"next");
        }
    }

    #[test]
    fn a_request_is_capped_at_300_words() {
        let words = vec!["a".to_owned(); MAX_WORDS + 1];
        assert_eq!(MAX_WORDS, 300);
        assert_eq!(encode_request(&words), Err(WireError::OverLimit));
        let empty: Vec<(u32, &[u8])> = vec![(0, b""); 301];
        assert_eq!(decode(&raw(301, &empty)), Err(WireError::OverLimit));
        assert!(decode(&raw(300, &empty[..300])).is_ok());
        // Refused on the count alone: no word is there to read.
        assert_eq!(decode(&raw(301, &[])), Err(WireError::OverLimit));
        assert_eq!(decode(&raw(u32::MAX, &[])), Err(WireError::OverLimit));
    }

    #[test]
    fn a_request_is_capped_at_128_kib_of_frame() {
        assert_eq!(MAX_REQUEST_BYTES, 131_072);
        // One word: 8 of header and 4 of length.
        let most = vec!["a".repeat(131_072 - 12)];
        let frame = encode_request(&most).expect("at the cap");
        assert_eq!(frame.len(), 131_072);
        assert_eq!(decode(&frame), Ok(most));
        assert_eq!(
            encode_request(&["a".repeat(131_072 - 11)]),
            Err(WireError::OverLimit)
        );
        // A length over the cap is refused before its bytes are looked for.
        for len in [131_072 - 11, 1 << 31, u32::MAX] {
            assert_eq!(decode(&raw(1, &[(len, b"")])), Err(WireError::OverLimit));
        }
        // Two words that are each under the cap and over it together.
        let half = vec![b'a'; 70_000];
        let words: [(u32, &[u8]); 2] = [(70_000, &half), (70_000, &half)];
        assert_eq!(decode(&raw(2, &words)), Err(WireError::OverLimit));
        // No room left for even the next length.
        let full = vec![b'a'; 131_072 - 12];
        let words: [(u32, &[u8]); 2] = [(131_060, &full), (0, b"")];
        assert_eq!(decode(&raw(2, &words)), Err(WireError::OverLimit));
        // Three bytes of room: still none for a length.
        let words: [(u32, &[u8]); 2] = [(131_057, &full[..131_057]), (0, b"")];
        assert_eq!(decode(&raw(2, &words)), Err(WireError::OverLimit));
        let words: [(u32, &[u8]); 2] = [(131_056, &full[..131_056]), (0, b"")];
        assert!(decode(&raw(2, &words)).is_ok());
    }

    #[test]
    fn a_request_cut_anywhere_is_a_short_read() {
        let frame = encode_request(&owned(&["tab", "group", "1", "é"])).expect("encodes");
        for n in 0..frame.len() {
            assert_eq!(decode(&frame[..n]), Err(SHORT), "cut at {n}");
        }
        assert!(decode(&frame).is_ok());
    }

    #[test]
    fn a_bad_magic_and_non_utf8_are_refused() {
        for magic in [b"LTC2", b"ltc1", b"GET ", b"\0\0\0\0"] {
            let mut frame = raw(0, &[]);
            frame[..4].copy_from_slice(magic);
            assert_eq!(decode(&frame), Err(WireError::Magic), "{magic:?}");
        }
        for bad in [&[0xff][..], &[0xc3], &[0xed, 0xa0, 0x80], &[0xc0, 0x80]] {
            let len = u32::try_from(bad.len()).expect("small");
            assert_eq!(decode(&raw(1, &[(len, bad)])), Err(WireError::Utf8));
        }
    }

    #[test]
    fn a_readers_error_is_passed_on() {
        struct Fails(ErrorKind);
        impl Read for Fails {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(self.0.into())
            }
        }
        for kind in [ErrorKind::TimedOut, ErrorKind::WouldBlock] {
            assert_eq!(decode_request(&mut Fails(kind)), Err(WireError::Read(kind)));
            assert_eq!(decode_reply(&mut Fails(kind)), Err(WireError::Read(kind)));
        }
    }

    #[test]
    fn replies_round_trip_for_every_status_a_reply_carries() {
        for (code, status) in [
            (Code::Done, 0),
            (Code::Failed, 1),
            (Code::Usage, 2),
            (Code::ControlOff, 3),
            (Code::NoSuchTab, 5),
            (Code::Refused, 6),
        ] {
            for body in ["", "{\"id\":7}", "no such tab é"] {
                let mut frame = encode_reply(code, body).expect("encodes");
                assert_eq!(frame[0], status);
                assert_eq!(frame.len(), 5 + body.len());
                frame.push(b'x');
                let mut rest = &frame[..];
                assert_eq!(decode_reply(&mut rest), Ok((code, body.to_owned())));
                assert_eq!(rest, b"x");
            }
        }
    }

    #[test]
    fn a_reply_with_any_other_status_is_refused() {
        assert_eq!(encode_reply(Code::Unreachable, ""), Err(WireError::Status));
        let mut refused = 0;
        for status in 0..=u8::MAX {
            let frame = [status, 0, 0, 0, 0];
            let got = decode_reply(&mut &frame[..]);
            if matches!(status, 0..=3 | 5 | 6) {
                assert!(got.is_ok(), "{status}");
            } else {
                assert_eq!(got, Err(WireError::Status), "{status}");
                refused += 1;
            }
        }
        assert_eq!(refused, 250);
    }

    #[test]
    fn a_reply_body_is_capped_at_4_mib() {
        assert_eq!(MAX_REPLY_BODY, 4_194_304);
        let most = "a".repeat(4_194_304);
        let frame = encode_reply(Code::Done, &most).expect("at the cap");
        assert_eq!(decode_reply(&mut &frame[..]), Ok((Code::Done, most)));
        assert_eq!(
            encode_reply(Code::Done, &"a".repeat(4_194_305)),
            Err(WireError::OverLimit)
        );
        for len in [4_194_305u32, u32::MAX] {
            let mut frame = vec![0];
            frame.extend_from_slice(&len.to_le_bytes());
            assert_eq!(decode_reply(&mut &frame[..]), Err(WireError::OverLimit));
        }
    }

    #[test]
    fn a_short_or_non_utf8_reply_is_refused() {
        let frame = encode_reply(Code::Done, "{\"id\":7}").expect("encodes");
        for n in 0..frame.len() {
            assert_eq!(decode_reply(&mut &frame[..n]), Err(SHORT), "cut at {n}");
        }
        let bad = [0, 1, 0, 0, 0, 0xff];
        assert_eq!(decode_reply(&mut &bad[..]), Err(WireError::Utf8));
    }
}
