//! Accessors `text`, `blob`, `data_pos`: boundary offsets, Latin-1, end of file without NUL.

use crate::{Db, Fault};

fn golden(name: &str) -> Vec<u8> {
    let json: serde_json::Value =
        serde_json::from_str(include_str!("../tests/golden.json")).expect("golden.json");
    let hex = json
        .as_array()
        .expect("array")
        .iter()
        .find(|c| c["name"] == name)
        .expect(name)["input_hex"]
        .as_str()
        .expect("hex")
        .to_string();
    (0..hex.len() / 2)
        .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("hex"))
        .collect()
}

fn range_message(off: u32, label: &str, shown: u32) -> Fault {
    Fault::IndexOutOfRange(format!(
        "Offset {off} is out of range for {label} values (length: {shown})"
    ))
}

#[test]
fn text_offset_at_length_reads_the_following_bytes_and_past_it_is_refused() {
    let bytes = golden("records-nested");
    let db = Db::open(&bytes).unwrap();
    let (len, start) = (db.layout.text_len, db.layout.text_start);
    let from = (start + u64::from(len)) as usize;
    let end = from + bytes[from..].iter().position(|&b| b == 0).unwrap();
    let expected: String = bytes[from..end].iter().map(|&b| char::from(b)).collect();
    assert_eq!(db.text(len), Ok(expected));
    assert_eq!(db.text(len + 1), Err(range_message(len + 1, "Text", len)));
}

#[test]
fn blob_bound_is_the_blob_length_but_the_message_shows_the_text_length() {
    let bytes = golden("records-nested");
    let db = Db::open(&bytes).unwrap();
    let (text_len, blob_len) = (db.layout.text_len, db.layout.blob_len);
    assert_ne!(text_len, blob_len, "the test must tell the two apart");
    assert!(db.blob(blob_len).is_ok());
    assert_eq!(
        db.blob(blob_len + 1),
        Err(range_message(blob_len + 1, "Blob", text_len))
    );
    // Beyond the text length but inside the blob: accepted.
    if blob_len > text_len {
        assert!(db.blob(text_len + 1).is_ok());
    }
}

#[test]
fn latin1_bytes_decode_to_the_same_code_points() {
    let bytes = golden("latin1-bytes");
    let db = Db::open(&bytes).unwrap();
    let expected: String = (1u8..=255).map(char::from).collect();
    assert_eq!(db.text(0), Ok(expected));
}

#[test]
fn a_string_without_nul_runs_to_the_end_of_the_file() {
    let bytes = golden("records-nested");
    let db = Db::open(&bytes).unwrap();
    let mut patched = bytes.clone();
    let data = db.layout.data_start as usize;
    patched[data..].fill(0xAA);
    let other = Db {
        bytes: &patched,
        layout: db.layout.clone(),
        sizes: Vec::new(),
        data_starts: Vec::new(),
        field_parents: Vec::new(),
    };
    let tail = other.blob(db.layout.blob_len).unwrap();
    assert_eq!(tail.chars().count(), patched.len() - data);
    assert!(tail.chars().all(|c| c == '\u{AA}'));
}

#[test]
fn data_pos_stops_at_the_instance_count() {
    let bytes = golden("records-nested");
    let db = Db::open(&bytes).unwrap();
    let (count, _) = db.mapping(0).unwrap();
    assert_eq!(count, 5);
    let first = db.data_pos(0, 0).unwrap();
    assert_eq!(first, db.layout.data_start);
    assert!(db.data_pos(0, count - 1).is_some());
    assert_eq!(db.data_pos(0, count), None);
    assert_eq!(db.data_pos(9, 0), None);
}
