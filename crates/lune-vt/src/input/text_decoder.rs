//! Port of xterm.js src/common/input/TextDecoder.ts @ f447274.
//!
//! `Utf8ToUtf32` (streaming UTF-8 → UTF-32 code points), plus the two string helpers.
//!
//! Rust notes (see `docs/porting-xterm.md`):
//!  - `StringToUtf32` (UTF-16 → UTF-32) is **replaced**: Rust input is bytes or `&str`, and
//!    `&str` goes through `Utf8ToUtf32` (plan decision D5). Its upstream tests are not ported.
//!  - The caller-sized `Uint32Array` target becomes `&mut Vec<u32>`; `decode` clears it and
//!    returns the number of code points written, so there is no "target too small" case.
//!  - `clear()` is [`Utf8ToUtf32::reset`].
//!  - The three near-identical 2/3/4-byte branches of the main loop are one loop over the
//!    continuation bytes. The decisions (when a lead byte is held back, when a bad continuation
//!    byte is re-read, which code points are dropped) are the same ones, in the same order.
//!  - Typed-array reads/writes use `get` / `get_mut`; nothing here indexes.
//!
//! # Split-sequence fix (deliberate deviation; plan M4 decision D4, ledger 3)
//!
//! Upstream stores a held-back partial sequence in `interim: Uint8Array(3)` and, on the next
//! chunk, recovers how many bytes it holds by scanning for the first entry whose low six bits
//! are zero (`while ((tmp = this.interim[++pos] & 0x3F) && pos < 4)`). A continuation byte
//! `0x80` has low six bits zero, so `E2 80 | 99` looks like "lead only", the decoder reads two
//! more bytes instead of one, and the U+2019 is lost (and a following character with it).
//! Lunegit decoded in C#, so it never hit this; this crate must not inherit it. The fix keeps an
//! explicit byte count (`interim_len`). Whole-buffer decoding (no interim state ever used) is
//! byte-for-byte upstream; only the resume path differs, and only where upstream was wrong.
//! Invariant: for every byte sequence and every split into chunks, the concatenated output
//! equals the whole-buffer output.

/// Convert one UTF-32 code point to a `String`.
///
/// Upstream builds UTF-16 surrogate pairs by hand. Rust strings hold scalar values only, so
/// values that are not Unicode scalars (lone surrogates, above 0x10FFFF) become U+FFFD.
// LUNETERM: replacement character for non-scalars; the decoder below can never produce one.
pub fn string_from_code_point(code_point: u32) -> String {
    char::from_u32(code_point)
        .unwrap_or(char::REPLACEMENT_CHARACTER)
        .to_string()
}

/// Convert UTF-32 code points to a `String` (upstream `utf32ToString(data)`; take a sub-slice
/// for its `start` / `end` arguments). Non-scalar values become U+FFFD, see
/// [`string_from_code_point`]. Never panics.
pub fn utf32_to_string(data: &[u32]) -> String {
    data.iter()
        .map(|&cp| char::from_u32(cp).unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

/// Bytes a partial sequence can be held in: the lead byte plus two continuation bytes. (A
/// sequence missing only its last byte has at most `4 - 1 = 3` bytes.)
const INTERIM_CAPACITY: usize = 3;

/// Streaming UTF-8 decoder: UTF-8 byte sequences in, UTF-32 code points out.
#[derive(Clone, Debug, Default)]
pub struct Utf8ToUtf32 {
    /// Held-back bytes of a sequence split across chunks (lead byte first).
    interim: [u8; INTERIM_CAPACITY],
    /// How many of `interim` are in use (upstream infers it from the bytes).
    // LUNETERM: explicit count instead of the `& 0x3F` sentinel scan, see the module docs (D4).
    interim_len: usize,
}

/// Number of bytes in the sequence a lead byte starts (upstream's `type`). Only called for
/// bytes the main loop classified as 2/3/4-byte leads.
fn sequence_len(lead: u8) -> usize {
    if lead & 0xE0 == 0xC0 {
        2
    } else if lead & 0xF0 == 0xE0 {
        3
    } else {
        4
    }
}

/// Payload bits of a lead byte.
fn lead_bits(lead: u8) -> u32 {
    let mask = if lead & 0xE0 == 0xC0 {
        0x1F
    } else if lead & 0xF0 == 0xE0 {
        0x0F
    } else {
        0x07
    };
    u32::from(lead & mask)
}

/// Upstream's final test, type dependent: is `cp` a legal code point for a `ty`-byte sequence
/// (no overlong form, no surrogate, no BOM, within Unicode)?
fn is_legal(ty: usize, cp: u32) -> bool {
    match ty {
        2 => cp >= 0x80,
        3 => !(cp < 0x0800 || (0xD800..=0xDFFF).contains(&cp) || cp == 0xFEFF),
        _ => (0x01_0000..=0x10_FFFF).contains(&cp),
    }
}

impl Utf8ToUtf32 {
    pub fn new() -> Self {
        Self::default()
    }

    /// Clears interim bytes and resets the decoder to a clean state (upstream `clear()`).
    pub fn reset(&mut self) {
        self.interim = [0; INTERIM_CAPACITY];
        self.interim_len = 0;
    }

    /// The bytes of an incomplete sequence held back for the next chunk (lead byte first;
    /// empty when the decoder is clean).
    pub fn interim(&self) -> &[u8] {
        self.interim.get(..self.interim_len).unwrap_or(&[])
    }

    /// Hold back `lead` plus `conts` for the next chunk.
    fn save(&mut self, lead: u8, conts: &[u8]) {
        self.reset();
        let mut len = 0;
        for (slot, &b) in self
            .interim
            .iter_mut()
            .zip(std::iter::once(&lead).chain(conts))
        {
            *slot = b;
            len += 1;
        }
        self.interim_len = len;
    }

    /// Decodes the UTF-8 byte sequences in `input` into UTF-32 code points in `target`.
    ///
    /// Assumes stream input: a sequence cut off at the end of `input` is held back and
    /// completed by the next call. `target` is cleared first. Returns the number of code points
    /// written.
    pub fn decode(&mut self, input: &[u8], target: &mut Vec<u32>) -> usize {
        target.clear();
        let length = input.len();
        if length == 0 {
            return 0;
        }

        let mut start_pos = 0usize;

        // Handle leftover bytes.
        if self.interim_len > 0 {
            let lead = self.interim.first().copied().unwrap_or(0);
            let ty = sequence_len(lead);
            let mut cp = lead_bits(lead);
            for &b in self.interim.iter().take(self.interim_len).skip(1) {
                cp = (cp << 6) | u32::from(b & 0x3F);
            }
            // LUNETERM: `pos` is the explicit interim count (D4); upstream scans for a zero byte.
            let mut pos = self.interim_len;
            let mut discard_interim = false;
            // Missing bytes - read ahead from input.
            let missing = ty.saturating_sub(pos);
            while start_pos < missing {
                let Some(&tmp) = input.get(start_pos) else {
                    // Input too short to finish the sequence: keep what was saved.
                    return 0;
                };
                start_pos += 1;
                if tmp & 0xC0 != 0x80 {
                    // Wrong continuation, discard interim bytes completely.
                    start_pos -= 1;
                    discard_interim = true;
                    break;
                }
                // Need to save so we can continue short inputs in the next call.
                if let Some(slot) = self.interim.get_mut(pos) {
                    *slot = tmp;
                    self.interim_len = pos + 1;
                }
                pos += 1;
                cp = (cp << 6) | u32::from(tmp & 0x3F);
            }
            if !discard_interim {
                if is_legal(ty, cp) {
                    target.push(cp);
                } else if ty == 2 {
                    // Wrong starter byte: the byte just read is re-read (and skipped).
                    start_pos = start_pos.saturating_sub(1);
                }
                // 3/4 byte: illegal code point or BOM, dropped.
            }
            self.reset();
        }

        // Loop through input.
        let mut i = start_pos;
        'outer: while i < length {
            // ASCII shortcut: four consecutive ASCII bytes at a time (pure speed, no effect on
            // the output).
            while i + 4 < length {
                match input.get(i..i + 4) {
                    Some(&[b1, b2, b3, b4]) if (b1 | b2 | b3 | b4) & 0x80 == 0 => {
                        target.extend([u32::from(b1), u32::from(b2), u32::from(b3), u32::from(b4)]);
                        i += 4;
                    }
                    _ => break,
                }
            }

            // Reread byte1.
            let Some(&byte1) = input.get(i) else {
                break;
            };
            i += 1;

            if byte1 < 0x80 {
                // 1 byte
                target.push(u32::from(byte1));
            } else if byte1 & 0xE0 == 0xC0 || byte1 & 0xF0 == 0xE0 || byte1 & 0xF8 == 0xF0 {
                // 2, 3 or 4 bytes
                let ty = if byte1 & 0xE0 == 0xC0 {
                    2
                } else if byte1 & 0xF0 == 0xE0 {
                    3
                } else {
                    4
                };
                let mut cp = lead_bits(byte1);
                let mut conts = [0u8; INTERIM_CAPACITY];
                for got in 0..ty - 1 {
                    let Some(&b) = input.get(i) else {
                        // Chunk ends inside the sequence: hold back what was read.
                        self.save(byte1, conts.get(..got).unwrap_or(&[]));
                        return target.len();
                    };
                    i += 1;
                    if b & 0xC0 != 0x80 {
                        // Wrong continuation: drop the partial sequence, re-read this byte.
                        i -= 1;
                        continue 'outer;
                    }
                    if let Some(slot) = conts.get_mut(got) {
                        *slot = b;
                    }
                    cp = (cp << 6) | u32::from(b & 0x3F);
                }
                if is_legal(ty, cp) {
                    target.push(cp);
                } else if ty == 2 {
                    // Wrong starter byte: re-read the continuation byte (which is skipped).
                    i -= 1;
                }
                // 3/4 byte: illegal code point or BOM, no `i -= 1` here.
            }
            // else: illegal byte, just skip
        }
        target.len()
    }
}

#[cfg(test)]
#[path = "text_decoder_tests.rs"]
mod tests;
