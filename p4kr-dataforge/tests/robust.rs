//! No input panics or drags on: mutations of each case of `golden.json`, then absurd
//! records built here (chain of strong pointers, array of a million announced elements,
//! struct shared by 50,000 instances, fanned-out pointers, endless paths and names,
//! inline classes nested 70,000 levels deep).

use p4kr_dataforge::{Db, RecordError};
use p4kr_xmlwrite::MAX_OUTPUT;
use std::time::{Duration, Instant};

const LIMIT: Duration = Duration::from_secs(2);

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).expect("hex"))
        .collect()
}

fn inputs() -> Vec<(String, Vec<u8>)> {
    let json: serde_json::Value =
        serde_json::from_str(include_str!("golden.json")).expect("golden.json");
    json.as_array()
        .expect("array")
        .iter()
        .map(|c| {
            let name = c["name"].as_str().expect("name").to_string();
            (name, unhex(c["input_hex"].as_str().expect("input_hex")))
        })
        .collect()
}

/// Opens and renders all outputs; returns the number of outputs and the largest size written.
fn render_all(bytes: &[u8]) -> (usize, usize, Vec<Result<(), RecordError>>) {
    let Ok(db) = Db::open(bytes) else {
        return (0, 0, Vec::new());
    };
    let outputs = db.outputs();
    let mut largest = 0;
    let results = outputs
        .iter()
        .map(|o| {
            let r = db.render(o)?;
            largest = largest.max(r.map_or(0, |x| x.len()));
            Ok(())
        })
        .collect();
    (outputs.len(), largest, results)
}

type Rendered = (usize, usize, Vec<Result<(), RecordError>>);

/// Inputs whose work is small by nature: under `LIMIT`.
fn check_fast(label: &str, bytes: &[u8]) -> Rendered {
    check_within(label, bytes, LIMIT)
}

fn check_within(label: &str, bytes: &[u8], limit: Duration) -> Rendered {
    let t = Instant::now();
    let r = render_all(bytes);
    assert!(t.elapsed() < limit, "{label}: {:?}", t.elapsed());
    assert!(r.1 <= MAX_OUTPUT, "{label}: output of {} bytes", r.1);
    r
}

#[test]
fn truncations_never_panic() {
    for (name, bytes) in inputs() {
        let step = if bytes.len() < 4096 {
            1
        } else {
            bytes.len() / 500
        };
        let tail = bytes.len().saturating_sub(64);
        for len in (0..bytes.len()).step_by(step).chain(tail..bytes.len()) {
            check_fast(&format!("{name} truncated at {len}"), &bytes[..len]);
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
    for (name, bytes) in inputs() {
        for k in 0..200 {
            let mut mutant = bytes.clone();
            let at = (next() % bytes.len() as u64) as usize;
            mutant[at] = (next() & 0xFF) as u8;
            check_fast(&format!("{name} mutation {k} at {at}"), &mutant);
        }
    }
}

// --- Small builder of v8 `.dcb` files --------------------------------------------------------------

const I32: u16 = 0x04;
const CLASS: u16 = 0x10;
const STRONG: u16 = 0x110;
const ATTRIBUTE: u16 = 0;
const SIMPLE_ARRAY: u16 = 2;
const CLASS_ARRAY: u16 = 3;
/// Rank of the tables in the file (after the five definition tables).
const T_I32: usize = 2;
const T_STRONG: usize = 15;

#[derive(Default)]
struct Dcb {
    structs: Vec<u8>,
    fields: Vec<u8>,
    mappings: Vec<(u32, u32)>,
    records: Vec<u8>,
    /// Value tables in file order (i8 … enum option): (count, bytes).
    values: [(u32, Vec<u8>); 19],
    text: Vec<u8>,
    blob: Vec<u8>,
    data: Vec<u8>,
    field_count: u32,
}

fn pool(pool: &mut Vec<u8>, s: &str) -> u32 {
    let at = pool.len() as u32;
    pool.extend_from_slice(s.as_bytes());
    pool.push(0);
    at
}

impl Dcb {
    /// Struct without a parent: `(name, type, conversion, index)` per field; `instances` data rows.
    fn structure(
        &mut self,
        name: &str,
        fields: &[(&str, u16, u16, u16)],
        instances: u32,
        data: &[u8],
    ) {
        let size: u32 = fields
            .iter()
            .map(|&(_, t, c, _)| match (c, t) {
                (0, I32) => 4,
                _ => 8,
            })
            .sum();
        self.sized(name, fields, size, instances, data);
    }

    /// `structure` with declared size `size` (an inline class has the size of its struct).
    fn sized(
        &mut self,
        name: &str,
        fields: &[(&str, u16, u16, u16)],
        size: u32,
        instances: u32,
        data: &[u8],
    ) {
        assert_eq!(data.len() as u64, u64::from(size) * u64::from(instances));
        let name = pool(&mut self.blob, name);
        let n = self.mappings.len() as u32;
        self.structs.extend(name.to_le_bytes());
        self.structs.extend(u32::MAX.to_le_bytes());
        self.structs.extend((fields.len() as u16).to_le_bytes());
        self.structs.extend((self.field_count as u16).to_le_bytes());
        self.structs.extend(size.to_le_bytes());
        for &(f, t, c, index) in fields {
            let f = pool(&mut self.blob, f);
            for x in [
                f.to_le_bytes().to_vec(),
                [index, t, c, 0]
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect(),
            ] {
                self.fields.extend(x);
            }
            self.field_count += 1;
        }
        self.mappings.push((instances, n));
        self.data.extend_from_slice(data);
    }

    /// Makes struct `child` a child of `parent` (its size does not change if `parent` is empty).
    fn set_parent(&mut self, child: u32, parent: u32) {
        let at = 16 * child as usize + 4;
        self.structs[at..at + 4].copy_from_slice(&parent.to_le_bytes());
    }

    /// `n` `i32` fields that no struct declares, to push the field indices.
    fn pad_fields(&mut self, n: u32) {
        let name = pool(&mut self.blob, "pad");
        for _ in 0..n {
            self.fields.extend(name.to_le_bytes());
            self.fields.extend(
                [0u16, I32, ATTRIBUTE, 0]
                    .iter()
                    .flat_map(|v| v.to_le_bytes()),
            );
        }
        self.field_count += n;
    }

    fn record(&mut self, name: &str, path: &str, s: u32, variant: u16) {
        let (name, path) = (pool(&mut self.blob, name), pool(&mut self.text, path));
        self.record_at(name, path, s, variant);
    }

    /// Record at the given name (blob) and path (text) offsets.
    fn record_at(&mut self, name: u32, path: u32, s: u32, variant: u16) {
        for x in [name, path, 0, s] {
            self.records.extend(x.to_le_bytes());
        }
        self.records.extend([7u8; 16]);
        self.records.extend(variant.to_le_bytes());
        self.records.extend(0u16.to_le_bytes());
    }

    fn value(&mut self, table: usize, row: &[u8]) {
        self.values[table].0 += 1;
        self.values[table].1.extend_from_slice(row);
    }

    fn build(self) -> Vec<u8> {
        let rows = |v: &[u8], n: usize| (v.len() / n) as u32;
        let defs = [
            rows(&self.structs, 16),
            self.field_count,
            0,
            self.mappings.len() as u32,
            rows(&self.records, 36),
        ];
        // Header counters: the boolean (8th value table of the file) comes before the integers.
        let v: Vec<u32> = self.values.iter().map(|t| t.0).collect();
        let counters = [&defs[..], &[v[8]], &v[0..8], &v[9..]].concat();
        let mut out = vec![0u8; 4];
        out.extend(8i32.to_le_bytes());
        out.extend([0u8; 8]);
        for c in counters {
            out.extend(c.to_le_bytes());
        }
        out.extend((self.text.len() as u32).to_le_bytes());
        out.extend((self.blob.len() as u32).to_le_bytes());
        out.extend(&self.structs);
        out.extend(&self.fields);
        for (count, s) in &self.mappings {
            out.extend(count.to_le_bytes());
            out.extend(s.to_le_bytes());
        }
        out.extend(&self.records);
        for (_, t) in &self.values {
            out.extend(t);
        }
        for part in [&self.text, &self.blob, &self.data] {
            out.extend(part);
        }
        out
    }
}

fn pointer(s: u32, v: u16) -> Vec<u8> {
    [
        s.to_le_bytes().to_vec(),
        v.to_le_bytes().to_vec(),
        vec![0, 0],
    ]
    .concat()
}

fn array(count: u32, first: u32) -> Vec<u8> {
    [count.to_le_bytes(), first.to_le_bytes()].concat()
}

fn single(db: &[u8]) -> Vec<u8> {
    let (outputs, _, results) = check_fast("synthetic", db);
    assert_eq!(outputs, results.len());
    let db = Db::open(db).expect("open");
    match db.render(&db.outputs()[0]) {
        Ok(Some(x)) => x,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_strong_chain_of_ten_thousand_levels_stops_at_the_reference_depth() {
    let mut db = Dcb::default();
    let n = 10_000u32;
    let data: Vec<u8> = (0..=n)
        .flat_map(|k| {
            let next = if k == n {
                pointer(u32::MAX, 0xFFFF)
            } else {
                pointer(0, (k + 1) as u16)
            };
            [(k as i32).to_le_bytes().to_vec(), next].concat()
        })
        .collect();
    db.structure(
        "Node",
        &[("id", I32, ATTRIBUTE, 0), ("next", STRONG, ATTRIBUTE, 0)],
        n + 1,
        &data,
    );
    db.record("Node.one", "a/one.xml", 0, 0);
    let xml = String::from_utf8(single(&db.build())).expect("utf-8");
    assert_eq!(
        xml.matches("<Node ").count(),
        100,
        "100 levels under the record"
    );
}

#[test]
fn a_million_announced_array_elements_are_cut_at_ten_thousand() {
    let mut db = Dcb::default();
    for k in 0..4i32 {
        db.value(T_I32, &k.to_le_bytes());
    }
    db.structure(
        "Holder",
        &[("list", I32, SIMPLE_ARRAY, 0)],
        1,
        &array(1_000_000, 0),
    );
    db.record("Holder.one", "a/one.xml", 0, 0);
    let xml = String::from_utf8(single(&db.build())).expect("utf-8");
    assert_eq!(
        xml.matches("<Int32 ").count(),
        5,
        "4 rows and the one after"
    );
    assert_eq!(xml.matches("<list value=").count(), 10_000 - 5);
}

#[test]
fn a_structure_shared_by_fifty_thousand_instances_renders_quickly() {
    let mut db = Dcb::default();
    let n = 50_000u32;
    db.structure(
        "Holder",
        &[("items", CLASS, CLASS_ARRAY, 1)],
        1,
        &array(n, 0),
    );
    let items: Vec<u8> = (0..n as i32).flat_map(i32::to_le_bytes).collect();
    db.structure("Item", &[("v", I32, ATTRIBUTE, 0)], n, &items);
    db.record("Holder.one", "a/holder.xml", 0, 0);
    for k in 0..n {
        db.record(&format!("Item.r{k}"), &format!("a/r{k}.xml"), 1, k as u16);
    }
    let bytes = db.build();
    let (outputs, _, results) = check_fast("50,000 records", &bytes);
    assert_eq!(outputs, n as usize + 1);
    assert!(results.iter().all(Result::is_ok));
    let db = Db::open(&bytes).expect("open");
    let xml = db.render(&db.outputs()[0]).expect("render").expect("file");
    assert_eq!(
        String::from_utf8_lossy(&xml).matches("<Item ").count(),
        10_000
    );
}

#[test]
fn fanned_out_pointers_are_bounded() {
    let mut db = Dcb::default();
    let (fan, kinds) = (10_000u32, 300u32);
    for k in 0..fan {
        db.value(T_STRONG, &pointer(0, (k % kinds) as u16));
    }
    let data: Vec<u8> = (0..kinds).flat_map(|_| array(fan, 0)).collect();
    db.structure("Node", &[("kids", STRONG, SIMPLE_ARRAY, 0)], kinds, &data);
    db.record("Node.one", "a/one.xml", 0, 0);
    let bytes = db.build();
    // The work goes up to the step cap (2^24): it is the outcome that is checked; the generous time
    // bound only serves to catch a cap that would no longer apply.
    let (_, _, results) = check_within("fan-out", &bytes, Duration::from_secs(10));
    assert_eq!(results, vec![Err(RecordError::TooMuchWork)]);
}

#[test]
fn a_record_path_outside_the_text_refuses_the_file_like_the_reference() {
    let mut db = Dcb::default();
    db.structure("Thing", &[("v", I32, ATTRIBUTE, 0)], 1, &7i32.to_le_bytes());
    db.record("Thing.one", "a/one.xml", 0, 0);
    let mut bytes = db.build();
    // Path offset (2nd field of the first record) pushed beyond the text.
    let record = 120 + 16 + 12 + 8;
    bytes[record + 4..record + 8].copy_from_slice(&1000u32.to_le_bytes());
    assert!(matches!(
        Db::open(&bytes),
        Err(p4kr_dataforge::DcbError::RecordPath {
            index: 0,
            offset: 1000,
            ..
        })
    ));
}

#[test]
fn a_record_fault_becomes_the_error_element_with_the_reference_message() {
    let mut db = Dcb::default();
    db.structure("Thing", &[("v", I32, ATTRIBUTE, 0)], 1, &7i32.to_le_bytes());
    db.record("Thing.one", "a/one.xml", 5, 0);
    let xml = single(&db.build());
    assert_eq!(
        String::from_utf8_lossy(&xml),
        "<Error value=\"Index 5 is out of range for Struct Definition values (count: 1)\" />"
    );
}

#[test]
fn strong_reads_under_fifty_thousand_empty_ancestors_cost_nothing_per_ancestor() {
    let mut db = Dcb::default();
    let depth = 50_000u32;
    for k in 0..depth {
        db.structure("Empty", &[], 0, &[]);
        if k > 0 {
            db.set_parent(k, k - 1);
        }
    }
    let n = 10_000u32;
    let leaves: Vec<u8> = (0..n as i32).flat_map(i32::to_le_bytes).collect();
    db.structure("Leaf", &[("v", I32, ATTRIBUTE, 0)], n, &leaves);
    db.set_parent(depth, depth - 1);
    for k in 0..n {
        db.value(T_STRONG, &pointer(depth, k as u16));
    }
    db.structure(
        "Holder",
        &[("list", STRONG, SIMPLE_ARRAY, 0)],
        1,
        &array(n, 0),
    );
    db.record("Holder.one", "a/one.xml", depth + 1, 0);
    let xml = String::from_utf8(single(&db.build())).expect("utf-8");
    assert_eq!(xml.matches("<Leaf ").count(), n as usize);
}

#[test]
fn a_field_range_past_the_reference_16_bit_index_is_refused() {
    let mut db = Dcb::default();
    db.pad_fields(u32::from(u16::MAX));
    db.structure("Wide", &[("v", I32, ATTRIBUTE, 0)], 0, &[]);
    assert!(matches!(
        Db::open(&db.build()),
        Err(p4kr_dataforge::DcbError::FieldIndex {
            index: 0,
            end: 65_536
        })
    ));
}

#[test]
fn a_path_without_end_refuses_the_file_before_anything_is_read() {
    // 10 MB of text with no NUL at the end of the file, 10,000 paths inside it: read in full, that would be
    // 10,000 paths of about 10 MB each.
    let mut db = Dcb::default();
    db.structure("Thing", &[], 0, &[]);
    db.blob.clear();
    db.text = vec![b'a'; 10 << 20];
    for k in 0..10_000 {
        db.record_at(0, k, 0, 0);
    }
    let bytes = db.build();
    assert_eq!(bytes.last(), Some(&b'a'));
    let (outputs, _, _) = check_fast("endless path", &bytes);
    assert_eq!(outputs, 0);
    assert!(matches!(
        Db::open(&bytes),
        Err(p4kr_dataforge::DcbError::RecordPathLength {
            index: 0,
            max: 4096
        })
    ));
}

#[test]
fn a_long_name_read_behind_cycled_pointers_is_charged_to_the_work_budget() {
    // 10,000 pointers to the already-open struct: each re-reads its 4 MB name then stops on
    // the cycle, with no step or output; read without counting, 40 GB per record.
    let mut db = Dcb::default();
    let fan = 10_000u32;
    for _ in 0..fan {
        db.value(T_STRONG, &pointer(0, 0));
    }
    let long = "N".repeat(4 << 20);
    db.structure(
        &long,
        &[("kids", STRONG, SIMPLE_ARRAY, 0)],
        1,
        &array(fan, 0),
    );
    db.record("Node.one", "a/one.xml", 0, 0);
    let bytes = db.build();
    let (_, _, results) = check_within("name re-read", &bytes, Duration::from_secs(20));
    assert_eq!(results, vec![Err(RecordError::TooMuchWork)]);
}

#[test]
fn seventy_thousand_nested_inline_classes_are_bounded() {
    // S0 → S1 → … → S{n} as inline classes (16-bit indices), the leaf carrying a strong pointer
    // to the next instance of S0: two passes, about 70,000 frames open at once.
    let mut db = Dcb::default();
    let n = 35_000u16;
    for k in 0..n {
        let instances = if k == 0 { 2 } else { 0 };
        let data = if k == 0 {
            [pointer(0, 1), pointer(u32::MAX, 0xFFFF)].concat()
        } else {
            Vec::new()
        };
        db.sized("S", &[("c", CLASS, ATTRIBUTE, k + 1)], 8, instances, &data);
    }
    db.sized("Leaf", &[("next", STRONG, ATTRIBUTE, 0)], 8, 0, &[]);
    db.record("S.one", "a/one.xml", 0, 0);
    let bytes = db.build();
    let (_, _, results) = check_within("inline classes", &bytes, Duration::from_secs(10));
    assert_eq!(results, vec![Err(RecordError::TooMuchWork)]);
}
