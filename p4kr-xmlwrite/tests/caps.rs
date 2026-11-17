//! Output caps per profile: DataForge records go beyond the CryXmlB cap (5.0 records exceed 256 MiB),
//! CryXmlB keeps its cap.

use p4kr_xmlwrite::{
    Child, Doc, Element, MAX_DATAFORGE_OUTPUT, MAX_OUTPUT, Profile, WriteError, write,
};
use std::borrow::Cow;

/// `<root><big value="aaa…"/></root>`, the value `len` bytes long.
fn doc_with_value(len: usize) -> Doc<'static> {
    let value = Cow::Owned("a".repeat(len));
    Doc {
        elements: vec![
            Element {
                name: Cow::Borrowed("root"),
                attrs: Vec::new(),
                children: vec![Child::Element(1)],
            },
            Element {
                name: Cow::Borrowed("big"),
                attrs: vec![(Cow::Borrowed("value"), value)],
                children: Vec::new(),
            },
        ],
        root: Some(0),
    }
}

#[test]
fn each_profile_has_its_cap() {
    assert_eq!(Profile::DocumentSave.max_output(), MAX_OUTPUT);
    assert_eq!(Profile::DataForge.max_output(), MAX_DATAFORGE_OUTPUT);
}

#[test]
fn a_dataforge_output_beyond_256_mib_is_written() {
    let len = 300 << 20;
    let doc = doc_with_value(len);
    let mut out = Vec::new();
    write(&doc, Profile::DataForge, &mut out).expect("written");
    assert!(out.len() > len);
    assert!(out.len() > MAX_OUTPUT);
}

#[test]
fn a_cryxml_output_beyond_its_cap_is_refused() {
    let doc = doc_with_value(300 << 20);
    let mut out = Vec::new();
    assert_eq!(
        write(&doc, Profile::DocumentSave, &mut out),
        Err(WriteError::TooLarge)
    );
}
