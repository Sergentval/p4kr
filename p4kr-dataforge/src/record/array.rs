//! Arrays: `u32 count`, `u32 first index`, then at most `MAX_NODES` elements read from the type's
//! table; a fault on an element becomes the field's error element and the array continues.

use super::*;
use crate::tables::spec;

pub(super) struct Array {
    el: usize,
    field: FieldDef,
    first: u32,
    count: u32,
    i: u32,
}

impl<'a> Builder<'_, 'a> {
    pub(super) fn array_field(&mut self, f: &FieldDef) -> Result<Option<Frame>, Fault> {
        let name = self.blob(f.name)?;
        let el = self.element(name)?;
        let count = self.u32()?;
        let first = self.u32()?;
        if count == 0 {
            self.discard(el);
            return Ok(None);
        }
        Ok(Some(Frame::Array(Array {
            el,
            field: *f,
            first,
            count,
            i: 0,
        })))
    }

    pub(super) fn array_step(&mut self) -> Option<Got<'a>> {
        self.steps += 1;
        let Some(Frame::Array(a)) = self.frames.last() else {
            return None;
        };
        if a.i >= a.count || a.i >= MAX_NODES {
            let el = a.el;
            self.frames.pop();
            if self.doc[el].children.is_empty() {
                return Some(Ok(self.discard(el)));
            }
            return Some(Ok(Some(Item::Elem(el))));
        }
        let (field, index) = (a.field, a.first.wrapping_add(a.i));
        match self.array_item(&field, index) {
            Step::Done(got) => self.array_got(got),
            Step::Push(frame, _) => self.frames.push(frame),
        }
        None
    }

    /// Stores an element of the array on top; a fault becomes the field's error element.
    pub(super) fn array_got(&mut self, got: Got<'a>) {
        let Some(Frame::Array(a)) = self.frames.last_mut() else {
            return;
        };
        a.i += 1;
        let el = a.el;
        let child = match got {
            Ok(Some(Item::Elem(e))) => Some(e),
            Ok(_) => None,
            Err(fault) => {
                let (name, code) = (self.doc[el].name.clone(), a.field.data_type);
                let text = format!(
                    "Error reading array property {name} of type {}: {}",
                    type_text(code),
                    fault.exception()
                );
                // The field name was already used to create the array: its check cannot fail.
                self.valued(name, Cow::Owned(text)).ok()
            }
        };
        if let Some(e) = child {
            self.doc[el].children.push(Child::Element(e));
        }
    }

    fn array_item(&mut self, f: &FieldDef, index: u32) -> Step<'a> {
        let Some(&(code, _, table, label, tag)) = spec(f.data_type) else {
            return Step::Done(Ok(None));
        };
        let target = match code {
            STRONG | WEAK => self.pointer_item(table, label, index),
            CLASS if f.index == 0xFFFF => Ok(None),
            CLASS => {
                let s = u32::from(f.index);
                self.mapping_name(s).map(|name| Some((name, s, index)))
            }
            _ => return Step::Done(self.value_item(code, (table, label, tag), index)),
        };
        match target {
            Ok(Some(target)) => self.struct_at(target, Pending::Nothing),
            other => Step::Done(other.map(|_| None)),
        }
    }

    /// Row `index` of a value table; the index equal to the row count reads the next table.
    fn row(&self, t: Table, label: &str, index: u32) -> Result<&'a [u8], Fault> {
        let count = self.db.count(t);
        if index > count {
            return Err(Fault::out_of_range(index, label, count));
        }
        let pos = usize::try_from(self.db.table_pos(t, u64::from(index))).ok();
        let n = t.row_size() as usize;
        pos.and_then(|p| self.db.bytes().get(p..p.checked_add(n)?))
            .ok_or(Fault::EndOfStream)
    }

    /// Pointer of an array (strong or weak, both expanded): struct `0xFFFFFFFF` → nothing.
    fn pointer_item(
        &mut self,
        t: Table,
        label: &str,
        index: u32,
    ) -> Result<Option<Target<'a>>, Fault> {
        let (s, v) = pointer_parts(self.row(t, label, index)?);
        if s == NONE {
            return Ok(None);
        }
        Ok(Some((self.mapping_name(s)?, s, u32::from(v))))
    }

    fn value_item(&mut self, code: u16, at: (Table, &str, &'static str), index: u32) -> Got<'a> {
        let (table, label, tag) = at;
        let b = self.row(table, label, index)?;
        let value = match code {
            BOOL => Cow::Borrowed(if b.first().is_some_and(|&x| x != 0) {
                "1"
            } else {
                "0"
            }),
            STR | LOCALE | ENUM => self.text(u32::from_le_bytes(arr(b)))?,
            REFERENCE => Cow::Owned(guid_text(&reference_guid(b))),
            _ => Cow::Owned(scalar_text(code, b)),
        };
        if value.is_empty() {
            return Ok(None);
        }
        Ok(Some(Item::Elem(self.valued(Cow::Borrowed(tag), value)?)))
    }
}
