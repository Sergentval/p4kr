mod common;

use p4kr_core::{Archive, Key, P4kError, Verify};
use sha2::{Digest, Sha256};

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
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

/// Offset of the central directory record of entry `entry`.
fn record_at(full: &[u8], entry: usize) -> usize {
    let a = Archive::from_bytes(full.to_vec()).unwrap();
    let u16_of = |at: usize| usize::from(u16::from_le_bytes([full[at], full[at + 1]]));
    let mut pos = a.tail().cd_offset as usize;
    for _ in 0..entry {
        pos += 46 + u16_of(pos + 28) + u16_of(pos + 30) + u16_of(pos + 32);
    }
    pos
}

/// Offset of the body of the ZIP64 block (size @+0, compressed size @+8): first extra block of the entry.
fn zip64_body(full: &[u8], entry: usize) -> usize {
    let record = record_at(full, entry);
    let nlen = usize::from(u16::from_le_bytes([full[record + 28], full[record + 29]]));
    record + 46 + nlen + 4
}

fn local_at(full: &[u8], entry: usize) -> usize {
    Archive::from_bytes(full.to_vec())
        .unwrap()
        .entry(entry)
        .unwrap()
        .local_offset as usize
}

fn kind(full: Vec<u8>, key: &Key, entry: usize) -> &'static str {
    Archive::from_bytes(full)
        .unwrap()
        .read(entry, Some(key), Verify::Crc)
        .unwrap_err()
        .kind()
}

#[test]
fn every_synth_case_gives_its_expected_result_in_full_mode() {
    let s = common::synth();
    let a = Archive::open(&s.archive).unwrap();
    let key = Key::from_file(&s.key).unwrap();
    for case in &s.expected {
        let i = case["index"].as_u64().unwrap() as usize;
        let got = a.read(i, Some(&key), Verify::Full);
        match case["expect"].as_str().unwrap() {
            "ok" => assert_eq!(
                hex(&Sha256::digest(got.unwrap())),
                case["sha256"].as_str().unwrap(),
                "{case}"
            ),
            _ => assert_eq!(
                got.unwrap_err().kind(),
                case["error"].as_str().unwrap(),
                "{case}"
            ),
        }
    }
}

#[test]
fn crc_mode_skips_only_the_sha_check() {
    let s = common::synth();
    let a = Archive::open(&s.archive).unwrap();
    let key = Key::from_file(&s.key).unwrap();
    assert!(a.read(7, Some(&key), Verify::Crc).is_ok()); // sha-faux: content intact
    assert_eq!(
        a.read(6, Some(&key), Verify::Crc).unwrap_err().kind(),
        "crc"
    );
}

#[test]
fn encrypted_entry_without_key_is_a_decrypt_error() {
    let a = Archive::open(&common::synth().archive).unwrap();
    assert_eq!(a.read(2, None, Verify::Crc).unwrap_err().kind(), "decrypt");
}

#[test]
fn positional_read_matches_mmap_read() {
    let s = common::synth();
    let a = Archive::open(&s.archive).unwrap();
    let key = Key::from_file(&s.key).unwrap();
    let file = std::fs::File::open(&s.archive).unwrap();
    for i in 0..a.len() {
        let m = a.read(i, Some(&key), Verify::Full).map_err(|e| e.kind());
        let p = a
            .read_at(&file, i, Some(&key), Verify::Full)
            .map_err(|e| e.kind());
        assert_eq!(m, p, "entry {i}");
    }
}

#[test]
fn index_out_of_range_is_a_format_error() {
    let a = Archive::open(&common::synth().archive).unwrap();
    assert!(matches!(
        a.read(a.len(), None, Verify::Crc),
        Err(P4kError::Format(_))
    ));
}

// --- Each error variant of the read path ---

#[test]
fn local_header_signature_mismatch() {
    let (mut full, key, _d) = load();
    let at = local_at(&full, 0);
    full[at] ^= 0xFF;
    assert_eq!(kind(full, &key, 0), "local-header");
}

#[test]
fn local_header_name_mismatch() {
    let (mut full, key, _d) = load();
    let at = local_at(&full, 1) + 30;
    full[at] ^= 1;
    assert_eq!(kind(full, &key, 1), "local-header");
}

#[test]
fn local_header_method_mismatch() {
    let (mut full, key, _d) = load();
    let at = local_at(&full, 1) + 8;
    full[at..at + 2].copy_from_slice(&0u16.to_le_bytes());
    assert_eq!(kind(full, &key, 1), "local-header");
}

#[test]
fn unknown_method_in_both_headers_is_unsupported() {
    let (mut full, key, _d) = load();
    let (cd, local) = (record_at(&full, 0) + 10, local_at(&full, 0) + 8);
    for at in [cd, local] {
        full[at..at + 2].copy_from_slice(&99u16.to_le_bytes());
    }
    let a = Archive::from_bytes(full).unwrap();
    let e = a.read(0, Some(&key), Verify::Full).unwrap_err();
    assert!(matches!(e, P4kError::UnsupportedMethod(99)), "{e:?}");
    assert_eq!(e.kind(), "method");
}

#[test]
fn unknown_extra_block_blocks_the_read() {
    let (mut full, key, _d) = load();
    let at = zip64_body(&full, 0) - 4; // ZIP64 block id → unknown block of the same length
    full[at..at + 2].copy_from_slice(&0x5004u16.to_le_bytes());
    let a = Archive::from_bytes(full).unwrap();
    assert!(a.entry(0).unwrap().bad_extra);
    assert_eq!(
        a.read(0, Some(&key), Verify::Crc).unwrap_err().kind(),
        "unknown-extra"
    );
}

/// Archive of one entry, without ZIP64: local header, central record, EOCD. Also returns
/// the offset of the central directory.
fn one_entry(method: u16, stored: &[u8], size: u32, crc: u32) -> (Vec<u8>, usize) {
    let name = b"x.bin";
    let csize = u32::try_from(stored.len()).unwrap();
    let mut out = Vec::new();
    let put = |out: &mut Vec<u8>, parts: &[&[u8]]| parts.iter().for_each(|p| out.extend(*p));
    let (m, nlen) = (method.to_le_bytes(), (name.len() as u16).to_le_bytes());
    let (crc, csize_b, size_b) = (crc.to_le_bytes(), csize.to_le_bytes(), size.to_le_bytes());
    put(
        &mut out,
        &[b"PK\x03\x04", &[45, 0, 0, 0], &m, &[0; 4], &crc, &csize_b],
    );
    put(&mut out, &[&size_b, &nlen, &[0, 0], name, stored]);
    let cd = out.len();
    put(
        &mut out,
        &[
            b"PK\x01\x02",
            &[46, 0, 45, 0, 0, 0],
            &m,
            &[0; 4],
            &crc,
            &csize_b,
        ],
    );
    put(
        &mut out,
        &[&size_b, &nlen, &[0; 12], &0u32.to_le_bytes(), name],
    );
    let cd_size = (out.len() - cd) as u32;
    put(
        &mut out,
        &[
            b"PK\x05\x06",
            &[0; 4],
            &[1, 0, 1, 0],
            &cd_size.to_le_bytes(),
        ],
    );
    put(&mut out, &[&(cd as u32).to_le_bytes(), &[0, 0]]);
    (out, cd)
}

fn crc(data: &[u8]) -> u32 {
    crc32c::crc32c(data)
}

#[test]
fn hand_built_stored_archive_reads_back() {
    let data = b"content".to_vec();
    let (full, _) = one_entry(0, &data, data.len() as u32, crc(&data));
    let a = Archive::from_bytes(full).unwrap();
    assert_eq!(a.read(0, None, Verify::Full).unwrap(), data);
}

#[test]
fn wide_size_without_zip64_block_opens_but_the_entry_is_unreadable() {
    let data = b"content".to_vec();
    let (mut full, cd) = one_entry(0, &data, data.len() as u32, crc(&data));
    full[cd + 24..cd + 28].copy_from_slice(&u32::MAX.to_le_bytes());
    let a = Archive::from_bytes(full).unwrap();
    assert!(a.entry(0).unwrap().bad_extra);
    assert_eq!(
        a.read(0, None, Verify::Full).unwrap_err().kind(),
        "unknown-extra"
    );
}

/// Compressible content over several zstd blocks (1 MiB).
fn big() -> Vec<u8> {
    (0..1u32 << 18)
        .flat_map(|n| (n.wrapping_mul(2_654_435_761) >> 11).to_le_bytes())
        .collect()
}

fn zstd_archive(frame: &[u8], size: u32, crc: u32) -> Vec<u8> {
    one_entry(100, frame, size, crc).0
}

fn read_kind(full: Vec<u8>) -> Result<Vec<u8>, &'static str> {
    Archive::from_bytes(full)
        .unwrap()
        .read(0, None, Verify::Crc)
        .map_err(|e| e.kind())
}

#[test]
fn multi_block_zstd_frame_reads_back() {
    let data = big();
    let frame = zstd::bulk::compress(&data, 3).unwrap();
    let full = zstd_archive(&frame, data.len() as u32, crc(&data));
    assert_eq!(read_kind(full).unwrap(), data);
}

#[test]
fn frame_longer_than_declared_followed_by_junk_is_a_zstd_error() {
    let data = big();
    let mut frame = zstd::bulk::compress(&data, 3).unwrap();
    frame.extend_from_slice(b"junk");
    let full = zstd_archive(&frame, 100, crc(&data));
    assert_eq!(read_kind(full).unwrap_err(), "zstd");
}

#[test]
fn clean_frame_longer_than_declared_is_a_size_error() {
    let data = big();
    let frame = zstd::bulk::compress(&data, 3).unwrap();
    let full = zstd_archive(&frame, 100, crc(&data));
    assert_eq!(read_kind(full).unwrap_err(), "size");
}

#[test]
fn clean_frame_shorter_than_declared_is_a_size_error() {
    let data = b"short".to_vec();
    let frame = zstd::bulk::compress(&data, 3).unwrap();
    let full = zstd_archive(&frame, 1000, crc(&data));
    assert_eq!(read_kind(full).unwrap_err(), "size");
}

#[test]
fn truncated_frame_whose_output_already_exceeds_the_size_is_a_zstd_error() {
    let data = big();
    let frame = zstd::bulk::compress(&data, 3).unwrap();
    let cut = &frame[..frame.len() - 10];
    let full = zstd_archive(cut, 100, crc(&data));
    assert_eq!(read_kind(full).unwrap_err(), "zstd");
}

#[test]
fn truncated_frame_within_the_size_is_a_zstd_error() {
    let data = big();
    let frame = zstd::bulk::compress(&data, 3).unwrap();
    let full = zstd_archive(&frame[..frame.len() - 10], data.len() as u32, crc(&data));
    assert_eq!(read_kind(full).unwrap_err(), "zstd");
}

#[test]
fn empty_stored_bytes_for_a_zstd_entry_are_a_zstd_error() {
    let full = zstd_archive(&[], 5, 0);
    assert_eq!(read_kind(full).unwrap_err(), "zstd");
}

#[test]
fn io_errors_are_typed() {
    let dir = tempfile::tempdir().unwrap();
    let missing = Archive::open(&dir.path().join("absent.p4k"));
    assert!(matches!(missing, Err(P4kError::Io(_))));
    assert_eq!(missing.err().unwrap().kind(), "exception:Io");
}

#[test]
fn positional_read_from_a_truncated_file_is_an_io_error() {
    let dir = tempfile::tempdir().unwrap();
    let s = common::synth();
    let a = Archive::open(&s.archive).unwrap();
    let cut = dir.path().join("cut.p4k");
    std::fs::write(&cut, b"").unwrap();
    let file = std::fs::File::open(&cut).unwrap();
    let e = a.read_at(&file, 0, None, Verify::Crc).unwrap_err();
    assert!(matches!(e, P4kError::Io(_)), "{e:?}");
}

// --- Key file ---

#[test]
fn key_file_accepts_crlf_spaces_and_comments() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("k.toml");
    std::fs::write(
        &path,
        "# test key\r\naes_key = \"000102030405060708090a0b0c0d0e0f\"  # end\r\n",
    )
    .unwrap();
    let k = Key::from_file(&path).unwrap();
    assert_eq!(format!("{k:?}"), "Key(…)");
}

#[test]
fn key_file_accepts_literal_strings_and_matches_the_synth_key() {
    let dir = tempfile::tempdir().unwrap();
    let s = common::synth();
    let text = std::fs::read_to_string(&s.key).unwrap();
    let literal = text.replace('"', "'");
    let path = dir.path().join("literal.toml");
    std::fs::write(&path, literal).unwrap();
    let a = Archive::open(&s.archive).unwrap();
    let key = Key::from_file(&path).unwrap();
    assert!(a.read(2, Some(&key), Verify::Full).is_ok());
    let bytes: [u8; 16] = std::array::from_fn(|i| i as u8);
    assert!(
        a.read(2, Some(&Key::from_bytes(bytes)), Verify::Full)
            .is_ok()
    );
}

#[test]
fn key_file_errors_never_show_the_value() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("k.toml");
    let bad = [
        "aes_key = \"0011\"\n",
        "aes_key = \"zz0102030405060708090a0b0c0d0e0f\"\n",
        "aes_key = \"+f0102030405060708090a0b0c0d0e0f\"\n",
        "aes_key = \"\u{e9}0102030405060708090a0b0c0d0e0f\"\n",
        "aes_key = 000102030405060708090a0b0c0d0e0f\n",
        "aes_key = \"000102030405060708090a0b0c0d0e0f\n",
        "other = 1\n",
    ];
    for text in bad {
        std::fs::write(&path, text).unwrap();
        let e = Key::from_file(&path).unwrap_err();
        assert!(matches!(e, P4kError::Key(_)), "{text}");
        let msg = e.to_string();
        for secret in ["0011", "zz01", "+f01", "0102030405"] {
            assert!(!msg.contains(secret), "{msg}");
        }
    }
    assert!(matches!(
        Key::from_file(&dir.path().join("absent.toml")),
        Err(P4kError::Key(_))
    ));
}

/// 4.10 archive: local headers as `PK\x03\x04` or as the `PK\x03\x14` variant; any other signature is refused.
#[test]
fn the_local_header_variant_reads_like_the_standard_one_in_both_read_paths() {
    let path = common::testdata("local-variant.p4k");
    let a = Archive::open(&path).unwrap();
    let key = Key::from_file(&common::synth().key).unwrap();
    let file = std::fs::File::open(&path).unwrap();
    assert_eq!(a.len(), 7);
    let last = a.len() - 1;
    for i in 0..last {
        let m = a.read(i, Some(&key), Verify::Full).unwrap();
        let p = a.read_at(&file, i, Some(&key), Verify::Full).unwrap();
        assert_eq!(m, p, "entry {i}");
        if !a.name(i).unwrap().ends_with(".xml") {
            assert!(m.starts_with(b"p4kr local header variant\n"), "entry {i}");
        }
    }
    assert_eq!(
        kind(std::fs::read(&path).unwrap(), &key, last),
        "local-header"
    );
    let e = a.read_at(&file, last, Some(&key), Verify::Crc).unwrap_err();
    assert_eq!(e.kind(), "local-header");
}
