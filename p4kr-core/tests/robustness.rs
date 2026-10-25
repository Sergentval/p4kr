//! Damaged archives: never a panic or an abort, always a typed error (or a correct read).
mod common;

use p4kr_core::{Archive, Key, P4kError, Verify};
use std::panic::{AssertUnwindSafe, catch_unwind};

fn exercise(bytes: Vec<u8>, key: &Key) {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        if let Ok(a) = Archive::from_bytes(bytes) {
            for i in 0..a.len() {
                let _ = a.read(i, Some(key), Verify::Full);
            }
        }
    }));
    assert!(outcome.is_ok(), "panique");
}

fn load() -> (Vec<u8>, Key, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let s = common::synth();
    (
        std::fs::read(&s.archive).unwrap(),
        Key::from_file(&s.key).unwrap(),
        dir,
    )
}

#[test]
fn truncation_at_every_byte_of_head_and_tail() {
    let (full, key, _d) = load();
    let n = full.len();
    for cut in (0..4096.min(n)).chain(n.saturating_sub(4096)..n) {
        exercise(full[..cut].to_vec(), &key);
    }
}

#[test]
fn every_byte_of_the_central_directory_flipped() {
    let (full, key, _d) = load();
    let cd = Archive::from_bytes(full.clone()).unwrap().tail();
    for at in cd.cd_offset as usize..(cd.cd_offset + cd.cd_size) as usize {
        let mut m = full.clone();
        m[at] ^= 0xFF;
        exercise(m, &key);
    }
}

/// Offset of the ZIP64 body (size @+0, compressed size @+8) of an entry: first extra block of its
/// record. Records are walked using their lengths @28/@30/@32.
fn zip64_body(full: &[u8], entry: usize) -> usize {
    let a = Archive::from_bytes(full.to_vec()).unwrap();
    let u16_of = |at: usize| usize::from(u16::from_le_bytes([full[at], full[at + 1]]));
    let mut record = a.tail().cd_offset as usize;
    for _ in 0..entry {
        record += 46 + u16_of(record + 28) + u16_of(record + 30) + u16_of(record + 32);
    }
    record + 46 + u16_of(record + 28) + 4
}

fn set_u64(full: &mut [u8], at: usize, value: u64) {
    full[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn absurd_compressed_size_is_a_size_error_without_allocation() {
    let (mut full, key, _d) = load();
    let at = zip64_body(&full, 0) + 8;
    set_u64(&mut full, at, 1 << 62);
    let a = Archive::from_bytes(full).unwrap();
    assert_eq!(
        a.read(0, Some(&key), Verify::Full).unwrap_err().kind(),
        "size"
    );
}

#[test]
fn absurd_compressed_size_of_a_zstd_entry_is_a_size_error() {
    let (mut full, key, _d) = load();
    let at = zip64_body(&full, 1) + 8;
    set_u64(&mut full, at, u64::MAX);
    let a = Archive::from_bytes(full).unwrap();
    assert_eq!(
        a.read(1, Some(&key), Verify::Crc).unwrap_err().kind(),
        "size"
    );
}

#[test]
fn absurd_size_of_a_stored_entry_is_a_size_error() {
    let (mut full, key, _d) = load();
    let at = zip64_body(&full, 0);
    set_u64(&mut full, at, 1 << 62);
    let a = Archive::from_bytes(full).unwrap();
    assert_eq!(
        a.read(0, Some(&key), Verify::Crc).unwrap_err().kind(),
        "size"
    );
}

#[test]
fn absurd_size_of_a_zstd_entry_does_not_allocate_it() {
    let (mut full, key, _d) = load();
    let at = zip64_body(&full, 1);
    set_u64(&mut full, at, 1 << 62);
    let a = Archive::from_bytes(full).unwrap();
    assert_eq!(a.name(1), Some("Data/Synth/zstd.bin"));
    assert_eq!(
        a.read(1, Some(&key), Verify::Crc).unwrap_err().kind(),
        "size"
    );
}

#[test]
fn tiny_files_are_format_errors() {
    for n in 0..4 {
        assert!(matches!(
            Archive::from_bytes(vec![0x50; n]),
            Err(P4kError::Format(_))
        ));
    }
}

/// The synth ends with EOCD64 (56) + locator (20) + EOCD (22), without a comment.
fn eocd64_at(full: &[u8]) -> usize {
    full.len() - 22 - 20 - 56
}

#[test]
fn zip64_tail_of_the_synth_archive_is_read_through_the_locator() {
    // The synth marks its EOCD with 0xFFFF / 0xFFFFFFFF: reading therefore goes through locator + EOCD64.
    let (full, key, _d) = load();
    let eocd64 = eocd64_at(&full);
    assert_eq!(&full[eocd64..eocd64 + 4], b"PK\x06\x06");
    let a = Archive::from_bytes(full).unwrap();
    assert_eq!(a.len(), 10);
    assert!(a.read(1, Some(&key), Verify::Full).is_ok());
}

#[test]
fn central_directory_overflowing_into_the_eocd64_is_a_format_error() {
    let (mut full, _key, _d) = load();
    let size_at = eocd64_at(&full) + 40; // central directory size in the EOCD64
    set_u64(&mut full, size_at, 1 << 40);
    assert!(matches!(
        Archive::from_bytes(full),
        Err(P4kError::Format(_))
    ));
}

#[test]
fn central_directory_offset_overflow_is_a_format_error() {
    let (mut full, _key, _d) = load();
    let offset_at = eocd64_at(&full) + 48;
    set_u64(&mut full, offset_at, u64::MAX);
    assert!(matches!(
        Archive::from_bytes(full),
        Err(P4kError::Format(_))
    ));
}
