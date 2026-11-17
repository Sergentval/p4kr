//! Writing like `XmlDocument.Save` (indented XmlTextWriter) or like `DataForge.Save`'s `XmlWriter`:
//! shared iterative traversal, settings in `Rules`. An error along the way leaves what precedes in `out`,
//! like the reference's partial file.

use crate::names::{
    NameKind, attribute_name, dotnet_attribute_fault, dotnet_name_fault, element_name,
};
use crate::{Child, Doc, Element, Profile, WriteError};

type Result<T> = std::result::Result<T, WriteError>;

const INDENT: usize = 2;
/// Worst growth of one attribute-value byte when escaped (`&quot;`, `&#x1F;`).
const ESCAPE_GROWTH: usize = 6;

/// What distinguishes the two profiles; the traversal is the same.
struct Rules {
    newline: &'static [u8],
    /// Tab in an attribute value written as `&#x9;` (otherwise as is).
    escape_tab: bool,
    /// Names rejected the way `XmlWriter` does (first character, element prefix, `xmlns`).
    check_names: bool,
}

const DOCUMENT_SAVE: Rules = Rules {
    newline: b"\n",
    escape_tab: false,
    check_names: false,
};

const DATA_FORGE: Rules = Rules {
    newline: b"\r\n",
    escape_tab: true,
    check_names: true,
};

impl Profile {
    fn rules(self) -> &'static Rules {
        match self {
            Profile::DocumentSave => &DOCUMENT_SAVE,
            Profile::DataForge => &DATA_FORGE,
        }
    }
}

/// Output cap: beyond it, `TooLarge` fails and nothing is written. Deliberate deviation from the reference,
/// for absurd inputs only (a chain of 100,000 levels, 1 MB of content shared by thousands of
/// nodes): real files are very far from it, and the reference itself falls over at such depths
/// (.NET's recursive `WriteTo`). The check is an upper bound made before each growth.
pub const MAX_OUTPUT: usize = 256 << 20;

#[derive(Clone, Copy)]
struct Frame {
    id: usize,
    next: usize,
    open_tag: bool,
    mixed: bool,
}

struct Writer<'o> {
    rules: &'static Rules,
    out: &'o mut Vec<u8>,
    stack: Vec<Frame>,
}

pub fn write(doc: &Doc<'_>, profile: Profile, out: &mut Vec<u8>) -> Result<()> {
    let root = doc.root.ok_or(WriteError::NoRoot)?;
    let mut w = Writer {
        rules: profile.rules(),
        out,
        stack: Vec::new(),
    };
    w.start(&doc.elements[root], root)?;
    while let Some(&Frame { id, next, .. }) = w.stack.last() {
        match doc.elements[id].children.get(next) {
            Some(child) => {
                if let Some(f) = w.stack.last_mut() {
                    f.next += 1;
                }
                match child {
                    Child::CData(text) => w.cdata(text)?,
                    Child::Element(kid) => w.start(&doc.elements[*kid], *kid)?,
                }
            }
            None => w.end(&doc.elements[id])?,
        }
    }
    Ok(())
}

impl Writer<'_> {
    /// Refuses to grow the output by `n` bytes beyond the cap.
    fn room(&self, n: usize) -> Result<()> {
        match self.out.len().checked_add(n) {
            Some(len) if len <= MAX_OUTPUT => Ok(()),
            _ => Err(WriteError::TooLarge),
        }
    }

    fn close_open_tag(&mut self) -> Result<()> {
        if let Some(f) = self.stack.last_mut()
            && f.open_tag
        {
            f.open_tag = false;
            self.room(1)?;
            self.out.push(b'>');
        }
        Ok(())
    }

    fn indent(&mut self, level: usize) -> Result<()> {
        let newline = self.rules.newline;
        self.room(newline.len().saturating_add(level.saturating_mul(INDENT)))?;
        self.out.extend_from_slice(newline);
        self.out.resize(self.out.len() + level * INDENT, b' ');
        Ok(())
    }

    /// `XmlWriter` rejection (`DataForge` profile) for an element and its attributes, before writing its tag.
    fn check_names(&self, el: &Element<'_>) -> Result<()> {
        if !self.rules.check_names {
            return Ok(());
        }
        let faults = std::iter::once(dotnet_name_fault(&el.name, NameKind::Element)).chain(
            el.attrs
                .iter()
                .map(|(key, value)| dotnet_attribute_fault(key, value)),
        );
        match faults.flatten().next() {
            Some(fault) => Err(WriteError::Name(fault)),
            None => Ok(()),
        }
    }

    fn start(&mut self, el: &Element<'_>, id: usize) -> Result<()> {
        self.close_open_tag()?;
        self.check_names(el)?;
        // An element opened inside a mixed element is mixed too (`content-and-children` case).
        let parent_mixed = self.stack.last().is_some_and(|f| f.mixed);
        if !self.stack.is_empty() && !parent_mixed {
            self.indent(self.stack.len())?;
        }
        self.stack.push(Frame {
            id,
            next: 0,
            open_tag: true,
            mixed: parent_mixed,
        });
        let name = element_name(&el.name);
        self.room(1 + name.len())?;
        self.out.push(b'<');
        self.out.extend_from_slice(name.as_bytes());
        for (key, value) in &el.attrs {
            let key = attribute_name(key);
            self.room(
                key.len()
                    .saturating_add(value.len().saturating_mul(ESCAPE_GROWTH))
                    + 4,
            )?;
            self.out.push(b' ');
            self.out.extend_from_slice(key.as_bytes());
            self.out.extend_from_slice(b"=\"");
            attr_value(value, self.rules, self.out);
            self.out.push(b'"');
        }
        Ok(())
    }

    fn cdata(&mut self, text: &str) -> Result<()> {
        self.close_open_tag()?;
        if let Some(f) = self.stack.last_mut() {
            f.mixed = true;
        }
        if text.contains("]]>") {
            return Err(WriteError::CDataEnd);
        }
        self.room(text.len().saturating_add(12))?;
        self.out.extend_from_slice(b"<![CDATA[");
        self.out.extend_from_slice(text.as_bytes());
        self.out.extend_from_slice(b"]]>");
        Ok(())
    }

    fn end(&mut self, el: &Element<'_>) -> Result<()> {
        let Some(f) = self.stack.pop() else {
            return Ok(());
        };
        if f.open_tag {
            self.room(3)?;
            self.out.extend_from_slice(b" />");
            return Ok(());
        }
        if !f.mixed {
            self.indent(self.stack.len())?;
        }
        let name = element_name(&el.name);
        self.room(3 + name.len())?;
        self.out.extend_from_slice(b"</");
        self.out.extend_from_slice(name.as_bytes());
        self.out.push(b'>');
        Ok(())
    }
}

fn is_xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}

fn attr_value(text: &str, rules: &Rules, out: &mut Vec<u8>) {
    let mut buf = [0u8; 4];
    for c in text.chars() {
        match c {
            '&' => out.extend_from_slice(b"&amp;"),
            '<' => out.extend_from_slice(b"&lt;"),
            '>' => out.extend_from_slice(b"&gt;"),
            '"' => out.extend_from_slice(b"&quot;"),
            '\n' => out.extend_from_slice(b"&#xA;"),
            '\r' => out.extend_from_slice(b"&#xD;"),
            '\t' if rules.escape_tab => out.extend_from_slice(b"&#x9;"),
            c if is_xml_char(c) => out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes()),
            c => out.extend_from_slice(format!("&#x{:X};", u32::from(c)).as_bytes()),
        }
    }
}
