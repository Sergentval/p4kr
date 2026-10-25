//! p4kr-cryxml: binary CryXmlB XML → text XML, byte for byte like unp4k's unforge library
//! (reading `CryXmlSerializer.ReadStream`, writing `XmlDocument.Save`). Rules observed in its sources and
//! pinned by `tests/golden.json` (reference outputs on synthetic cases); no code reused.

mod dotnet;
mod table;
mod tree;

pub use table::Kind;

use p4kr_xmlwrite::{Profile, WriteError};

/// Conversion failures; messages contain neither an entry name nor any content.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CryXmlError {
    #[error("unexpected end of file")]
    Eof,
    #[error("negative offset")]
    NegativeOffset,
    #[error("character outside the basic multilingual plane")]
    Supplementary,
    #[error("UTF-8 decoding longer than the character buffer")]
    CharOverflow,
    #[error("string not found")]
    MissingString,
    #[error("attribute index out of table")]
    AttributeIndex,
    #[error("invalid XML name")]
    BadName,
    #[error("second root element")]
    SecondRoot,
    #[error("node is its own parent")]
    SelfParent,
    #[error("CDATA section containing `]]>`")]
    CDataEnd,
    #[error("document without a root element")]
    NoRoot,
    /// Deliberate deviation from the reference: output is capped (see `p4kr_xmlwrite::MAX_OUTPUT`).
    #[error("XML output beyond the cap")]
    TooLarge,
}

impl From<WriteError> for CryXmlError {
    fn from(e: WriteError) -> Self {
        match e {
            WriteError::NoRoot => Self::NoRoot,
            WriteError::CDataEnd => Self::CDataEnd,
            WriteError::TooLarge => Self::TooLarge,
            // Only the `DataForge` profile rejects names; this converter writes in `DocumentSave`.
            WriteError::Name(_) => Self::BadName,
        }
    }
}

pub type Result<T> = std::result::Result<T, CryXmlError>;

/// Outcome of a conversion, in the sense of unforge.cli.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// XML fully written.
    Converted,
    /// Already XML (first character `<`): nothing is written.
    Text,
    /// Not a CryXmlB: nothing is written, no error.
    Other,
    /// Failure; `xml` carries the partial bytes if writing had started.
    Failed,
}

#[derive(Debug)]
pub struct Conversion {
    pub outcome: Outcome,
    /// Bytes the reference leaves in the `.xml` file; `None`: file never created.
    pub xml: Option<Vec<u8>>,
    pub error: Option<CryXmlError>,
}

impl Conversion {
    fn nothing(outcome: Outcome, error: Option<CryXmlError>) -> Self {
        Self {
            outcome,
            xml: None,
            error,
        }
    }
}

pub fn convert(input: &[u8]) -> Conversion {
    let tables = match table::kind(input) {
        Ok((Kind::Text, _)) => return Conversion::nothing(Outcome::Text, None),
        Ok((Kind::Other, _)) => return Conversion::nothing(Outcome::Other, None),
        Ok((Kind::Binary(_), reader)) => table::read(reader),
        Err(e) => Err(e),
    };
    let tables = match tables {
        Ok(tables) => tables,
        Err(e) => return Conversion::nothing(Outcome::Failed, Some(e)),
    };
    let doc = match tree::build(&tables) {
        Ok(doc) if doc.root.is_some() => doc,
        Ok(_) => return Conversion::nothing(Outcome::Failed, Some(CryXmlError::NoRoot)),
        Err(e) => return Conversion::nothing(Outcome::Failed, Some(e)),
    };
    let mut out = Vec::with_capacity(input.len().min(1 << 24));
    match p4kr_xmlwrite::write(&doc, Profile::DocumentSave, &mut out) {
        Ok(()) => Conversion {
            outcome: Outcome::Converted,
            xml: Some(out),
            error: None,
        },
        Err(WriteError::TooLarge) => {
            Conversion::nothing(Outcome::Failed, Some(CryXmlError::TooLarge))
        }
        Err(e) => Conversion {
            outcome: Outcome::Failed,
            xml: Some(out),
            error: Some(e.into()),
        },
    }
}

/// Rule of `Smelter.Smelt`: extension exactly `.dcb` (case-sensitive) → DataForge, never CryXml.
pub fn is_dataforge(name: &str) -> bool {
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    file.rfind('.').is_some_and(|i| &file[i..] == ".dcb")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dataforge_is_the_exact_lowercase_extension() {
        assert!(is_dataforge("Data\\Base.dcb"));
        assert!(is_dataforge("Data/x/.dcb"));
        assert!(!is_dataforge("Data/Base.DCB"));
        assert!(!is_dataforge("Data/a.dcb/b"));
        assert!(!is_dataforge("Data/a.dcbx"));
    }
}
