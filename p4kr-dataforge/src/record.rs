//! One record → XML tree, following the reference's rules (unforge, concepts only).
//!
//! The reference reads through nested generators and exceptions; here, an explicit stack of frames: `Walk`
//! (the fields of a struct, in sequence, with their counter) and `Array` (the elements of an array,
//! module `array`). A finished or failed frame hands a `Got` to its parent, which stores it or catches it
//! where the reference has its `try`: attribute field (error text in an attribute), array element
//! (error element), record (`<Error value=… />`). The read position is shared, like the reference's
//! stream: only reading a struct by index restores it on exit.

mod array;

use crate::header::{NONE, Table};
use crate::tables::types::*;
use crate::tables::{
    FieldDef, RecordDef, StructDef, arr, pointer_parts, reference_guid, type_text,
};
use crate::value::{f32_text, f64_text, guid_text};
use crate::{Db, Fault};
use array::Array;
use p4kr_xmlwrite::names::{NameFault, NameKind, dotnet_name_fault, split_name};
use p4kr_xmlwrite::{Child, Doc, Element, MAX_OUTPUT};
use std::borrow::Cow;
use std::collections::HashSet;
use std::hash::{BuildHasherDefault, Hasher};

/// Fields counted per struct, elements per array (the reference's `MaxNodes`).
const MAX_NODES: u32 = 10_000;
/// Structs read by index and still open, beyond which the reference descends no further.
const MAX_DEPTH: usize = 100;
/// Deliberate deviation from the reference, like `MAX_OUTPUT`: an absurd record (pointers fanning out
/// over 100 levels) would demand endless work. One step: a field, an array element, or a
/// field range of an ancestor; beyond that, `TooMuchWork`. The estimated size (`size`, bounded by
/// `MAX_OUTPUT`) also counts abandoned subtrees: it is a measure of work, not of the output.
const MAX_STEPS: u64 = 1 << 24;
/// Deliberate deviation, like `MAX_STEPS`: bytes of names and texts read per record, written or not (a
/// name read then abandoned on a cycle costs no step but is re-read in full). Real names are
/// tiny: the bound is far from any real output (at most `MAX_OUTPUT`).
const MAX_SCANNED: u64 = 1 << 30;
/// Deliberate deviation: frames open at once. Inline classes nest with no depth bound;
/// the reference calls itself recursively for each class and dies well before this number.
const MAX_FRAMES: usize = 1 << 16;

/// Record abandoned: size or work bounds exceeded.
pub(crate) enum Halt {
    Size,
    Work,
}

type Text<'a> = Cow<'a, str>;

enum Item<'a> {
    Attr(Text<'a>, Text<'a>),
    Elem(usize),
}

type Node<'a> = Option<Item<'a>>;
type Got<'a> = Result<Node<'a>, Fault>;
/// Struct to read by index: (already-checked name of the element receiving it, struct, instance).
type Target<'a> = (Text<'a>, u32, u32);

/// Value of a simple type read from `b` (type width), like the reference's `$"{value}"`.
fn scalar_text(code: u16, b: &[u8]) -> String {
    match code {
        I8 => i8::from_le_bytes(arr(b)).to_string(),
        I16 => i16::from_le_bytes(arr(b)).to_string(),
        I32 => i32::from_le_bytes(arr(b)).to_string(),
        I64 => i64::from_le_bytes(arr(b)).to_string(),
        U16 => u16::from_le_bytes(arr(b)).to_string(),
        U32 => u32::from_le_bytes(arr(b)).to_string(),
        U64 => u64::from_le_bytes(arr(b)).to_string(),
        SINGLE => f32_text(f32::from_le_bytes(arr(b))),
        DOUBLE => f64_text(f64::from_le_bytes(arr(b))),
        GUID => guid_text(&arr(b)),
        // `u8`, and boolean of an attribute field: the raw byte in decimal.
        _ => b.first().copied().unwrap_or(0).to_string(),
    }
}

/// Rejection by `CreateElement` / `CreateAttribute`; write-time rejections come later.
fn created(name: &str, kind: NameKind) -> Result<(), Fault> {
    match dotnet_name_fault(name, kind) {
        Some(NameFault::Create(m)) => Err(Fault::Name(m)),
        _ => Ok(()),
    }
}

/// `CreateAttributeWithValue`: name checked, empty value → no attribute.
fn attr<'a>(name: Text<'a>, value: Text<'a>) -> Got<'a> {
    created(&name, NameKind::Attribute)?;
    Ok((!value.is_empty()).then_some(Item::Attr(name, value)))
}

/// Identity of an attribute in its list: local name and namespace (`xml`, `xmlns`, or none: any
/// other prefix has none).
fn attr_key(name: &str) -> (u8, &str) {
    match split_name(name) {
        ("xml", local) => (1, local),
        ("xmlns", local) => (2, local),
        ("", "xmlns") => (2, "xmlns"),
        (_, local) => (0, local),
    }
}

/// FNV-1a hash of an attribute's identity; a collision only costs one more search.
fn key_hash(name: &str) -> u64 {
    let (ns, local) = attr_key(name);
    std::iter::once(ns)
        .chain(local.bytes())
        .fold(0xcbf2_9ce4_8422_2325, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
        })
}

/// Already-mixed hashes: the table takes them as they are.
#[derive(Default)]
struct Spread(u64);

impl Hasher for Spread {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        self.0 = bytes
            .iter()
            .fold(self.0, |h, &b| h.rotate_left(8) ^ u64::from(b));
    }
    fn write_u64(&mut self, v: u64) {
        self.0 = v;
    }
}

type KeySet = HashSet<u64, BuildHasherDefault<Spread>>;

/// What the parent frame does with the result of its child frame.
enum Pending {
    Nothing,
    /// Attribute field in progress (inline class): stored, or caught as error text.
    Attr(FieldDef),
    /// Strong pointer: the struct read is wrapped in the field's element.
    Strong(FieldDef),
    /// Array: its element is stored if it is not empty.
    Array,
}

/// Fields of a struct, read in sequence: ranges `ranges[start..end]` of the builder (parents
/// first), current range `range`, next field `next` in that range.
struct Walk {
    el: usize,
    start: usize,
    end: usize,
    range: usize,
    next: u32,
    counter: u32,
    /// Opened by reading a struct by index: position to restore on exit.
    saved: Option<u64>,
    pending: Pending,
    /// Hashes of the identities of attributes already stored (a duplicate is then searched for properly).
    keys: KeySet,
}

enum Frame {
    Walk(Walk),
    Array(Array),
}

enum Step<'a> {
    Done(Got<'a>),
    Push(Frame, Pending),
}

struct Builder<'d, 'a> {
    db: &'d Db<'a>,
    doc: Vec<Element<'a>>,
    /// Field ranges of the open `Walk` frames, stacked like them.
    ranges: Vec<(u32, u32)>,
    pos: u64,
    structs: Vec<(u32, u32)>,
    frames: Vec<Frame>,
    steps: u64,
    size: u64,
    /// Bytes of names and texts read (`MAX_SCANNED`).
    scanned: u64,
}

/// The tree of record `index`, `None` if it yields no file.
pub(crate) fn record_doc<'a>(db: &Db<'a>, index: u32) -> Result<Option<Doc<'a>>, Halt> {
    let mut b = Builder {
        db,
        doc: Vec::new(),
        ranges: Vec::new(),
        pos: 0,
        structs: Vec::new(),
        frames: Vec::new(),
        steps: 0,
        size: 0,
        scanned: 0,
    };
    Ok(match b.record(index)? {
        Ok(None) => None,
        Ok(Some(root)) => Some(Doc {
            elements: b.doc,
            root: Some(root),
        }),
        Err(fault) => Some(error_doc(&fault)),
    })
}

fn error_doc(fault: &Fault) -> Doc<'static> {
    let value = Cow::Owned(fault.message().to_string());
    let attrs = vec![(Cow::Borrowed("value"), value)];
    let children = Vec::new();
    let elements = vec![Element {
        name: Cow::Borrowed("Error"),
        attrs,
        children,
    }];
    Doc {
        elements,
        root: Some(0),
    }
}

impl<'a> Builder<'_, 'a> {
    // --- Record --------------------------------------------------------------------------------------

    fn record(&mut self, index: u32) -> Result<Result<Option<usize>, Fault>, Halt> {
        let (rec, name) = match self.record_name(index) {
            Ok(x) => x,
            Err(fault) => return Ok(Err(fault)),
        };
        let v = u32::from(rec.variant);
        let got = match self.struct_at((name, rec.struct_index, v), Pending::Nothing) {
            Step::Done(got) => got,
            Step::Push(frame, _) => self.run(frame)?,
        };
        Ok(match got {
            Ok(Some(Item::Elem(e))) => self.record_attrs(&rec, e).map(|()| Some(e)),
            Ok(_) => Ok(None),
            Err(fault) => Err(fault),
        })
    }

    fn record_name(&mut self, index: u32) -> Result<(RecordDef, Text<'a>), Fault> {
        let rec = self.db.record_def(index).ok_or(Fault::EndOfStream)?;
        let name = self.blob(rec.name)?;
        created(&name, NameKind::Element)?;
        Ok((rec, name))
    }

    /// `__type`, `__ref`, `__path`, `__team`, added like the other attributes (same name: replaced).
    fn record_attrs(&mut self, rec: &RecordDef, el: usize) -> Result<(), Fault> {
        let def = self.struct_def(rec.struct_index)?;
        let kind = self.blob(def.name)?;
        self.add_attr(el, "__type", kind);
        self.add_attr(el, "__ref", Cow::Owned(guid_text(&rec.guid)));
        let path = self.text(rec.path)?;
        self.add_attr(el, "__path", path);
        let team = self.blob(rec.team)?;
        self.add_attr(el, "__team", team);
        Ok(())
    }

    fn add_attr(&mut self, el: usize, name: &'static str, value: Text<'a>) {
        let key = attr_key(name);
        self.size += (name.len() + value.len() + 4) as u64;
        let attrs = &mut self.doc[el].attrs;
        attrs.retain(|(n, _)| attr_key(n) != key);
        attrs.push((Cow::Borrowed(name), value));
    }

    // --- Loop ----------------------------------------------------------------------------------------

    fn run(&mut self, first: Frame) -> Result<Got<'a>, Halt> {
        self.frames.push(first);
        let mut carry: Option<Got<'a>> = None;
        loop {
            if self.steps > MAX_STEPS
                || self.scanned > MAX_SCANNED
                || self.frames.len() > MAX_FRAMES
            {
                return Err(Halt::Work);
            }
            if self.size > MAX_OUTPUT as u64 {
                return Err(Halt::Size);
            }
            if let Some(got) = carry.take() {
                if self.frames.is_empty() {
                    return Ok(got);
                }
                carry = self.deliver(got);
                continue;
            }
            carry = match self.frames.last() {
                Some(Frame::Walk(_)) => self.walk_step(),
                Some(Frame::Array(_)) => self.array_step(),
                None => return Ok(Ok(None)),
            };
        }
    }

    /// Stores a child frame's result in the top frame; `Some` if the latter fails.
    fn deliver(&mut self, got: Got<'a>) -> Option<Got<'a>> {
        match self.frames.last_mut() {
            Some(Frame::Walk(w)) => {
                let pending = std::mem::replace(&mut w.pending, Pending::Nothing);
                self.deliver_walk(pending, got)
            }
            Some(Frame::Array(_)) => {
                self.array_got(got);
                None
            }
            None => Some(got),
        }
    }

    fn deliver_walk(&mut self, pending: Pending, got: Got<'a>) -> Option<Got<'a>> {
        let (field, got) = match pending {
            Pending::Nothing => return None,
            Pending::Array => {
                if let Ok(Some(item)) = got {
                    self.append(item);
                    self.count_up();
                }
                return None;
            }
            Pending::Attr(f) => (f, got),
            Pending::Strong(f) => match got {
                Ok(Some(Item::Elem(e))) => (f, self.wrap(&f, e)),
                other => (f, other),
            },
        };
        match got.or_else(|fault| self.catch(&field, &fault)) {
            Ok(node) => {
                if let Some(item) = node {
                    self.append(item);
                }
                self.count_up();
                None
            }
            Err(fault) => self.fail_walk(fault),
        }
    }

    fn walk_step(&mut self) -> Option<Got<'a>> {
        self.steps += 1;
        let Some(k) = self.advance() else {
            return Some(self.finish_walk());
        };
        let Some(f) = self.db.field_def(k) else {
            return self.fail_walk(Fault::EndOfStream);
        };
        if f.conversion & 0xFF != 0 {
            return match self.array_field(&f) {
                Ok(Some(frame)) => self.push(frame, Pending::Array),
                Ok(None) => None,
                Err(fault) => self.fail_walk(fault),
            };
        }
        match self.attr_field(&f) {
            Step::Done(got) => self.deliver_walk(Pending::Attr(f), got),
            Step::Push(frame, pending) => self.push(frame, pending),
        }
    }

    /// Next field of the top `Walk` frame, `None` at the end of the struct or when the counter is full.
    fn advance(&mut self) -> Option<u32> {
        let Some(Frame::Walk(w)) = self.frames.last_mut() else {
            return None;
        };
        if w.counter >= MAX_NODES {
            return None;
        }
        while let Some(&(first, end)) = self.ranges.get(w.range).filter(|_| w.range < w.end) {
            let k = first + w.next;
            if k < end {
                w.next += 1;
                return Some(k);
            }
            w.range += 1;
            w.next = 0;
        }
        None
    }

    fn push(&mut self, frame: Frame, pending: Pending) -> Option<Got<'a>> {
        if let Some(Frame::Walk(w)) = self.frames.last_mut() {
            w.pending = pending;
        }
        self.frames.push(frame);
        None
    }

    fn pop_walk(&mut self) -> Option<Walk> {
        match self.frames.pop() {
            Some(Frame::Walk(w)) => {
                self.ranges.truncate(w.start);
                if let Some(saved) = w.saved {
                    self.pos = saved;
                    self.structs.pop();
                }
                Some(w)
            }
            _ => None,
        }
    }

    fn finish_walk(&mut self) -> Got<'a> {
        let Some(w) = self.pop_walk() else {
            return Ok(None);
        };
        let e = &self.doc[w.el];
        if e.attrs.is_empty() && e.children.is_empty() {
            return Ok(self.discard(w.el));
        }
        Ok(Some(Item::Elem(w.el)))
    }

    fn fail_walk(&mut self, fault: Fault) -> Option<Got<'a>> {
        if let Some(w) = self.pop_walk() {
            self.discard(w.el);
        }
        Some(Err(fault))
    }

    /// Abandons element `el`: everything created since it (its content, or leftovers) goes with it.
    fn discard(&mut self, el: usize) -> Node<'a> {
        self.doc.truncate(el);
        None
    }

    fn count_up(&mut self) {
        if let Some(Frame::Walk(w)) = self.frames.last_mut() {
            w.counter += 1;
        }
    }

    /// Stores into the top frame's element; an attribute of the same identity is removed first.
    fn append(&mut self, item: Item<'a>) {
        let Some(Frame::Walk(w)) = self.frames.last_mut() else {
            return;
        };
        match item {
            Item::Elem(e) => self.doc[w.el].children.push(Child::Element(e)),
            Item::Attr(name, value) => {
                self.size += (name.len() + value.len() + 4) as u64;
                let attrs = &mut self.doc[w.el].attrs;
                if !w.keys.insert(key_hash(&name)) {
                    let key = attr_key(&name);
                    attrs.retain(|(n, _)| attr_key(n) != key);
                }
                attrs.push((name, value));
            }
        }
    }

    // --- Reads ---------------------------------------------------------------------------------------

    /// String of the text at offset `off`, counted in `scanned`; every text read goes through here.
    fn text(&mut self, off: u32) -> Result<Text<'a>, Fault> {
        let s = self.db.text_cow(off)?;
        self.scanned += s.len() as u64;
        Ok(s)
    }

    /// String of the blob (names), counted in `scanned`; every name read goes through here.
    fn blob(&mut self, off: u32) -> Result<Text<'a>, Fault> {
        let s = self.db.blob_cow(off)?;
        self.scanned += s.len() as u64;
        Ok(s)
    }

    fn take_n(&mut self, n: usize) -> Result<&'a [u8], Fault> {
        let (bytes, at) = (self.db.bytes(), usize::try_from(self.pos).ok());
        match at.and_then(|p| bytes.get(p..p.checked_add(n)?)) {
            Some(b) => {
                self.pos += n as u64;
                Ok(b)
            }
            None => {
                // Partial read: the reference's stream advances to its end.
                self.pos = self.pos.max(bytes.len() as u64);
                Err(Fault::EndOfStream)
            }
        }
    }

    fn u32(&mut self) -> Result<u32, Fault> {
        self.take_n(4).map(|b| u32::from_le_bytes(arr(b)))
    }

    fn struct_def(&self, s: u32) -> Result<StructDef, Fault> {
        let count = self.db.count(Table::Struct);
        if s > count {
            return Err(Fault::out_of_range(s, "Struct Definition", count));
        }
        self.db.struct_def(s).ok_or(Fault::EndOfStream)
    }

    fn mapping(&self, s: u32) -> Result<(u32, u32), Fault> {
        let count = self.db.count(Table::Mapping);
        if s > count {
            return Err(Fault::out_of_range(s, "Data Mapping", count));
        }
        self.db.mapping(s).ok_or(Fault::EndOfStream)
    }

    /// Checked name of a struct's element, like `CreateElement(name)`.
    fn struct_name(&mut self, s: u32) -> Result<Text<'a>, Fault> {
        let name = self.blob(self.struct_def(s)?.name)?;
        created(&name, NameKind::Element)?;
        Ok(name)
    }

    /// Checked name of a mapping: that of the struct it designates.
    fn mapping_name(&mut self, s: u32) -> Result<Text<'a>, Fault> {
        let (_, target) = self.mapping(s)?;
        self.struct_name(target)
    }

    /// Adds an element with an already-checked name.
    fn push_element(&mut self, name: Text<'a>) -> usize {
        self.size += name.len() as u64 + 8;
        self.doc.push(Element {
            name,
            attrs: Vec::new(),
            children: Vec::new(),
        });
        self.doc.len() - 1
    }

    fn element(&mut self, name: Text<'a>) -> Result<usize, Fault> {
        created(&name, NameKind::Element)?;
        Ok(self.push_element(name))
    }

    /// `CreateElementWithValue` with a text: element and its `value` attribute.
    fn valued(&mut self, name: Text<'a>, value: Text<'a>) -> Result<usize, Fault> {
        let el = self.element(name)?;
        self.size += value.len() as u64 + 9;
        self.doc[el].attrs.push((Cow::Borrowed("value"), value));
        Ok(el)
    }

    // --- Structs -------------------------------------------------------------------------------------

    /// Reads struct `s`, instance `v`, into an element of the given name (already checked, as the
    /// reference creates it beforehand): depth or cycle → nothing, no element.
    fn struct_at(&mut self, target: Target<'a>, pending: Pending) -> Step<'a> {
        let (name, s, v) = target;
        if self.structs.len() > MAX_DEPTH || self.structs.contains(&(s, v)) {
            return Step::Done(Ok(None));
        }
        let el = self.push_element(name);
        let saved = self.pos;
        self.structs.push((s, v));
        let opened = self.locate(s, v).and_then(|pos| {
            self.pos = pos;
            self.walk(el, s, Some(saved))
        });
        match opened {
            Ok(w) => Step::Push(Frame::Walk(w), pending),
            Err(fault) => {
                self.pos = saved;
                self.structs.pop();
                self.discard(el);
                Step::Done(Err(fault))
            }
        }
    }

    fn locate(&mut self, s: u32, v: u32) -> Result<u64, Fault> {
        let def = self.struct_def(s)?;
        let (count, _) = self.mapping(s)?;
        if count < v {
            let name = self.blob(def.name)?;
            return Err(Fault::IndexOutOfRange(format!(
                "Variant Index {v} is out of range for struct {name} with count {count}"
            )));
        }
        self.db.instance_pos(s, v).ok_or_else(|| {
            Fault::KeyNotFound(format!(
                "Struct Index {s} not found in Struct to Data Offset Map"
            ))
        })
    }

    /// Field frame of `s`: ranges of the ancestors that have fields (empty ones are skipped by
    /// `field_parent`), one step each.
    fn walk(&mut self, el: usize, s: u32, saved: Option<u64>) -> Result<Walk, Fault> {
        let start = self.ranges.len();
        let mut cur = Some(s);
        while let Some(i) = cur {
            let Some(def) = self.db.struct_def(i) else {
                self.ranges.truncate(start);
                return Err(Fault::EndOfStream);
            };
            let first = u32::from(def.first_field);
            if def.field_count > 0 {
                self.ranges
                    .push((first, first + u32::from(def.field_count)));
                self.steps += 1;
            }
            // Parent loops are rejected at open time; the bound prevents any endless loop.
            let depth = self.ranges.len() - start;
            cur = self
                .db
                .field_parent(i)
                .filter(|_| depth <= self.db.struct_count());
        }
        self.ranges[start..].reverse();
        Ok(Walk {
            el,
            start,
            end: self.ranges.len(),
            range: start,
            next: 0,
            counter: 0,
            saved,
            pending: Pending::Nothing,
            keys: KeySet::default(),
        })
    }

    // --- Attribute fields ----------------------------------------------------------------------------

    fn attr_field(&mut self, f: &FieldDef) -> Step<'a> {
        match f.data_type {
            CLASS => self.class_field(f),
            STRONG => match self.strong_target() {
                Ok(Some(target)) => self.struct_at(target, Pending::Strong(*f)),
                other => Step::Done(other.map(|_| None)),
            },
            WEAK => Step::Done(self.weak_field(f)),
            REFERENCE => Step::Done(self.reference_field(f)),
            LOCALE => Step::Done(self.locale_field(f)),
            code => Step::Done(self.simple_field(f, code)),
        }
    }

    /// Inline class: its fields follow in the same bytes.
    fn class_field(&mut self, f: &FieldDef) -> Step<'a> {
        let s = u32::from(f.index);
        let opened = self
            .struct_def(s)
            .and_then(|_| self.blob(f.name))
            .and_then(|name| self.element(name));
        match opened.and_then(|el| self.walk(el, s, None)) {
            Ok(w) => Step::Push(Frame::Walk(w), Pending::Attr(*f)),
            Err(fault) => Step::Done(Err(fault)),
        }
    }

    /// Strong pointer: (element named by the struct, struct, instance), `None` if null.
    fn strong_target(&mut self) -> Result<Option<Target<'a>>, Fault> {
        let (s, v) = pointer_parts(self.take_n(8)?);
        if (s, v) == (NONE, 0xFFFF) {
            return Ok(None);
        }
        Ok(Some((self.struct_name(s)?, s, u32::from(v))))
    }

    fn wrap(&mut self, f: &FieldDef, child: usize) -> Got<'a> {
        let name = self.blob(f.name)?;
        let el = self.element(name)?;
        self.doc[el].children.push(Child::Element(child));
        Ok(Some(Item::Elem(el)))
    }

    fn weak_field(&mut self, f: &FieldDef) -> Got<'a> {
        let (s, v) = pointer_parts(self.take_n(8)?);
        if (s, v) == (NONE, 0xFFFF) {
            return Ok(None);
        }
        let def = self.struct_def(s)?;
        let name = self.blob(f.name)?;
        let target = self.blob(def.name)?;
        let el = self.valued(name, Cow::Owned(format!("{target}[{v:04X}]")))?;
        Ok(Some(Item::Elem(el)))
    }

    fn reference_field(&mut self, f: &FieldDef) -> Got<'a> {
        let guid = reference_guid(self.take_n(20)?);
        if guid == [0; 16] {
            return Ok(None);
        }
        attr(self.blob(f.name)?, Cow::Owned(guid_text(&guid)))
    }

    /// The offset is read before the name, unlike the other types.
    fn locale_field(&mut self, f: &FieldDef) -> Got<'a> {
        let off = self.u32()?;
        let name = self.blob(f.name)?;
        let value = self.text(off)?;
        attr(name, value)
    }

    fn simple_field(&mut self, f: &FieldDef, code: u16) -> Got<'a> {
        let Some(&(_, _, table, _, _)) = crate::tables::spec(code) else {
            // Unknown type: rejected at open time.
            return Ok(None);
        };
        let name = self.blob(f.name)?;
        let value = match code {
            STR | ENUM => {
                let off = self.u32()?;
                self.text(off)?
            }
            _ => Cow::Owned(scalar_text(code, self.take_n(table.row_size() as usize)?)),
        };
        attr(name, value)
    }

    /// Catch for an attribute field: the name is re-read, and may fail in turn.
    fn catch(&mut self, f: &FieldDef, fault: &Fault) -> Got<'a> {
        let name = self.blob(f.name)?;
        let value = format!(
            "Error reading property {name} of type {}: {}",
            type_text(f.data_type),
            fault.exception()
        );
        attr(name, Cow::Owned(value))
    }
}
