//! Character reading like UTF-8 `System.IO.BinaryReader` (`PeekChar`, `ReadChar`, `ReadChars`) and unforge's
//! NUL-terminated string reading, which counts characters then steps back by as many bytes.
//!
//! The decoder is .NET's: replacement by maximal subpart, state (pending bytes) kept from one call to the
//! next and across `Seek`s. `PeekChar`/`ReadChar` feed it one byte at a time with a single-character
//! buffer; `ReadChars` feeds it as many bytes as missing characters (one fewer if it has a pending byte).
//! Producing more characters than the buffer holds raises an ArgumentException in the reference.

use crate::{CryXmlError, Result};

const REPLACEMENT: u16 = 0xFFFD;
/// Maximum size of a byte read in `ReadChars` (.NET's `MaxCharBytesSize`).
const MAX_CHAR_BYTES: usize = 128;

/// UTF-16 units produced by one byte: none, one, or two (replacement then character, or a pair).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Units {
    buf: [u16; 2],
    len: usize,
}

impl Units {
    const NONE: Self = Self {
        buf: [0; 2],
        len: 0,
    };

    fn push(mut self, unit: u16) -> Self {
        self.buf[self.len] = unit;
        self.len += 1;
        self
    }

    fn as_slice(&self) -> &[u16] {
        &self.buf[..self.len]
    }
}

/// Stateful UTF-8 decoder: the lead byte and continuations already received of an unfinished sequence.
#[derive(Debug, Default, Clone, Copy)]
struct Utf8Decoder {
    pending: [u8; 3],
    len: usize,
}

/// Total length of a sequence from its lead byte (C2..F4).
fn sequence_len(lead: u8) -> usize {
    match lead {
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

/// Continuation byte valid at this place (the second is bounded to exclude overlong forms and surrogates).
fn continues(lead: u8, place: usize, byte: u8) -> bool {
    let range = match (place, lead) {
        (1, 0xE0) => 0xA0..=0xBF,
        (1, 0xED) => 0x80..=0x9F,
        (1, 0xF0) => 0x90..=0xBF,
        (1, 0xF4) => 0x80..=0x8F,
        _ => 0x80..=0xBF,
    };
    range.contains(&byte)
}

impl Utf8Decoder {
    fn has_state(&self) -> bool {
        self.len > 0
    }

    fn push(&mut self, byte: u8) -> Units {
        let mut out = Units::NONE;
        if self.len > 0 {
            let lead = self.pending[0];
            if continues(lead, self.len, byte) {
                if self.len + 1 < sequence_len(lead) {
                    self.pending[self.len] = byte;
                    self.len += 1;
                    return out;
                }
                let mask = [0, 0, 0x1F, 0x0F, 0x07][sequence_len(lead)];
                let cp = self.pending[1..self.len]
                    .iter()
                    .chain([byte].iter())
                    .fold(u32::from(lead & mask), |cp, &c| {
                        cp << 6 | u32::from(c & 0x3F)
                    });
                self.len = 0;
                let mut units = [0u16; 2];
                let c = char::from_u32(cp).unwrap_or(char::REPLACEMENT_CHARACTER);
                return c
                    .encode_utf16(&mut units)
                    .iter()
                    .fold(out, |o, &u| o.push(u));
            }
            // Broken sequence: replaced, then the byte restarts from an empty state.
            self.len = 0;
            out = out.push(REPLACEMENT);
        }
        match byte {
            0x00..=0x7F => out.push(u16::from(byte)),
            0xC2..=0xF4 => {
                self.pending[0] = byte;
                self.len = 1;
                out
            }
            _ => out.push(REPLACEMENT),
        }
    }
}

/// A UTF-8 `BinaryReader` over a byte array: position and decoder state.
#[derive(Debug, Clone)]
pub(crate) struct CharReader<'a> {
    b: &'a [u8],
    pub pos: usize,
    decoder: Utf8Decoder,
}

impl<'a> CharReader<'a> {
    pub(crate) fn new(b: &'a [u8], pos: usize) -> Self {
        Self {
            b,
            pos,
            decoder: Utf8Decoder::default(),
        }
    }

    pub(crate) fn bytes(&self) -> &'a [u8] {
        self.b
    }

    /// Reads one character into a one-unit buffer; `None` for -1 (end of file).
    fn read_one(&mut self) -> Result<Option<u16>> {
        while let Some(&byte) = self.b.get(self.pos) {
            self.pos += 1;
            match *self.decoder.push(byte).as_slice() {
                [] => {}
                [u] => return Ok(Some(u)),
                [hi, _] if (0xD800..0xDC00).contains(&hi) => {
                    return Err(CryXmlError::Supplementary);
                }
                _ => return Err(CryXmlError::CharOverflow),
            }
        }
        Ok(None)
    }

    /// `PeekChar`: position restored, decoder state not (like .NET).
    pub(crate) fn peek_char(&mut self) -> Result<Option<u16>> {
        let pos = self.pos;
        let unit = self.read_one();
        self.pos = pos;
        unit
    }

    /// `ReadChar`: -1 becomes an end of file.
    pub(crate) fn read_char(&mut self) -> Result<u16> {
        self.read_one()?.ok_or(CryXmlError::Eof)
    }

    /// `ReadChars(count)`: at most `count` units, fewer at the end of file.
    pub(crate) fn read_chars(&mut self, count: usize) -> Result<Vec<u16>> {
        let mut out = Vec::with_capacity(count.min(self.b.len().saturating_sub(self.pos) + 1));
        while out.len() < count {
            let room = count - out.len();
            let wanted = if room > 1 && self.decoder.has_state() {
                room - 1
            } else {
                room
            };
            let n = wanted
                .min(MAX_CHAR_BYTES)
                .min(self.b.len().saturating_sub(self.pos));
            if n == 0 {
                break;
            }
            let before = out.len();
            for &byte in &self.b[self.pos..self.pos + n] {
                out.extend_from_slice(self.decoder.push(byte).as_slice());
            }
            self.pos += n;
            if out.len() - before > room {
                return Err(CryXmlError::CharOverflow);
            }
        }
        Ok(out)
    }

    /// unforge's `ReadCString`. Identical to the text for NUL-terminated ASCII; shifted for a multi-byte
    /// string or one without a final NUL, like the reference. A lone surrogate (cut in the middle of a
    /// 4-byte character, forged input only) becomes U+FFFD here whereas .NET keeps it.
    pub(crate) fn read_cstring(&mut self) -> Result<String> {
        let mut count = 0usize;
        while self.pos < self.b.len() {
            if self.read_char()? == 0 {
                break;
            }
            count += 1;
        }
        let nul = self.pos;
        self.pos = nul
            .checked_sub(count + 1)
            .ok_or(CryXmlError::NegativeOffset)?;
        let units = self.read_chars(count + 1)?;
        self.pos = nul;
        let kept = &units[..count.min(units.len())];
        Ok(String::from_utf16_lossy(kept)
            .chars()
            .filter(|&c| c != '\0')
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cstring(b: &[u8], pos: usize) -> Result<(String, usize)> {
        let mut r = CharReader::new(b, pos);
        let s = r.read_cstring()?;
        Ok((s, r.pos))
    }

    fn one(b: &[u8]) -> Result<u16> {
        CharReader::new(b, 0).read_char()
    }

    fn chars(b: &[u8], count: usize) -> Result<Vec<u16>> {
        CharReader::new(b, 0).read_chars(count)
    }

    #[test]
    fn ascii_strings_read_back_and_stop_after_the_nul() {
        assert_eq!(cstring(b"abc\0def\0", 0).unwrap(), ("abc".into(), 4));
        assert_eq!(cstring(b"abc\0def\0", 4).unwrap(), ("def".into(), 8));
        assert_eq!(cstring(b"\0x", 0).unwrap(), (String::new(), 1));
    }

    #[test]
    fn multibyte_strings_are_shifted_like_the_reference() {
        // "\u{e9}" (UTF-8 C3 A9): 1 character counted, step back 2 bytes from after the NUL (3) → re-read from A9.
        assert_eq!(
            cstring(b"\xc3\xa9\0x\0", 0).unwrap(),
            ("\u{fffd}".into(), 3)
        );
    }

    #[test]
    fn last_string_without_nul_loses_its_last_character() {
        // "ab" at end of file, preceded by a NUL: re-read from that NUL, NUL removed.
        assert_eq!(cstring(b"x\0ab", 2).unwrap(), ("a".into(), 4));
        assert_eq!(cstring(b"ab", 0), Err(CryXmlError::NegativeOffset));
    }

    #[test]
    fn invalid_bytes_become_replacement_characters() {
        assert_eq!(one(b"\xff"), Ok(0xFFFD));
        assert_eq!(one(b"\xe2\x82"), Err(CryXmlError::Eof));
        assert_eq!(chars(b"\xe0\x80b", 3).unwrap(), vec![0xFFFD, 0xFFFD, 98]);
        assert_eq!(
            chars(b"\xe2\x82\xe2\x82\xacb", 3).unwrap(),
            vec![0xFFFD, 0x20AC, 98]
        );
        assert_eq!(
            chars("é😀".as_bytes(), 3).unwrap(), // check-french: allow Unicode test input
            vec![0xE9, 0xD83D, 0xDE00]
        );
    }

    #[test]
    fn a_broken_sequence_followed_by_a_char_fails_one_char_reads() {
        assert_eq!(one(b"\xe2\x82b"), Err(CryXmlError::CharOverflow));
        assert_eq!(one(b"\xe2b"), Err(CryXmlError::CharOverflow));
        assert_eq!(one(b"\xe2\x82\xff"), Err(CryXmlError::CharOverflow));
        // C3 C3 b: the second C3 breaks the first (one replacement) then stays pending; "b" breaks it.
        let mut r = CharReader::new(b"\xc3\xc3b", 0);
        assert_eq!(r.read_char(), Ok(0xFFFD));
        assert_eq!(r.read_char(), Err(CryXmlError::CharOverflow));
        assert_eq!(
            CharReader::new(b"\xc3b", 0).peek_char(),
            Err(CryXmlError::CharOverflow)
        );
        // Broken by a lead byte: it stays pending, only one character comes out.
        let mut r = CharReader::new(b"\xe2\x82\xc3\xa9", 0);
        assert_eq!((r.read_char(), r.pos), (Ok(0xFFFD), 3));
        assert_eq!((r.read_char(), r.pos), (Ok(0xE9), 4));
        assert_eq!(one(b"\xffb"), Ok(0xFFFD));
        assert_eq!(one(b"\x80b"), Ok(0xFFFD));
        assert_eq!(one(b"\xc0b"), Ok(0xFFFD));
    }

    #[test]
    fn bulk_reads_fail_when_the_decoder_outruns_the_buffer() {
        // Enough room: the replacement and the next character fit.
        assert_eq!(chars(b"\xe2\x82b", 2).unwrap(), vec![0xFFFD, 98]);
        // `CryXm E9 E9 00`: 6 units for 7 bytes, E9 pending; the next byte yields two for one slot.
        assert_eq!(chars(b"CryXm\xe9\xe9\0", 7), Err(CryXmlError::CharOverflow));
        // Only one slot for a surrogate pair.
        assert_eq!(chars("😀".as_bytes(), 1), Err(CryXmlError::CharOverflow));
    }

    #[test]
    fn four_byte_sequences_fail_to_read() {
        assert_eq!(one("😀".as_bytes()), Err(CryXmlError::Supplementary));
        assert_eq!(
            CharReader::new("😀".as_bytes(), 0).peek_char(),
            Err(CryXmlError::Supplementary)
        );
    }

    #[test]
    fn peek_reports_end_of_file_as_none_and_keeps_the_position() {
        assert_eq!(CharReader::new(b"", 0).peek_char().unwrap(), None);
        assert_eq!(CharReader::new(b"\xe2", 0).peek_char().unwrap(), None);
        let mut r = CharReader::new(b"<", 0);
        assert_eq!((r.peek_char().unwrap(), r.pos), (Some(u16::from(b'<')), 0));
    }

    #[test]
    fn read_chars_stops_at_end_of_file() {
        let mut r = CharReader::new(b"abc", 1);
        assert_eq!((r.read_chars(10).unwrap(), r.pos), (vec![98, 99], 3));
    }
}
