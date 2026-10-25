//! p4kr-dataforge: reading DataForge databases (`.dcb`) and converting them to XML like the reference.

mod header;
mod outputs;
mod record;
mod tables;
#[doc(hidden)]
pub mod value;

pub use outputs::{Output, has_trace, safe_relative};

use header::{Layout, Table};
#[cfg(test)]
mod accessor_tests;

use std::borrow::Cow;
use tables::{FieldDef, RecordDef, StructDef};

#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum DcbError {
    #[error("version {found} not supported (known: 8)")]
    Version { found: i32 },
    #[error("table {table}: {need} bytes expected, {have} available")]
    Truncated {
        table: &'static str,
        need: u64,
        have: u64,
    },
    #[error("end of file: {expected} bytes expected, {actual} read")]
    Length { expected: u64, actual: u64 },
    #[error("mapping {index}: struct {found} instead of {index}")]
    Mapping { index: u32, found: u32 },
    #[error("{mappings} mappings for {structs} structs")]
    MappingCount { mappings: u32, structs: u32 },
    #[error("fields: the structs declare {declared}, the table only has {count}")]
    FieldTotal { declared: u64, count: u32 },
    #[error("field {index}: unknown type 0x{code:04X}")]
    UnknownType { index: u32, code: u16 },
    #[error("field {index}: unknown conversion {code}")]
    UnknownConversion { index: u32, code: u16 },
    #[error("struct {index}: parent {parent} missing")]
    Parent { index: u32, parent: u32 },
    #[error("struct {index}: fields {first}..{end} outside the table")]
    Fields { index: u32, first: u32, end: u32 },
    #[error("struct {index}: declared size {declared}, computed {computed}")]
    StructSize {
        index: u32,
        declared: u32,
        computed: u64,
    },
    /// The reference reads every path at open time and then rejects the whole file.
    #[error("record {index}: path at offset {offset}, outside the text ({len} bytes)")]
    RecordPath { index: u32, offset: u32, len: u32 },
    /// The reference's field index is 16-bit: beyond it, its loop never ends.
    #[error("struct {index}: fields up to {end}, beyond the reference's 16-bit index")]
    FieldIndex { index: u32, end: u32 },
    /// Path with no end before `max` bytes: no file of that name can be created (the reference's save
    /// fails on it), and reading it in full would cost without bound.
    #[error("record {index}: path longer than {max} bytes")]
    RecordPathLength { index: u32, max: usize },
}

/// Failure of a single record; the others are rendered.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum RecordError {
    #[error("unsafe path")]
    UnsafePath,
    /// Produced by p4kr-core, when a record path collides with another output of the extraction.
    #[error("path already taken by another output")]
    Collision,
    #[error("output too large")]
    TooLarge,
    /// Deliberate deviation from the reference: a record demanding excessive work (see `MAX_STEPS`).
    #[error("record too costly to render")]
    TooMuchWork,
    /// Name rejected by the reference's XML writer, which then abandons its whole save; p4kr
    /// abandons only this record.
    #[error("element or attribute name rejected at write time")]
    BadName,
    /// Write impossible: XML writer refusal unrelated to a name (does not happen: root always set, never
    /// any CDATA, kept so as never to panic), or an I/O error on the file (p4kr-core, which then reports
    /// the kind of error separately).
    #[error("write failed")]
    Write,
}

/// Exception of the reference while reading a record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Fault {
    IndexOutOfRange(String),
    EndOfStream,
    KeyNotFound(String),
    /// Name rejected when creating the element or attribute (.NET message).
    Name(String),
}

/// Marker of a call stack in an error text (`ToString()` of a .NET exception), which the reference
/// recognises once written (`&#xA;   at `). The text after the marker is written to disk: do not change it.
const TRACE: &str = "\n   at p4kr-dataforge (pile d'appels de la référence non reproduite)"; // check-french: allow bytes written into the XML

impl Fault {
    pub(crate) fn out_of_range(index: u32, label: &str, count: u32) -> Fault {
        Fault::IndexOutOfRange(format!(
            "Index {index} is out of range for {label} values (count: {count})"
        ))
    }

    pub(crate) fn message(&self) -> &str {
        match self {
            Fault::IndexOutOfRange(m) | Fault::KeyNotFound(m) | Fault::Name(m) => m,
            Fault::EndOfStream => "Unable to read beyond the end of the stream.",
        }
    }

    /// Short form of .NET's `ToString()`: class, message, then one marked stack line.
    pub(crate) fn exception(&self) -> String {
        let class = match self {
            Fault::IndexOutOfRange(_) => "System.IndexOutOfRangeException",
            Fault::EndOfStream => "System.IO.EndOfStreamException",
            Fault::KeyNotFound(_) => "System.Collections.Generic.KeyNotFoundException",
            Fault::Name(_) => "System.Xml.XmlException",
        };
        format!("{class}: {}{TRACE}", self.message())
    }
}

/// Open database: bytes in place, table positions and data offset of each struct.
pub struct Db<'a> {
    bytes: &'a [u8],
    layout: Layout,
    sizes: Vec<u64>,
    data_starts: Vec<u64>,
    /// For each struct, the nearest ancestor that has fields (`NONE`: none).
    field_parents: Vec<u32>,
}

impl<'a> Db<'a> {
    /// Reads the header and the tables, and rejects any inconsistent file before anything is produced.
    pub fn open(bytes: &'a [u8]) -> Result<Db<'a>, DcbError> {
        let layout = Layout::parse(bytes)?;
        let unchecked = Db {
            bytes,
            layout,
            sizes: Vec::new(),
            data_starts: Vec::new(),
            field_parents: Vec::new(),
        };
        let (sizes, data_starts) = tables::check(&unchecked)?;
        check_record_paths(&unchecked)?;
        let field_parents = tables::field_parents(&unchecked);
        Ok(Db {
            sizes,
            data_starts,
            field_parents,
            ..unchecked
        })
    }

    pub fn version(&self) -> i32 {
        self.layout.version
    }

    pub fn record_count(&self) -> usize {
        self.layout.counts[Table::Record.slot()] as usize
    }

    pub fn struct_count(&self) -> usize {
        self.layout.counts[Table::Struct.slot()] as usize
    }
}

/// Maximum length of a record path, in bytes.
const MAX_PATH: usize = 4096;

fn check_record_paths(db: &Db) -> Result<(), DcbError> {
    let len = db.layout.text_len;
    for index in 0..db.count(Table::Record) {
        let offset = db.record_def(index).map_or(u32::MAX, |r| r.path);
        if offset > len {
            return Err(DcbError::RecordPath { index, offset, len });
        }
        let tail = db.pool_tail(db.layout.text_start, offset);
        let end = tail.iter().take(MAX_PATH + 1).position(|&b| b == 0);
        if end.unwrap_or(tail.len()) > MAX_PATH {
            return Err(DcbError::RecordPathLength {
                index,
                max: MAX_PATH,
            });
        }
    }
    Ok(())
}

impl<'a> Db<'a> {
    pub(crate) fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    pub(crate) fn layout(&self) -> &Layout {
        &self.layout
    }

    pub(crate) fn count(&self, t: Table) -> u32 {
        self.layout.counts[t.slot()]
    }

    pub(crate) fn table_pos(&self, t: Table, index: u64) -> u64 {
        self.layout.starts[t.slot()].saturating_add(index.saturating_mul(t.row_size()))
    }

    pub(crate) fn struct_def(&self, i: u32) -> Option<StructDef> {
        tables::read_struct(self, i)
    }

    pub(crate) fn field_def(&self, i: u32) -> Option<FieldDef> {
        tables::read_field(self, i)
    }

    pub(crate) fn record_def(&self, i: u32) -> Option<RecordDef> {
        tables::read_record(self, i)
    }

    /// (instance count, struct) of mapping `i`.
    pub(crate) fn mapping(&self, i: u32) -> Option<(u32, u32)> {
        tables::read_mapping(self, i)
    }

    /// Nearest ancestor of `s` that has fields; empty ancestors cost nothing to walk.
    pub(crate) fn field_parent(&self, s: u32) -> Option<u32> {
        self.field_parents
            .get(s as usize)
            .copied()
            .filter(|&p| p != header::NONE)
    }

    /// Start of instance `variant` of struct `s` with no bound on `variant`, like the reference
    /// (32-bit size × instance product; the instance equal to the instance count reads what follows);
    /// `None` if `s` has no data.
    pub(crate) fn instance_pos(&self, s: u32, variant: u32) -> Option<u64> {
        let start = *self.data_starts.get(s as usize)?;
        let size = u32::try_from(*self.sizes.get(s as usize)?).ok()?;
        Some(start + u64::from(size.wrapping_mul(variant)))
    }

    /// Start of instance `variant` of struct `s`, `None` if it does not exist.
    #[cfg(test)]
    pub(crate) fn data_pos(&self, s: u32, variant: u32) -> Option<u64> {
        let (count, _) = self.mapping(s)?;
        if variant >= count {
            return None;
        }
        let start = *self.data_starts.get(s as usize)?;
        let size = *self.sizes.get(s as usize)?;
        Some(start + u64::from(variant) * size)
    }

    /// Latin-1 string of the text at offset `off`.
    pub(crate) fn text(&self, off: u32) -> Result<String, Fault> {
        self.text_cow(off).map(Cow::into_owned)
    }

    /// String of the blob; the error message gives the text length, like the reference.
    #[cfg(test)]
    pub(crate) fn blob(&self, off: u32) -> Result<String, Fault> {
        self.blob_cow(off).map(Cow::into_owned)
    }

    /// `text`, borrowed from the file bytes when it is ASCII.
    pub(crate) fn text_cow(&self, off: u32) -> Result<Cow<'a, str>, Fault> {
        self.pooled(off, self.layout.text_start, self.layout.text_len, "Text")
    }

    /// `blob`, borrowed from the file bytes when it is ASCII.
    pub(crate) fn blob_cow(&self, off: u32) -> Result<Cow<'a, str>, Fault> {
        self.pooled(off, self.layout.blob_start, self.layout.blob_len, "Blob")
    }

    /// File bytes from offset `off` of a section; empty beyond the end.
    fn pool_tail(&self, start: u64, off: u32) -> &'a [u8] {
        usize::try_from(start + u64::from(off))
            .ok()
            .and_then(|from| self.bytes.get(from..))
            .unwrap_or_default()
    }

    fn pooled(&self, off: u32, start: u64, limit: u32, label: &str) -> Result<Cow<'a, str>, Fault> {
        // The reference bounds the offset by the section length, but its message
        // always prints the text one (blob included).
        let shown = self.layout.text_len;
        if off > limit {
            return Err(Fault::IndexOutOfRange(format!(
                "Offset {off} is out of range for {label} values (length: {shown})"
            )));
        }
        let tail = self.pool_tail(start, off);
        let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
        let raw = &tail[..end];
        Ok(match std::str::from_utf8(raw) {
            Ok(ascii) if raw.is_ascii() => Cow::Borrowed(ascii),
            _ => Cow::Owned(raw.iter().map(|&b| char::from(b)).collect()),
        })
    }
}
