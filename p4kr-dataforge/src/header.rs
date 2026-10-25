//! 120-byte header and table positions, computed in `u64` and bounded by the file length.

use crate::DcbError;

pub(crate) const HEADER_LEN: u64 = 120;
pub(crate) const VERSION: i32 = 8;
pub(crate) const NONE: u32 = 0xFFFF_FFFF;

/// Tables, declared in the order in which they follow each other in the file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Table {
    Struct,
    Field,
    Enum,
    Mapping,
    Record,
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    Bool,
    Single,
    Double,
    Guid,
    Str,
    Locale,
    EnumValue,
    Strong,
    Weak,
    Reference,
    EnumOption,
}

pub(crate) const TABLE_COUNT: usize = 24;

/// Order in the file (layout).
pub(crate) const LAYOUT: [Table; TABLE_COUNT] = [
    Table::Struct,
    Table::Field,
    Table::Enum,
    Table::Mapping,
    Table::Record,
    Table::I8,
    Table::I16,
    Table::I32,
    Table::I64,
    Table::U8,
    Table::U16,
    Table::U32,
    Table::U64,
    Table::Bool,
    Table::Single,
    Table::Double,
    Table::Guid,
    Table::Str,
    Table::Locale,
    Table::EnumValue,
    Table::Strong,
    Table::Weak,
    Table::Reference,
    Table::EnumOption,
];

/// Order of the counters in the header: the boolean precedes the integers.
const COUNTERS: [Table; TABLE_COUNT] = [
    Table::Struct,
    Table::Field,
    Table::Enum,
    Table::Mapping,
    Table::Record,
    Table::Bool,
    Table::I8,
    Table::I16,
    Table::I32,
    Table::I64,
    Table::U8,
    Table::U16,
    Table::U32,
    Table::U64,
    Table::Single,
    Table::Double,
    Table::Guid,
    Table::Str,
    Table::Locale,
    Table::EnumValue,
    Table::Strong,
    Table::Weak,
    Table::Reference,
    Table::EnumOption,
];

impl Table {
    pub(crate) fn slot(self) -> usize {
        self as usize
    }

    pub(crate) fn row_size(self) -> u64 {
        match self {
            Table::Struct => 16,
            Table::Field => 12,
            Table::Enum | Table::Mapping | Table::Strong | Table::Weak => 8,
            Table::Record => 36,
            Table::I8 | Table::U8 | Table::Bool => 1,
            Table::I16 | Table::U16 => 2,
            Table::I32
            | Table::U32
            | Table::Single
            | Table::Str
            | Table::Locale
            | Table::EnumValue
            | Table::EnumOption => 4,
            Table::I64 | Table::U64 | Table::Double => 8,
            Table::Guid => 16,
            Table::Reference => 20,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Table::Struct => "structs",
            Table::Field => "fields",
            Table::Enum => "enums",
            Table::Mapping => "mappings",
            Table::Record => "records",
            Table::I8 => "i8 values",
            Table::I16 => "i16 values",
            Table::I32 => "i32 values",
            Table::I64 => "i64 values",
            Table::U8 => "u8 values",
            Table::U16 => "u16 values",
            Table::U32 => "u32 values",
            Table::U64 => "u64 values",
            Table::Bool => "boolean values",
            Table::Single => "single values",
            Table::Double => "double values",
            Table::Guid => "guid values",
            Table::Str => "string values",
            Table::Locale => "locale values",
            Table::EnumValue => "enum values",
            Table::Strong => "strong pointer values",
            Table::Weak => "weak pointer values",
            Table::Reference => "reference values",
            Table::EnumOption => "enum option values",
        }
    }
}

/// Positions of the tables and sections. Negative counters are read as unsigned: huge, hence rejected.
#[derive(Clone, Debug)]
pub(crate) struct Layout {
    pub version: i32,
    pub counts: [u32; TABLE_COUNT],
    pub starts: [u64; TABLE_COUNT],
    pub text_len: u32,
    pub text_start: u64,
    pub blob_len: u32,
    pub blob_start: u64,
    pub data_start: u64,
}

fn le32(bytes: &[u8], at: usize) -> u32 {
    let mut raw = [0u8; 4];
    raw.copy_from_slice(&bytes[at..at + 4]);
    u32::from_le_bytes(raw)
}

fn section(label: &'static str, pos: u64, need: u64, len: u64) -> Result<u64, DcbError> {
    let have = len.saturating_sub(pos);
    if need > have {
        return Err(DcbError::Truncated {
            table: label,
            need,
            have,
        });
    }
    Ok(pos + need)
}

impl Layout {
    pub(crate) fn parse(bytes: &[u8]) -> Result<Layout, DcbError> {
        let len = bytes.len() as u64;
        if len < HEADER_LEN {
            return Err(DcbError::Truncated {
                table: "header",
                need: HEADER_LEN,
                have: len,
            });
        }
        let version = le32(bytes, 4) as i32;
        if version != VERSION {
            return Err(DcbError::Version { found: version });
        }
        let mut counts = [0u32; TABLE_COUNT];
        for (k, table) in COUNTERS.iter().enumerate() {
            counts[table.slot()] = le32(bytes, 16 + 4 * k);
        }
        let (text_len, blob_len) = (le32(bytes, 112), le32(bytes, 116));
        let mut starts = [0u64; TABLE_COUNT];
        let mut pos = HEADER_LEN;
        for table in LAYOUT {
            starts[table.slot()] = pos;
            let need = u64::from(counts[table.slot()]) * table.row_size();
            pos = section(table.label(), pos, need, len)?;
        }
        let text_start = pos;
        let blob_start = section("text", pos, u64::from(text_len), len)?;
        let data_start = section("blob", blob_start, u64::from(blob_len), len)?;
        Ok(Layout {
            version,
            counts,
            starts,
            text_len,
            text_start,
            blob_len,
            blob_start,
            data_start,
        })
    }
}
