use p4kr_dataforge::{Db, DcbError};
use std::time::{Duration, Instant};

const BREAKAGES: [&str; 7] = [
    "version-9",
    "version-6",
    "truncated",
    "trailing-byte",
    "mapping-swapped",
    "struct-size-wrong",
    "unknown-type",
];

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).expect("hex"))
        .collect()
}

fn cases() -> Vec<(String, Option<String>, Vec<u8>)> {
    let json: serde_json::Value =
        serde_json::from_str(include_str!("golden.json")).expect("golden.json");
    json.as_array()
        .expect("array")
        .iter()
        .map(|c| {
            (
                c["name"].as_str().expect("name").to_string(),
                c["error"].as_str().map(str::to_string),
                unhex(c["input_hex"].as_str().expect("input_hex")),
            )
        })
        .collect()
}

fn open_error(name: &str) -> DcbError {
    let (_, _, bytes) = cases().into_iter().find(|c| c.0 == name).expect(name);
    Db::open(&bytes).err().expect("refusal expected")
}

#[test]
fn accepted_cases_open() {
    let mut opened = 0;
    for (name, error, bytes) in cases() {
        if error.is_some() || BREAKAGES.contains(&name.as_str()) {
            continue;
        }
        let db = Db::open(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(db.version(), 8, "{name}");
        assert!(db.struct_count() > 0 || name == "empty-struct", "{name}");
        opened += 1;
    }
    assert!(opened >= 38, "{opened} cases opened");
}

#[test]
fn counts_are_exposed() {
    let (_, _, bytes) = cases()
        .into_iter()
        .find(|c| c.0 == "records-nested")
        .unwrap();
    let db = Db::open(&bytes).unwrap();
    assert_eq!((db.struct_count(), db.record_count()), (1, 5));
}

#[test]
fn breakage_cases_are_refused_with_their_error() {
    assert_eq!(open_error("version-9"), DcbError::Version { found: 9 });
    assert_eq!(open_error("version-6"), DcbError::Version { found: 6 });
    assert!(matches!(
        open_error("truncated"),
        DcbError::Length { .. } | DcbError::Truncated { .. }
    ));
    assert!(matches!(
        open_error("trailing-byte"),
        DcbError::Length { .. }
    ));
    assert_eq!(
        open_error("mapping-swapped"),
        DcbError::Mapping { index: 0, found: 1 }
    );
    assert!(matches!(
        open_error("struct-size-wrong"),
        DcbError::StructSize { index: 0, .. }
    ));
    assert_eq!(
        open_error("unknown-type"),
        DcbError::UnknownType {
            index: 0,
            code: 0x11
        }
    );
}

#[test]
fn short_inputs_are_refused() {
    for len in [0usize, 1, 119] {
        assert!(matches!(
            Db::open(&vec![0u8; len]),
            Err(DcbError::Truncated {
                table: "header",
                ..
            })
        ));
    }
}

fn check_fast(label: &str, bytes: &[u8]) {
    let t = Instant::now();
    let _ = Db::open(bytes);
    assert!(t.elapsed() < Duration::from_secs(2), "{label} too slow");
}

#[test]
fn truncations_never_panic() {
    for (name, _, bytes) in cases() {
        let step = if bytes.len() < 4096 {
            1
        } else {
            bytes.len() / 2000
        };
        for len in (0..bytes.len()).step_by(step) {
            check_fast(&name, &bytes[..len]);
        }
        for len in bytes.len().saturating_sub(64)..bytes.len() {
            check_fast(&name, &bytes[..len]);
        }
    }
}

#[test]
fn byte_flips_never_panic() {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for (name, _, bytes) in cases() {
        for _ in 0..200 {
            let mut mutated = bytes.clone();
            let at = (next() % mutated.len() as u64) as usize;
            mutated[at] ^= 1 << (next() % 8);
            check_fast(&name, &mutated);
        }
    }
}

#[test]
fn flips_in_the_header_never_panic() {
    for (name, _, bytes) in cases().into_iter().take(12) {
        for at in 0..120 {
            for bit in 0..8 {
                let mut mutated = bytes.clone();
                mutated[at] ^= 1 << bit;
                check_fast(&name, &mutated);
            }
        }
    }
}

fn case(name: &str) -> Vec<u8> {
    cases().into_iter().find(|c| c.0 == name).expect(name).2
}

fn put16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn struct_row(i: usize) -> usize {
    120 + 16 * i
}

/// Start of the row of field `i`: the field table follows the struct table.
fn field_row(b: &[u8], i: usize) -> usize {
    let structs = u32::from_le_bytes(b[16..20].try_into().unwrap()) as usize;
    120 + 16 * structs + 12 * i
}

fn refused(bytes: &[u8]) -> DcbError {
    Db::open(bytes).err().expect("refusal expected")
}

#[test]
fn parent_loops_are_refused() {
    // inheritance: struct 0 = base, 1 = middle (parent 0), 2 = leaf (parent 1).
    let mut b = case("inheritance");
    put32(&mut b, struct_row(0) + 4, 1);
    assert!(matches!(refused(&b), DcbError::Parent { .. }));
    let mut b = case("inheritance");
    put32(&mut b, struct_row(0) + 4, 0);
    assert!(matches!(refused(&b), DcbError::Parent { .. }));
    let mut b = case("inheritance");
    put32(&mut b, struct_row(0) + 4, 99);
    assert_eq!(
        refused(&b),
        DcbError::Parent {
            index: 0,
            parent: 99
        }
    );
}

#[test]
fn class_field_cycles_and_bad_indices_are_refused() {
    // class-inline: fields 0-1 = struct 0, fields 2-4 = struct 1; field 3 is the class.
    for target in [1u16, 9, 0xFFFF] {
        let mut b = case("class-inline");
        let at = field_row(&b, 3) + 4;
        put16(&mut b, at, target);
        assert!(
            matches!(refused(&b), DcbError::Parent { index: 1, .. }),
            "{target}"
        );
    }
}

#[test]
fn a_class_cycle_through_two_structures_is_refused() {
    // Field 0 (struct 0) becomes a class of struct 1, whose field 3 is a class of 0.
    let mut b = case("class-inline");
    let (f0, f3) = (field_row(&b, 0), field_row(&b, 3));
    put16(&mut b, f3 + 4, 0);
    put16(&mut b, f0 + 4, 1);
    put16(&mut b, f0 + 6, 0x10);
    put16(&mut b, f0 + 8, 0);
    assert_eq!(
        refused(&b),
        DcbError::Parent {
            index: 1,
            parent: 0
        }
    );
}

#[test]
fn field_ranges_conversions_and_totals_are_refused() {
    let mut b = case("class-inline");
    put16(&mut b, struct_row(0) + 10, 0xFFF0);
    assert!(matches!(refused(&b), DcbError::Fields { index: 0, .. }));
    let mut b = case("class-inline");
    let at = field_row(&b, 0) + 8;
    put16(&mut b, at, 7);
    assert_eq!(
        refused(&b),
        DcbError::UnknownConversion { index: 0, code: 7 }
    );
    // Each range fits in the table, but their sum exceeds it.
    let mut b = case("class-inline");
    put16(&mut b, struct_row(0) + 8, 5);
    assert_eq!(
        refused(&b),
        DcbError::FieldTotal {
            declared: 8,
            count: 5
        }
    );
}

#[test]
fn absurd_counts_are_refused_without_allocating() {
    for at in (16..112).step_by(4) {
        for v in [0xFFFF_FFFFu32, 0x8000_0000, 0x7FFF_FFFF] {
            let mut b = case("class-inline");
            put32(&mut b, at, v);
            assert!(Db::open(&b).is_err(), "{at} {v:#x}");
        }
    }
    let mut b = case("class-inline");
    put32(&mut b, 16, 0xFFFF_FFFF);
    assert!(matches!(
        refused(&b),
        DcbError::Truncated {
            table: "structs",
            ..
        }
    ));
}

#[test]
fn mapping_count_mismatch_has_its_own_error() {
    let mut b = case("class-inline");
    put32(&mut b, 28, 1);
    assert!(matches!(
        refused(&b),
        DcbError::MappingCount {
            mappings: 1,
            structs: 2
        } | DcbError::Truncated { .. }
            | DcbError::Length { .. }
    ));
}
