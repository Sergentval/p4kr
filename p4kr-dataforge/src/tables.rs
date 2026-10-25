//! Table rows and consistency checks, all done before any output.

use crate::header::{NONE, Table};
use crate::{Db, DcbError};
use std::borrow::Cow;

/// Data type codes.
pub(crate) mod types {
    pub(crate) const BOOL: u16 = 0x01;
    pub(crate) const I8: u16 = 0x02;
    pub(crate) const I16: u16 = 0x03;
    pub(crate) const I32: u16 = 0x04;
    pub(crate) const I64: u16 = 0x05;
    pub(crate) const U8: u16 = 0x06;
    pub(crate) const U16: u16 = 0x07;
    pub(crate) const U32: u16 = 0x08;
    pub(crate) const U64: u16 = 0x09;
    pub(crate) const STR: u16 = 0x0A;
    pub(crate) const SINGLE: u16 = 0x0B;
    pub(crate) const DOUBLE: u16 = 0x0C;
    pub(crate) const LOCALE: u16 = 0x0D;
    pub(crate) const GUID: u16 = 0x0E;
    pub(crate) const ENUM: u16 = 0x0F;
    pub(crate) const CLASS: u16 = 0x10;
    pub(crate) const STRONG: u16 = 0x110;
    pub(crate) const WEAK: u16 = 0x210;
    pub(crate) const REFERENCE: u16 = 0x310;
}

use types::*;

/// A type of the reference: code, name (`EDataType`), value table of an array (its row has the
/// width of the attribute field), label for index messages, name of the array element.
pub(crate) type Spec = (u16, &'static str, Table, &'static str, &'static str);

#[rustfmt::skip]
const SPECS: [Spec; 19] = [
    (BOOL, "varBoolean", Table::Bool, "Boolean", "Bool"),
    (I8, "varInt8", Table::I8, "Int8", "Int8"),
    (I16, "varInt16", Table::I16, "Int16", "Int16"),
    (I32, "varInt32", Table::I32, "Int32", "Int32"),
    (I64, "varInt64", Table::I64, "Int64", "Int64"),
    (U8, "varUInt8", Table::U8, "UInt8", "UInt8"),
    (U16, "varUInt16", Table::U16, "UInt16", "UInt16"),
    (U32, "varUInt32", Table::U32, "UInt32", "UInt32"),
    (U64, "varUInt64", Table::U64, "UInt64", "UInt64"),
    (STR, "varString", Table::Str, "String", "String"),
    (SINGLE, "varSingle", Table::Single, "Single", "Single"),
    (DOUBLE, "varDouble", Table::Double, "Double", "Double"),
    (LOCALE, "varLocale", Table::Locale, "Locale", "LocID"),
    (GUID, "varGuid", Table::Guid, "Guid", "Guid"),
    (ENUM, "varEnum", Table::EnumValue, "Enum", "Enum"),
    (CLASS, "varClass", Table::Struct, "Struct Definition", ""),
    (STRONG, "varStrongPointer", Table::Strong, "Strong Pointer", ""),
    (WEAK, "varWeakPointer", Table::Weak, "Weak Pointer", ""),
    (REFERENCE, "varReference", Table::Reference, "Reference", "Reference"),
];

pub(crate) fn spec(code: u16) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.0 == code)
}

/// Name of the type in the reference's messages; an unknown code is written in decimal.
pub(crate) fn type_text(code: u16) -> Cow<'static, str> {
    spec(code).map_or_else(|| Cow::Owned(code.to_string()), |s| Cow::Borrowed(s.1))
}

/// Fixed-size array from a slice (zero-padded, never panics).
pub(crate) fn arr<const N: usize>(b: &[u8]) -> [u8; N] {
    std::array::from_fn(|i| b.get(i).copied().unwrap_or(0))
}

/// GUID of a reference (20 bytes: ignored `u32`, then the GUID).
pub(crate) fn reference_guid(b: &[u8]) -> [u8; 16] {
    arr(b.get(4..).unwrap_or_default())
}

/// Pointer (8 bytes): struct, instance (the padding is ignored).
pub(crate) fn pointer_parts(b: &[u8]) -> (u32, u16) {
    let v = u16::from_le_bytes(arr(b.get(4..).unwrap_or_default()));
    (u32::from_le_bytes(arr(b)), v)
}

const NO_FIELD_INDEX: u16 = 0xFFFF;

#[derive(Clone, Copy, Debug)]
pub(crate) struct StructDef {
    pub name: u32,
    pub parent: u32,
    pub field_count: u16,
    pub first_field: u16,
    pub size: u32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct FieldDef {
    pub name: u32,
    pub index: u16,
    pub data_type: u16,
    pub conversion: u16,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RecordDef {
    pub name: u32,
    pub path: u32,
    pub team: u32,
    pub struct_index: u32,
    pub guid: [u8; 16],
    pub variant: u16,
}

pub(crate) fn raw<const N: usize>(bytes: &[u8], pos: u64) -> Option<[u8; N]> {
    let start = usize::try_from(pos).ok()?;
    let end = start.checked_add(N)?;
    bytes.get(start..end)?.try_into().ok()
}

pub(crate) fn u16_at(bytes: &[u8], pos: u64) -> Option<u16> {
    raw(bytes, pos).map(u16::from_le_bytes)
}

pub(crate) fn u32_at(bytes: &[u8], pos: u64) -> Option<u32> {
    raw(bytes, pos).map(u32::from_le_bytes)
}

pub(crate) fn read_struct(db: &Db, i: u32) -> Option<StructDef> {
    let p = db.table_pos(Table::Struct, u64::from(i));
    let b = db.bytes();
    Some(StructDef {
        name: u32_at(b, p)?,
        parent: u32_at(b, p + 4)?,
        field_count: u16_at(b, p + 8)?,
        first_field: u16_at(b, p + 10)?,
        size: u32_at(b, p + 12)?,
    })
}

pub(crate) fn read_field(db: &Db, i: u32) -> Option<FieldDef> {
    let p = db.table_pos(Table::Field, u64::from(i));
    let b = db.bytes();
    Some(FieldDef {
        name: u32_at(b, p)?,
        index: u16_at(b, p + 4)?,
        data_type: u16_at(b, p + 6)?,
        conversion: u16_at(b, p + 8)?,
    })
}

pub(crate) fn read_record(db: &Db, i: u32) -> Option<RecordDef> {
    let p = db.table_pos(Table::Record, u64::from(i));
    let b = db.bytes();
    Some(RecordDef {
        name: u32_at(b, p)?,
        path: u32_at(b, p + 4)?,
        team: u32_at(b, p + 8)?,
        struct_index: u32_at(b, p + 12)?,
        guid: raw(b, p + 16)?,
        variant: u16_at(b, p + 32)?,
    })
}

pub(crate) fn read_mapping(db: &Db, i: u32) -> Option<(u32, u32)> {
    let p = db.table_pos(Table::Mapping, u64::from(i));
    Some((u32_at(db.bytes(), p)?, u32_at(db.bytes(), p + 4)?))
}

/// The accessors return `None` only outside the file, which the position checks already rule out.
fn unreachable_read(table: &'static str) -> DcbError {
    DcbError::Truncated {
        table,
        need: 0,
        have: 0,
    }
}

fn check_mapping(db: &Db) -> Result<(), DcbError> {
    let (maps, structs) = (db.count(Table::Mapping), db.count(Table::Struct));
    for n in 0..maps {
        let (_, found) = read_mapping(db, n).ok_or_else(|| unreachable_read("mappings"))?;
        if found != n || n >= structs {
            return Err(DcbError::Mapping { index: n, found });
        }
    }
    if maps != structs {
        // Structs without a mapping: the file does not say where their instances are.
        return Err(DcbError::MappingCount {
            mappings: maps,
            structs,
        });
    }
    Ok(())
}

fn check_fields(db: &Db) -> Result<(), DcbError> {
    for i in 0..db.count(Table::Field) {
        let f = read_field(db, i).ok_or_else(|| unreachable_read("fields"))?;
        if spec(f.data_type).is_none() {
            return Err(DcbError::UnknownType {
                index: i,
                code: f.data_type,
            });
        }
        let conversion = f.conversion & 0xFF;
        if conversion > 3 {
            return Err(DcbError::UnknownConversion {
                index: i,
                code: conversion,
            });
        }
    }
    Ok(())
}

fn check_struct_shapes(db: &Db) -> Result<(), DcbError> {
    let (structs, fields) = (db.count(Table::Struct), db.count(Table::Field));
    let mut total = 0u64;
    for i in 0..structs {
        let s = read_struct(db, i).ok_or_else(|| unreachable_read("structs"))?;
        if s.parent != NONE && s.parent >= structs {
            return Err(DcbError::Parent {
                index: i,
                parent: s.parent,
            });
        }
        total += u64::from(s.field_count);
        let (first, end) = (
            u32::from(s.first_field),
            u32::from(s.first_field) + u32::from(s.field_count),
        );
        if end > fields {
            return Err(DcbError::Fields {
                index: i,
                first,
                end,
            });
        }
        // The reference's index is a `u16` compared to `end`: from 65,536 on, it wraps around forever.
        if end > u32::from(u16::MAX) {
            return Err(DcbError::FieldIndex { index: i, end });
        }
    }
    // Bounds the work of the size computation: each field is read only a bounded number of times.
    if total > u64::from(fields) {
        return Err(DcbError::FieldTotal {
            declared: total,
            count: fields,
        });
    }
    Ok(())
}

/// Size of a field in an instance; `class_size` gives that of an already-computed struct.
fn field_size(f: &FieldDef, class_size: u64) -> u64 {
    if f.conversion & 0xFF != 0 {
        return 8;
    }
    match f.data_type {
        0x01 | 0x02 | 0x06 => 1,
        0x03 | 0x07 => 2,
        0x04 | 0x08 | 0x0A | 0x0B | 0x0D | 0x0F => 4,
        0x05 | 0x09 | 0x0C | 0x110 | 0x210 => 8,
        0x0E => 16,
        0x310 => 20,
        _ => class_size,
    }
}

/// Struct on which the size computation of `s` depends at step `cursor` (0 = parent, then one field per step).
fn dependency(db: &Db, s: &StructDef, i: u32, cursor: u32) -> Result<Option<u32>, DcbError> {
    if cursor == 0 {
        return Ok((s.parent != NONE).then_some(s.parent));
    }
    let k = u32::from(s.first_field) + cursor - 1;
    let f = read_field(db, k).ok_or_else(|| unreachable_read("fields"))?;
    if f.conversion & 0xFF != 0 || f.data_type != CLASS {
        return Ok(None);
    }
    let target = u32::from(f.index);
    if f.index == NO_FIELD_INDEX || target >= db.count(Table::Struct) {
        return Err(DcbError::Parent {
            index: i,
            parent: target,
        });
    }
    Ok(Some(target))
}

fn own_size(db: &Db, s: &StructDef, sizes: &[Option<u64>]) -> Result<u64, DcbError> {
    let mut total = 0u64;
    for k in 0..u32::from(s.field_count) {
        let f = read_field(db, u32::from(s.first_field) + k)
            .ok_or_else(|| unreachable_read("fields"))?;
        let class = sizes.get(usize::from(f.index)).copied().flatten();
        total = total.saturating_add(field_size(&f, class.unwrap_or(0)));
    }
    Ok(total.saturating_add(match s.parent {
        NONE => 0,
        p => sizes[p as usize].unwrap_or(0),
    }))
}

/// Sizes computed (inheritance included) by depth-first traversal with an explicit stack; a loop of parents
/// or classes is rejected (`Parent`).
fn compute_sizes(db: &Db) -> Result<Vec<u64>, DcbError> {
    let n = db.count(Table::Struct) as usize;
    let mut sizes: Vec<Option<u64>> = vec![None; n];
    let mut gray = vec![false; n];
    for root in 0..n {
        if sizes[root].is_some() {
            continue;
        }
        gray[root] = true;
        let mut stack: Vec<(u32, u32)> = vec![(root as u32, 0)];
        while let Some(&(i, cursor)) = stack.last() {
            let s = read_struct(db, i).ok_or_else(|| unreachable_read("structs"))?;
            if cursor > u32::from(s.field_count) {
                sizes[i as usize] = Some(own_size(db, &s, &sizes)?);
                gray[i as usize] = false;
                stack.pop();
                continue;
            }
            if let Some(top) = stack.last_mut() {
                top.1 += 1;
            }
            let Some(dep) = dependency(db, &s, i, cursor)? else {
                continue;
            };
            if gray[dep as usize] {
                return Err(DcbError::Parent {
                    index: i,
                    parent: dep,
                });
            }
            if sizes[dep as usize].is_none() {
                gray[dep as usize] = true;
                stack.push((dep, 0));
            }
        }
    }
    Ok(sizes.into_iter().map(|s| s.unwrap_or(0)).collect())
}

fn check_struct_sizes(db: &Db, computed: &[u64]) -> Result<(), DcbError> {
    for (i, &size) in computed.iter().enumerate() {
        let s = read_struct(db, i as u32).ok_or_else(|| unreachable_read("structs"))?;
        if u64::from(s.size) != size {
            return Err(DcbError::StructSize {
                index: i as u32,
                declared: s.size,
                computed: size,
            });
        }
    }
    Ok(())
}

/// For each struct, the nearest ancestor that has fields (`NONE`: none), so that a read
/// never walks empty ancestors. Call after `check` (parents exist, no loop).
pub(crate) fn field_parents(db: &Db) -> Vec<u32> {
    const UNSET: u32 = NONE - 1;
    let n = db.count(Table::Struct) as usize;
    let mut out = vec![UNSET; n];
    let mut path = Vec::new();
    let parent = |i: u32| read_struct(db, i).map_or(NONE, |s| s.parent);
    let has_fields = |i: u32| read_struct(db, i).is_some_and(|s| s.field_count > 0);
    for root in 0..n as u32 {
        let mut cur = root;
        // `path`: structs whose answer is that of their parent (empty).
        let answer = loop {
            match out.get(cur as usize).copied() {
                Some(known) if known != UNSET => break known,
                None => break NONE,
                _ => {}
            }
            let p = parent(cur);
            if p == NONE || has_fields(p) {
                out[cur as usize] = p;
                break p;
            }
            path.push(cur);
            cur = p;
        };
        for i in path.drain(..) {
            out[i as usize] = answer;
        }
    }
    out
}

/// All the checks, in order; returns the sizes and the data offsets (cumulative, in mapping
/// order) of each struct.
pub(crate) fn check(db: &Db) -> Result<(Vec<u64>, Vec<u64>), DcbError> {
    check_mapping(db)?;
    check_fields(db)?;
    check_struct_shapes(db)?;
    let sizes = compute_sizes(db)?;
    check_struct_sizes(db, &sizes)?;
    let mut starts = Vec::with_capacity(sizes.len());
    let mut pos = db.layout().data_start;
    for (n, &size) in sizes.iter().enumerate() {
        starts.push(pos);
        let (instances, _) =
            read_mapping(db, n as u32).ok_or_else(|| unreachable_read("mappings"))?;
        pos = pos.saturating_add(u64::from(instances).saturating_mul(size));
    }
    let actual = db.bytes().len() as u64;
    if pos != actual {
        return Err(DcbError::Length {
            expected: pos,
            actual,
        });
    }
    Ok((sizes, starts))
}
