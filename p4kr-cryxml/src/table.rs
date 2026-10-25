//! Signature, header and tables of a CryXmlB, read like unforge's `CryXmlSerializer.ReadStream`.

use crate::dotnet::CharReader;
use crate::{CryXmlError, Result};
use std::collections::HashMap;

const NODE_SIZE: i32 = 28;
const ATTR_SIZE: i32 = 8;
const CHILD_SIZE: i32 = 4;

/// Kind of content, decided by its first character and its signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Starts with `<`: already XML, nothing is converted.
    Text,
    /// Neither `<` nor a known signature: not a CryXmlB.
    Other,
    /// Signature recognised; the header starts at this position.
    Binary(usize),
}

/// Kind and reader placed after the signature: the decoder state carries over to string reads.
pub(crate) fn kind(b: &[u8]) -> Result<(Kind, CharReader<'_>)> {
    let mut r = CharReader::new(b, 0);
    match r.peek_char()? {
        Some(u) if u == u16::from(b'<') => return Ok((Kind::Text, r)),
        Some(u) if u == u16::from(b'C') => {}
        _ => return Ok((Kind::Other, r)),
    }
    let units = r.read_chars(7)?;
    let header = match units.iter().position(|&u| u == 0) {
        Some(i) => &units[..i],
        None if units.len() < 7 => return Err(CryXmlError::Eof),
        None => &units[..],
    };
    let is = |s: &str| header.iter().copied().eq(s.encode_utf16());
    if is("CryXml") || is("CryXmlB") {
        r.read_cstring()?;
    } else if is("CRY3SDK") {
        r.pos = (r.pos + 2).min(b.len());
    } else {
        return Ok((Kind::Other, r));
    }
    Ok((Kind::Binary(r.pos), r))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NodeRow {
    pub name: i32,
    pub content: i32,
    pub attr_count: i16,
    pub parent: i32,
}

#[derive(Debug)]
pub(crate) struct Tables {
    pub nodes: Vec<NodeRow>,
    pub attrs: Vec<(i32, i32)>,
    pub strings: HashMap<i32, String>,
}

struct Cursor<'a> {
    b: &'a [u8],
    pos: usize,
    /// Bytes reversed before reading, hence big-endian.
    reversed: bool,
}

impl Cursor<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let end = self
            .pos
            .checked_add(N)
            .filter(|&e| e <= self.b.len())
            .ok_or(CryXmlError::Eof)?;
        let mut bytes: [u8; N] = self.b[self.pos..end]
            .try_into()
            .map_err(|_| CryXmlError::Eof)?;
        if self.reversed {
            bytes.reverse();
        }
        self.pos = end;
        Ok(bytes)
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take::<4>()?))
    }
    fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_le_bytes(self.take::<2>()?))
    }
    /// `Seek(offset, Begin)`: negative → error; beyond the end allowed (the next read fails).
    fn seek(&mut self, offset: i32) -> Result<()> {
        self.pos = usize::try_from(offset).map_err(|_| CryXmlError::NegativeOffset)?;
        Ok(())
    }
    /// Loop condition of the reference: `position < offset + count × size` in C# `int` (overflow).
    fn before(&self, offset: i32, count: i32, size: i32) -> bool {
        (self.pos as i64) < i64::from(offset.wrapping_add(count.wrapping_mul(size)))
    }
}

/// Tables read from the position of `r` (end of the signature), with its decoder for strings.
pub(crate) fn read(mut r: CharReader<'_>) -> Result<Tables> {
    let (b, start) = (r.bytes(), r.pos);
    // Mirrors the reference: its order labels plus `BitConverter`'s native order (x86) make it read
    // little-endian first, then big-endian if the length does not match the file size.
    let mut c = Cursor {
        b,
        pos: start,
        reversed: false,
    };
    if i64::from(c.i32()?) != b.len() as i64 {
        c = Cursor {
            b,
            pos: start,
            reversed: true,
        };
        c.i32()?;
    }
    let (node_off, node_count) = (c.i32()?, c.i32()?);
    let (attr_off, attr_count) = (c.i32()?, c.i32()?);
    let (child_off, child_count) = (c.i32()?, c.i32()?);
    let (string_off, _string_size) = (c.i32()?, c.i32()?);

    c.seek(node_off)?;
    let mut nodes = Vec::new();
    while c.before(node_off, node_count, NODE_SIZE) {
        let (name, content) = (c.i32()?, c.i32()?);
        let (attr_count, _child_count) = (c.i16()?, c.i16()?);
        let parent = c.i32()?;
        let (_first_attr, _first_child, _reserved) = (c.i32()?, c.i32()?, c.i32()?);
        nodes.push(NodeRow {
            name,
            content,
            attr_count,
            parent,
        });
    }
    c.seek(attr_off)?;
    let mut attrs = Vec::new();
    while c.before(attr_off, attr_count, ATTR_SIZE) {
        attrs.push((c.i32()?, c.i32()?));
    }
    c.seek(child_off)?;
    while c.before(child_off, child_count, CHILD_SIZE) {
        c.i32()?; // read like the reference (may fail), never used
    }
    c.seek(string_off)?;
    let mut strings = HashMap::new();
    r.pos = c.pos;
    while r.pos < b.len() {
        let at = r.pos;
        let text = r.read_cstring()?;
        strings.insert((at as i64 - i64::from(string_off)) as i32, text);
    }
    Ok(Tables {
        nodes,
        attrs,
        strings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sniff(b: &[u8]) -> Result<Kind> {
        kind(b).map(|(k, _)| k)
    }

    fn tables(b: &[u8], start: usize) -> Result<Tables> {
        read(CharReader::new(b, start))
    }

    /// A `Root` node, no attribute: signature, little- or big-endian header, one row, "Root\0\0".
    fn single(big_endian: bool) -> Vec<u8> {
        let strings = b"Root\0\0";
        let (node_off, string_off) = (44, 44 + 28);
        let len = string_off + strings.len() as i32;
        let put = |out: &mut Vec<u8>, v: i32| {
            out.extend(if big_endian {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            })
        };
        let mut out = b"CryXmlB\0".to_vec();
        for v in [
            len,
            node_off,
            1,
            node_off + 28,
            0,
            node_off + 28,
            0,
            string_off,
            strings.len() as i32,
        ] {
            put(&mut out, v);
        }
        put(&mut out, 0);
        put(&mut out, 5);
        out.extend([0u8; 4]); // two zero i16s
        for v in [-1, 0, 0, 0] {
            put(&mut out, v);
        }
        out.extend(strings);
        out
    }

    #[test]
    fn signatures_are_recognised_like_the_reference() {
        assert_eq!(sniff(b"<a/>").unwrap(), Kind::Text);
        assert_eq!(sniff(b"").unwrap(), Kind::Other);
        assert_eq!(sniff(b"\xef\xbb\xbf<a/>").unwrap(), Kind::Other);
        assert_eq!(sniff(b"  <a/>").unwrap(), Kind::Other);
        assert_eq!(sniff(b"Cry"), Err(CryXmlError::Eof));
        assert_eq!(sniff(b"Cr\0").unwrap(), Kind::Other);
        assert_eq!(sniff(b"CryXmlB\0rest").unwrap(), Kind::Binary(8));
        assert_eq!(sniff(b"CRY3SDK\0\0rest").unwrap(), Kind::Binary(9));
        assert_eq!(sniff(b"CryXml\0abc\0rest").unwrap(), Kind::Binary(11));
        assert_eq!(sniff(b"CryXmlZ\0").unwrap(), Kind::Other);
        // Bulk read: pending E9 broken by E9 then NUL, two characters for one slot.
        assert_eq!(sniff(b"CryXm\xe9\xe9\0"), Err(CryXmlError::CharOverflow));
    }

    #[test]
    fn tables_read_in_both_byte_orders() {
        for big_endian in [false, true] {
            let b = single(big_endian);
            let t = tables(&b, 8).unwrap();
            assert_eq!(
                t.nodes,
                vec![NodeRow {
                    name: 0,
                    content: 5,
                    attr_count: 0,
                    parent: -1
                }]
            );
            assert_eq!(t.strings.get(&0).map(String::as_str), Some("Root"));
            assert_eq!(t.strings.get(&5).map(String::as_str), Some(""));
        }
    }

    #[test]
    fn absurd_counts_end_in_an_error_without_allocating() {
        let mut b = single(false);
        b[8 + 8..8 + 12].copy_from_slice(&0x0100_0000i32.to_le_bytes());
        assert_eq!(tables(&b, 8).unwrap_err(), CryXmlError::Eof);
        b[8 + 8..8 + 12].copy_from_slice(&0x7FFF_FFFFi32.to_le_bytes());
        // 0x7FFFFFFF × 28 overflows to -28: the loop does not run (golden `huge-count`).
        assert!(tables(&b, 8).unwrap().nodes.is_empty());
        b[8 + 8..8 + 12].copy_from_slice(&(-1i32).to_le_bytes());
        assert!(tables(&b, 8).unwrap().nodes.is_empty());
    }

    #[test]
    fn wrong_length_makes_the_tables_reread_big_endian() {
        let mut b = single(false);
        let wrong = b.len() as i32 + 1;
        b[8..12].copy_from_slice(&wrong.to_le_bytes());
        assert_eq!(tables(&b, 8).unwrap_err(), CryXmlError::Eof);
    }

    #[test]
    fn negative_offsets_fail_the_seek() {
        let mut b = single(false);
        b[8 + 28..8 + 32].copy_from_slice(&(-4i32).to_le_bytes());
        assert_eq!(tables(&b, 8).unwrap_err(), CryXmlError::NegativeOffset);
    }
}
