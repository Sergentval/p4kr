//! End of archive (EOCD, ZIP64) and central directory decoded into a compact table: one fixed-size row per
//! entry and a single buffer for the names. Rules: the reference implementation.

use crate::{P4kError, Result};
use memmap2::Mmap;
use std::fs::File;
use std::ops::Deref;
use std::path::{Path, PathBuf};

const EOCD_SIG: &[u8] = b"PK\x05\x06";
const LOC64_SIG: &[u8] = b"PK\x06\x07";
const EOCD64_SIG: &[u8] = b"PK\x06\x06";
const CD_SIG: &[u8] = b"PK\x01\x02";
const EOCD_LEN: usize = 22;
const LOC64_LEN: usize = 20;
const EOCD64_LEN: usize = 56;
const CD_LEN: usize = 46;
const MAX32: u32 = u32::MAX;
const ZIP64_ID: u16 = 0x0001;
const SIG_ID: u16 = 0x5000;
const ENC_ID: u16 = 0x5002;
const SHA_ID: u16 = 0x5003;
/// Known blocks and their declared length (4-byte header included, a CIG quirk).
const KNOWN: [(u16, usize); 4] = [(ZIP64_ID, 32), (SIG_ID, 132), (ENC_ID, 6), (SHA_ID, 36)];

pub(crate) const ENCRYPTED: u8 = 1;
pub(crate) const HAS_SHA: u8 = 2;
pub(crate) const BAD_EXTRA: u8 = 4;

pub(crate) fn le<const N: usize>(b: &[u8], at: usize) -> Option<[u8; N]> {
    b.get(at..at.checked_add(N)?)?.try_into().ok()
}
pub(crate) fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    le::<2>(b, at).map(u16::from_le_bytes)
}
pub(crate) fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    le::<4>(b, at).map(u32::from_le_bytes)
}
pub(crate) fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    le::<8>(b, at).map(u64::from_le_bytes)
}

fn format(msg: String) -> P4kError {
    P4kError::Format(msg)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tail {
    pub cd_offset: u64,
    pub cd_size: u64,
    pub entries: u64,
}

/// Table row: absolute offsets in the archive, name in `Archive::names[name_start..name_end]`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Row {
    pub name_start: usize,
    pub name_end: usize,
    pub raw_name_at: u64,
    pub raw_name_len: u16,
    pub local_offset: u64,
    pub csize: u64,
    pub size: u64,
    pub sha_at: u64,
    pub crc: u32,
    pub method: u16,
    pub flags: u8,
}

#[derive(Clone, Copy, Debug, Default)]
struct Extras {
    zip64: Option<(u64, u64, u64)>,
    encrypted: bool,
    sha_at: Option<u64>,
    bad: bool,
}

fn parse_extra(b: &[u8], start: usize, len: usize) -> Extras {
    let mut x = Extras::default();
    let mut seen = [false; KNOWN.len()];
    let end = start.saturating_add(len);
    let mut pos = start;
    while pos < end {
        let (Some(id), Some(declared)) = (u16_at(b, pos), u16_at(b, pos.saturating_add(2))) else {
            x.bad = true;
            break;
        };
        let blen = usize::from(declared);
        if blen < 4 || pos.saturating_add(blen) > end {
            x.bad = true;
            break;
        }
        let body = pos + 4;
        match KNOWN.iter().position(|&(k, l)| k == id && l == blen) {
            Some(k) if !seen[k] => {
                seen[k] = true;
                match id {
                    ZIP64_ID => {
                        x.zip64 = match (u64_at(b, body), u64_at(b, body + 8), u64_at(b, body + 16))
                        {
                            (Some(u), Some(c), Some(o)) => Some((u, c, o)),
                            _ => None,
                        }
                    }
                    ENC_ID => x.encrypted = u16_at(b, body).is_some_and(|v| v != 0),
                    SHA_ID => x.sha_at = Some(body as u64),
                    _ => {} // 0x5000: signature, unused by the reader
                }
            }
            _ => x.bad = true, // unknown, unexpected length or duplicate
        }
        pos += blen;
    }
    x
}

pub(crate) fn read_tail(b: &[u8]) -> Result<Tail> {
    let start = b.len().saturating_sub(EOCD_LEN + 0xFFFF);
    let pos = b
        .get(start..)
        .and_then(|window| window.windows(4).rposition(|w| w == EOCD_SIG))
        .map(|p| p + start)
        .filter(|p| p + EOCD_LEN <= b.len())
        .ok_or_else(|| {
            format("end of archive not found (EOCD missing): truncated archive?".into())
        })?;
    let (Some(entries), Some(size), Some(offset)) = (
        u16_at(b, pos + 10),
        u32_at(b, pos + 12),
        u32_at(b, pos + 16),
    ) else {
        return Err(format(format!("EOCD truncated at {pos:#x}")));
    };
    if size != MAX32 && offset != MAX32 && entries != 0xFFFF {
        return Ok(Tail {
            cd_offset: offset.into(),
            cd_size: size.into(),
            entries: entries.into(),
        });
    }
    let loc = pos
        .checked_sub(LOC64_LEN)
        .filter(|&l| b.get(l..l + 4) == Some(LOC64_SIG))
        .ok_or_else(|| format(format!("ZIP64 locator missing before {pos:#x}")))?;
    let eocd64 =
        u64_at(b, loc + 8).ok_or_else(|| format(format!("ZIP64 locator truncated at {loc:#x}")))?;
    let e = usize::try_from(eocd64)
        .ok()
        .filter(|&e| {
            e.checked_add(EOCD64_LEN).is_some_and(|end| end <= loc)
                && b.get(e..e + 4) == Some(EOCD64_SIG)
        })
        .ok_or_else(|| format(format!("invalid EOCD64 at {eocd64:#x}")))?;
    let (Some(total), Some(size), Some(offset)) =
        (u64_at(b, e + 32), u64_at(b, e + 40), u64_at(b, e + 48))
    else {
        return Err(format(format!("EOCD64 truncated at {eocd64:#x}")));
    };
    if offset.checked_add(size).is_none_or(|end| end > eocd64) {
        return Err(format(format!(
            "central directory ({offset:#x} + {size}) overflows into the EOCD64 at {eocd64:#x}"
        )));
    }
    Ok(Tail {
        cd_offset: offset,
        cd_size: size,
        entries: total,
    })
}

/// Central directory fields: method @10, crc @16, sizes @20/@24, lengths @28/@30/@32, offset @42.
fn decode_entry(b: &[u8], pos: usize, end: usize, names: &mut String) -> Result<(Row, usize)> {
    if pos + CD_LEN > end || b.get(pos..pos + 4) != Some(CD_SIG) {
        return Err(format(format!(
            "central directory signature missing at {pos:#x}"
        )));
    }
    let field16 = |at: usize| u16_at(b, pos + at).unwrap_or(0);
    let field32 = |at: usize| u32_at(b, pos + at).unwrap_or(0);
    let (method, crc, csize32, size32, off32) = (
        field16(10),
        field32(16),
        field32(20),
        field32(24),
        field32(42),
    );
    let (nlen, elen, clen) = (
        usize::from(field16(28)),
        usize::from(field16(30)),
        usize::from(field16(32)),
    );
    let name_at = pos + CD_LEN;
    let extra_at = name_at + nlen;
    let next = extra_at + elen + clen;
    if next > end {
        return Err(format(format!(
            "entry at {pos:#x} overflows the central directory"
        )));
    }
    let x = parse_extra(b, extra_at, elen);
    let wide = [csize32, size32, off32].contains(&MAX32);
    let (size, csize, local_offset, bad) = match (wide, x.zip64) {
        (false, _) => (size32.into(), csize32.into(), off32.into(), x.bad),
        (true, Some((u, c, o))) => (u, c, o, x.bad),
        (true, None) => (0, 0, 0, true),
    };
    let raw = b
        .get(name_at..extra_at)
        .ok_or_else(|| format(format!("name of the entry at {pos:#x} is outside the file")))?;
    let name_start = names.len();
    match std::str::from_utf8(raw) {
        Ok(s) => names.push_str(s),
        Err(_) => names.push_str(&String::from_utf8_lossy(raw)),
    }
    let flags = (u8::from(x.encrypted) * ENCRYPTED)
        | (u8::from(x.sha_at.is_some()) * HAS_SHA)
        | (u8::from(bad) * BAD_EXTRA);
    let row = Row {
        name_start,
        name_end: names.len(),
        raw_name_at: name_at as u64,
        raw_name_len: field16(28),
        local_offset,
        csize,
        size,
        sha_at: x.sha_at.unwrap_or(0),
        crc,
        method,
        flags,
    };
    Ok((row, next))
}

fn read_central(b: &[u8], tail: &Tail) -> Result<(Vec<Row>, String)> {
    let span = || {
        format!(
            "central directory outside the file ({:#x} + {})",
            tail.cd_offset, tail.cd_size
        )
    };
    let start = usize::try_from(tail.cd_offset).map_err(|_| format(span()))?;
    let end = tail
        .cd_offset
        .checked_add(tail.cd_size)
        .and_then(|e| usize::try_from(e).ok())
        .filter(|&e| e <= b.len())
        .ok_or_else(|| format(span()))?;
    let span_len = end.checked_sub(start).ok_or_else(|| format(span()))?;
    // The declared entry count is bounded by the directory size before any allocation.
    let cap = usize::try_from(tail.entries)
        .unwrap_or(usize::MAX)
        .min(span_len / CD_LEN);
    let mut rows = Vec::with_capacity(cap);
    let mut names = String::with_capacity(span_len / 4);
    let mut pos = start;
    for index in 0..tail.entries {
        if pos >= end {
            return Err(format(format!(
                "entry {index}: central directory exhausted at {pos:#x}"
            )));
        }
        let (row, next) = decode_entry(b, pos, end, &mut names)?;
        rows.push(row);
        pos = next;
    }
    if pos != end {
        return Err(format(format!(
            "central directory: ends at {pos:#x}, expected at {end:#x}"
        )));
    }
    Ok((rows, names))
}

enum Storage {
    Map(Mmap),
    Owned(Vec<u8>),
}

impl Deref for Storage {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Storage::Map(m) => m,
            Storage::Owned(v) => v,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct EntryRef<'a> {
    pub index: usize,
    pub name: &'a str,
    pub csize: u64,
    pub size: u64,
    pub method: u16,
    pub crc: u32,
    pub local_offset: u64,
    pub encrypted: bool,
    pub has_sha: bool,
    pub bad_extra: bool,
}

pub struct Archive {
    path: PathBuf,
    storage: Storage,
    rows: Vec<Row>,
    names: String,
    tail: Tail,
}

impl Archive {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        if len < EOCD_LEN as u64 {
            return Err(format(format!(
                "file of {len} bytes: too short for an archive"
            )));
        }
        // SAFETY: read-only mapping; like any mmap reader, we assume the archive is not
        // rewritten while being read (truncation midway would raise SIGBUS).
        let map = unsafe { Mmap::map(&file)? };
        Self::build(path.to_path_buf(), Storage::Map(map))
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        if bytes.len() < EOCD_LEN {
            return Err(format(format!(
                "{} bytes: too short for an archive",
                bytes.len()
            )));
        }
        Self::build(PathBuf::new(), Storage::Owned(bytes))
    }

    fn build(path: PathBuf, storage: Storage) -> Result<Self> {
        let tail = read_tail(&storage)?;
        let (rows, names) = read_central(&storage, &tail)?;
        Ok(Self {
            path,
            storage,
            rows,
            names,
            tail,
        })
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn tail(&self) -> Tail {
        self.tail
    }
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.storage
    }
    pub(crate) fn row(&self, i: usize) -> Result<&Row> {
        self.rows.get(i).ok_or_else(|| {
            format(format!(
                "index {i} outside the table ({} entries)",
                self.rows.len()
            ))
        })
    }
    pub fn name(&self, i: usize) -> Option<&str> {
        let r = self.rows.get(i)?;
        self.names.get(r.name_start..r.name_end)
    }
    pub fn entry(&self, i: usize) -> Option<EntryRef<'_>> {
        let r = self.rows.get(i)?;
        Some(EntryRef {
            index: i,
            name: self.names.get(r.name_start..r.name_end)?,
            csize: r.csize,
            size: r.size,
            method: r.method,
            crc: r.crc,
            local_offset: r.local_offset,
            encrypted: r.flags & ENCRYPTED != 0,
            has_sha: r.flags & HAS_SHA != 0,
            bad_extra: r.flags & BAD_EXTRA != 0,
        })
    }
    pub fn entries(&self) -> impl Iterator<Item = EntryRef<'_>> {
        (0..self.rows.len()).filter_map(|i| self.entry(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(id: u16, declared: u16, body: &[u8]) -> Vec<u8> {
        let mut v = id.to_le_bytes().to_vec();
        v.extend_from_slice(&declared.to_le_bytes());
        v.extend_from_slice(body);
        v
    }

    fn zip64(u: u64, c: u64, o: u64) -> Vec<u8> {
        let mut body = Vec::new();
        for x in [u, c, o] {
            body.extend_from_slice(&x.to_le_bytes());
        }
        body.extend_from_slice(&0u32.to_le_bytes());
        block(ZIP64_ID, 32, &body)
    }

    #[test]
    fn declared_length_includes_the_four_byte_header() {
        let raw = zip64(5, 6, 7);
        let x = parse_extra(&raw, 0, raw.len());
        assert_eq!((x.zip64, x.bad), (Some((5, 6, 7)), false));
    }

    #[test]
    fn standard_zip_length_is_not_the_cig_block() {
        let mut raw = zip64(5, 6, 7);
        raw[2..4].copy_from_slice(&28u16.to_le_bytes()); // "body only" length of the standard ZIP
        assert!(parse_extra(&raw, 0, raw.len()).bad);
    }

    #[test]
    fn unknown_duplicate_or_malformed_blocks_mark_the_entry() {
        let unknown = block(0x5004, 8, &[0xaa; 4]);
        let mut dup = zip64(1, 1, 1);
        dup.extend(zip64(2, 2, 2));
        let short = block(0x5002, 2, &[]);
        let overflow = block(0x5002, 60, &[0; 2]);
        for raw in [unknown, dup, short, overflow] {
            assert!(parse_extra(&raw, 0, raw.len()).bad, "{raw:?}");
        }
    }

    #[test]
    fn encryption_and_sha_blocks_are_read() {
        let mut raw = block(ENC_ID, 6, &1u16.to_le_bytes());
        raw.extend(block(SHA_ID, 36, &[7u8; 32]));
        let x = parse_extra(&raw, 0, raw.len());
        assert!(x.encrypted && !x.bad);
        assert_eq!(x.sha_at, Some(10));
    }

    #[test]
    fn zip64_marker_without_locator_is_a_format_error() {
        let mut eocd = b"PK\x05\x06".to_vec();
        eocd.extend_from_slice(&[0u8; 6]);
        eocd.extend_from_slice(&0xFFFFu16.to_le_bytes());
        eocd.extend_from_slice(&[0u8; 10]);
        assert!(matches!(read_tail(&eocd), Err(P4kError::Format(_))));
    }
}
