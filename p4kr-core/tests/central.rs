mod common;

use p4kr_core::{Archive, P4kError};

#[test]
fn synth_archive_lists_ten_entries_in_order() {
    let s = common::synth();
    let a = Archive::open(&s.archive).unwrap();
    assert_eq!(a.len(), s.expected.len());
    for case in &s.expected {
        let i = case["index"].as_u64().unwrap() as usize;
        let name = format!("Data/Synth/{}.bin", case["case"].as_str().unwrap());
        assert_eq!(a.name(i), Some(name.as_str()));
    }
}

#[test]
fn flags_follow_the_extra_blocks() {
    let a = Archive::open(&common::synth().archive).unwrap();
    let e: Vec<_> = a.entries().collect();
    assert_eq!(e[0].method, 0);
    assert!(e[1..].iter().all(|x| x.method == 100));
    assert_eq!(
        e.iter().map(|x| x.encrypted).collect::<Vec<_>>(),
        [
            false, false, true, true, true, true, false, false, false, false
        ]
    );
    assert!(e.iter().all(|x| x.has_sha && !x.bad_extra)); // entry 5 has its extra block moved
}

#[test]
fn bytes_and_file_give_the_same_table() {
    let s = common::synth();
    let a = Archive::open(&s.archive).unwrap();
    let b = Archive::from_bytes(std::fs::read(&s.archive).unwrap()).unwrap();
    assert_eq!(a.tail(), b.tail());
    let names = |x: &Archive| {
        x.entries()
            .map(|e| (e.name.to_string(), e.csize, e.size, e.crc))
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&a), names(&b));
    assert!(b.path().as_os_str().is_empty());
}

#[test]
fn short_or_truncated_files_are_format_errors() {
    for bytes in [vec![], vec![0u8; 3], vec![0u8; 21]] {
        assert!(matches!(
            Archive::from_bytes(bytes),
            Err(P4kError::Format(_))
        ));
    }
    let full = std::fs::read(common::synth().archive).unwrap();
    assert!(matches!(
        Archive::from_bytes(full[..40].to_vec()),
        Err(P4kError::Format(_))
    ));
    assert!(matches!(
        Archive::from_bytes(full[..full.len() - 1].to_vec()),
        Err(P4kError::Format(_))
    ));
}

#[test]
fn empty_plain_archive_opens_with_no_entry() {
    let mut eocd = b"PK\x05\x06".to_vec();
    eocd.extend_from_slice(&[0u8; 18]);
    let a = Archive::from_bytes(eocd).unwrap();
    assert_eq!((a.len(), a.tail().entries), (0, 0));
}

/// The synth ends with EOCD64 (56) + locator (20) + EOCD (22), without a comment; its
/// central directory ends exactly on the EOCD64.
fn synth_with_cd_size_delta(delta: u64) -> Vec<u8> {
    let mut full = std::fs::read(common::synth().archive).unwrap();
    let eocd64 = full.len() - 22 - 20 - 56;
    let u64_at = |b: &[u8], at: usize| u64::from_le_bytes(b[at..at + 8].try_into().unwrap());
    let (size, offset) = (u64_at(&full, eocd64 + 40), u64_at(&full, eocd64 + 48));
    assert_eq!(offset + size, eocd64 as u64, "the synth is an exact fit");
    full[eocd64 + 40..eocd64 + 48].copy_from_slice(&(size + delta).to_le_bytes());
    full
}

#[test]
fn central_directory_ending_exactly_on_the_eocd64_is_accepted() {
    let a = Archive::from_bytes(synth_with_cd_size_delta(0)).unwrap();
    assert_eq!(a.len(), 10);
}

#[test]
fn central_directory_one_byte_past_the_eocd64_is_refused() {
    assert!(matches!(
        Archive::from_bytes(synth_with_cd_size_delta(1)),
        Err(P4kError::Format(_))
    ));
}
