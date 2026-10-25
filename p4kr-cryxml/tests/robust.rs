//! Robustness: no input panics, loops forever or allocates without bound.

use p4kr_cryxml::{CryXmlError, Outcome, convert};
use std::time::{Duration, Instant};

fn inputs() -> Vec<Vec<u8>> {
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("golden.json")).expect("golden.json");
    golden
        .as_array()
        .expect("list")
        .iter()
        .map(|c| {
            let s = c["input_hex"].as_str().expect("hex");
            (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
                .collect()
        })
        .collect()
}

/// Chain of `depth` nested elements (n0 > n1 > …), with no attribute or content.
fn chain(depth: usize) -> Vec<u8> {
    let mut strings = Vec::new();
    let mut offsets = Vec::with_capacity(depth);
    for i in 0..depth {
        offsets.push(strings.len() as i32);
        strings.extend_from_slice(format!("n{i}").as_bytes());
        strings.push(0);
    }
    let empty = strings.len() as i32;
    strings.push(0);
    let node_off = 44usize;
    let string_off = node_off + 28 * depth;
    let len = string_off + strings.len();
    let mut out = b"CryXmlB\0".to_vec();
    for v in [
        len,
        node_off,
        depth,
        string_off,
        0,
        string_off,
        0,
        string_off,
        strings.len(),
    ] {
        out.extend((v as i32).to_le_bytes());
    }
    for (i, &name) in offsets.iter().enumerate() {
        out.extend(name.to_le_bytes());
        out.extend(empty.to_le_bytes());
        out.extend([0u8; 4]);
        for v in [i as i32 - 1, 0, 0, 0] {
            out.extend(v.to_le_bytes());
        }
    }
    out.extend(strings);
    out
}

#[test]
fn every_truncation_of_every_case_terminates() {
    for input in inputs() {
        for n in 0..=input.len() {
            let _ = convert(&input[..n]);
        }
    }
}

#[test]
fn random_byte_flips_never_panic() {
    let mut seed = 0x2026_1024_u64;
    for input in inputs().into_iter().filter(|i| !i.is_empty()) {
        for _ in 0..200 {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let mut m = input.clone();
            let at = (seed >> 33) as usize % m.len();
            m[at] ^= ((seed >> 17) as u8) | 1;
            let _ = convert(&m);
        }
    }
}

#[test]
fn huge_declared_counts_fail_fast() {
    let mut b = chain(3);
    b[8 + 8..8 + 12].copy_from_slice(&0x7FFF_FFF0_i32.to_le_bytes());
    let t = Instant::now();
    assert_eq!(convert(&b).outcome, Outcome::Failed);
    assert!(t.elapsed() < Duration::from_secs(1));
}

#[test]
fn deep_documents_are_written_without_recursion() {
    let c = convert(&chain(5_000));
    assert_eq!(c.outcome, Outcome::Converted, "{:?}", c.error);
    let xml = c.xml.expect("xml");
    assert!(xml.starts_with(b"<n0>\n  <n1>"));
    assert!(xml.ends_with(b"</n0>"));
}

/// A root and `kids` children, all with content `content` (a single string in the table).
fn shared_content(kids: usize, content: &[u8]) -> Vec<u8> {
    let strings = [b"n\0".as_slice(), content, b"\0"].concat();
    let count = kids + 1;
    let node_off = 44usize;
    let string_off = node_off + 28 * count;
    let mut out = b"CryXmlB\0".to_vec();
    let header = [
        string_off + strings.len(),
        node_off,
        count,
        string_off,
        0,
        string_off,
        0,
    ];
    for v in header.into_iter().chain([string_off, strings.len()]) {
        out.extend((v as i32).to_le_bytes());
    }
    for i in 0..count {
        let parent = if i == 0 { -1 } else { 0 };
        // Name, content, two zero i16s (attributes, children), parent, then three ignored integers.
        for v in [0i32, 2, 0, parent, 0, 0, 0] {
            out.extend(v.to_le_bytes());
        }
    }
    out.extend(strings);
    out
}

#[test]
fn absurdly_deep_documents_hit_the_output_cap_quickly() {
    let t = Instant::now();
    let c = convert(&chain(100_000));
    assert_eq!(
        (c.outcome, c.error),
        (Outcome::Failed, Some(CryXmlError::TooLarge))
    );
    assert!(c.xml.is_none());
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
}

#[test]
fn a_huge_shared_content_stays_bounded() {
    let t = Instant::now();
    let c = convert(&shared_content(6_000, &vec![b'x'; 1 << 20]));
    match c.outcome {
        Outcome::Converted => assert!(c.xml.expect("xml").len() <= 256 << 20),
        _ => assert_eq!(
            (c.outcome, c.error, c.xml),
            (Outcome::Failed, Some(CryXmlError::TooLarge), None)
        ),
    }
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
}

#[test]
fn shared_content_is_written_for_every_node() {
    let c = convert(&shared_content(2, b"T"));
    assert_eq!(c.outcome, Outcome::Converted, "{:?}", c.error);
    assert_eq!(
        c.xml.as_deref(),
        Some(b"<n><![CDATA[T]]><n><![CDATA[T]]></n><n><![CDATA[T]]></n></n>".as_slice())
    );
}

/// Raw table: node rows (name, content, attribute count, parent), attribute rows (key, value).
fn raw_table(nodes: &[(i32, i32, i16, i32)], attrs: &[(i32, i32)], strings: &[u8]) -> Vec<u8> {
    let node_off = 44usize;
    let attr_off = node_off + 28 * nodes.len();
    let string_off = attr_off + 8 * attrs.len();
    let mut out = b"CryXmlB\0".to_vec();
    let header = [
        string_off + strings.len(),
        node_off,
        nodes.len(),
        attr_off,
        attrs.len(),
        string_off,
        0,
        string_off,
        strings.len(),
    ];
    for v in header {
        out.extend((v as i32).to_le_bytes());
    }
    for &(name, content, attr_count, parent) in nodes {
        out.extend(name.to_le_bytes());
        out.extend(content.to_le_bytes());
        out.extend(attr_count.to_le_bytes());
        out.extend([0u8; 2]);
        for v in [parent, 0, 0, 0] {
            out.extend(v.to_le_bytes());
        }
    }
    for &(key, value) in attrs {
        out.extend(key.to_le_bytes());
        out.extend(value.to_le_bytes());
    }
    out.extend_from_slice(strings);
    out
}

/// Strings "v\0" (offset 0), then a valid 1 MiB name (offset 2).
fn big_name_strings() -> Vec<u8> {
    [b"v\0".as_slice(), &vec![b'a'; 1 << 20], b"\0"].concat()
}

fn assert_quick_and_sound(input: &[u8]) {
    let t = Instant::now();
    let c = convert(input);
    assert!(matches!(c.outcome, Outcome::Converted | Outcome::Failed));
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
}

#[test]
fn a_huge_shared_blank_content_is_scanned_once() {
    assert_quick_and_sound(&shared_content(50_000, &vec![b' '; 1 << 20]));
}

#[test]
fn a_huge_shared_valid_name_is_checked_once() {
    let nodes: Vec<_> = (0..50_000)
        .map(|i| (2, 0, 0, if i == 0 { -1 } else { 0 }))
        .collect();
    assert_quick_and_sound(&raw_table(&nodes, &[], &big_name_strings()));
}

#[test]
fn a_huge_shared_valid_attribute_key_is_checked_once() {
    // 20 nodes of 2,500 attributes each, all with the same key.
    let nodes: Vec<_> = (0..20)
        .map(|i| (0, 0, 2_500, if i == 0 { -1 } else { 0 }))
        .collect();
    let attrs = vec![(2, 0); 20 * 2_500];
    assert_quick_and_sound(&raw_table(&nodes, &attrs, &big_name_strings()));
}
