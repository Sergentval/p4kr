//! Document tree as unforge builds it: one element per node, in table order; attributes read
//! consecutively (the first-attribute index is ignored); non-blank content as CDATA; attached to its parent
//! if already created, otherwise root. Index arena, no DOM; texts are borrowed from the string table,
//! never copied (the same content shared by thousands of nodes costs nothing here).

use crate::table::Tables;
use crate::{CryXmlError, Result};
use p4kr_xmlwrite::names::is_valid_name;
use p4kr_xmlwrite::{Child, Doc, Element};
use std::borrow::Cow;
use std::collections::HashMap;

const BUGGED: &str = "BUGGED";

/// Per-string checks, memoised by offset: a string shared by thousands of rows is scanned
/// (valid name, blank, hashed) only once, not once per row.
#[derive(Default)]
struct Memo<'a> {
    valid_name: HashMap<i32, bool>,
    blank: HashMap<i32, bool>,
    /// Identity of an attribute key: same text (at distinct offsets) → same number.
    key_ids: HashMap<i32, usize>,
    interned: HashMap<&'a str, usize>,
}

impl<'a> Memo<'a> {
    fn check_name(&mut self, offset: i32, s: &str) -> Result<()> {
        let ok = *self
            .valid_name
            .entry(offset)
            .or_insert_with(|| is_valid_name(s));
        if ok {
            Ok(())
        } else {
            Err(CryXmlError::BadName)
        }
    }

    fn is_blank(&mut self, offset: i32, s: &str) -> bool {
        *self
            .blank
            .entry(offset)
            .or_insert_with(|| s.chars().all(char::is_whitespace))
    }

    fn key_id(&mut self, offset: i32, s: &'a str) -> usize {
        let next = self.interned.len();
        let interned = &mut self.interned;
        *self
            .key_ids
            .entry(offset)
            .or_insert_with(|| *interned.entry(s).or_insert(next))
    }
}

pub(crate) fn build(t: &Tables) -> Result<Doc<'_>> {
    let text = |offset: i32| t.strings.get(&offset).map(String::as_str);
    let mut memo = Memo::default();
    let mut elements: Vec<Element<'_>> = Vec::with_capacity(t.nodes.len());
    let mut root = None;
    let mut next_attr = 0usize;
    for (id, node) in t.nodes.iter().enumerate() {
        let name = text(node.name).ok_or(CryXmlError::MissingString)?;
        memo.check_name(node.name, name)?;
        let mut el = Element {
            name: Cow::Borrowed(name),
            attrs: Vec::new(),
            children: Vec::new(),
        };
        // SetAttribute: same name → value replaced in place (indexed to stay linear).
        let mut slots: HashMap<usize, usize> = HashMap::new();
        for _ in 0..node.attr_count.max(0) {
            let &(name_off, value_off) =
                t.attrs.get(next_attr).ok_or(CryXmlError::AttributeIndex)?;
            let key = text(name_off).ok_or(CryXmlError::MissingString)?;
            let value = text(value_off).unwrap_or(BUGGED);
            memo.check_name(name_off, key)?;
            let id = memo.key_id(name_off, key);
            match slots.get(&id) {
                Some(&i) => el.attrs[i].1 = Cow::Borrowed(value),
                None => {
                    slots.insert(id, el.attrs.len());
                    el.attrs.push((Cow::Borrowed(key), Cow::Borrowed(value)));
                }
            }
            next_attr += 1;
        }
        match text(node.content) {
            Some(s) if !memo.is_blank(node.content, s) => {
                el.children.push(Child::CData(Cow::Borrowed(s)))
            }
            Some(_) => {}
            None => el.children.push(Child::CData(Cow::Borrowed(BUGGED))),
        }
        elements.push(el);
        match usize::try_from(node.parent).ok().filter(|&p| p <= id) {
            Some(p) if p == id => return Err(CryXmlError::SelfParent),
            Some(p) => elements[p].children.push(Child::Element(id)),
            None if root.is_some() => return Err(CryXmlError::SecondRoot),
            None => root = Some(id),
        }
    }
    Ok(Doc { elements, root })
}
