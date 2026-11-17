//! p4kr-xmlwrite: shared XML tree and indented writer (CryXmlB, DataForge). Texts are `Cow`: converters
//! that read a string table borrow, the others own.

pub mod names;
mod write;

use names::NameFault;

use std::borrow::Cow;

pub use write::{MAX_OUTPUT, write};

#[derive(Debug)]
pub enum Child<'a> {
    CData(Cow<'a, str>),
    Element(usize),
}

#[derive(Debug)]
pub struct Element<'a> {
    pub name: Cow<'a, str>,
    pub attrs: Vec<(Cow<'a, str>, Cow<'a, str>)>,
    pub children: Vec<Child<'a>>,
}

/// Index arena: `root` and `Child::Element` designate positions in `elements`.
#[derive(Debug)]
pub struct Doc<'a> {
    pub elements: Vec<Element<'a>>,
    pub root: Option<usize>,
}

/// Writing variant: `XmlDocument.Save` (CryXmlB) or `XmlWriter` configured like `DataForge.Save`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    DocumentSave,
    DataForge,
}

#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum WriteError {
    #[error("document without a root element")]
    NoRoot,
    #[error("CDATA section containing `]]>`")]
    CDataEnd,
    /// Deliberate deviation from the reference: output is capped (see `MAX_OUTPUT`).
    #[error("XML output beyond the cap")]
    TooLarge,
    /// The `DataForge` profile follows `XmlWriter`, which rejects some names; `NameFault` keeps the stage and
    /// the message of the reference.
    #[error("{0}")]
    Name(NameFault),
}
