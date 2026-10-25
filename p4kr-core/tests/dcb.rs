//! Writing the records of a `.dcb` from the expected file of p4kr-dataforge.

mod common;

use p4kr_core::dcb::write_records;
use p4kr_dataforge::{Db, RecordError};
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Prefix of the p4kr temporary files (`extract.rs::TMP_PREFIX`): ':' would open an alternate stream on Windows.
#[cfg(not(windows))]
const TMP: &str = ".p4kr-tmp:";
#[cfg(windows)]
const TMP: &str = ".p4kr-tmp~";

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn case(name: &str) -> (Vec<u8>, Vec<Value>) {
    let v: Value =
        serde_json::from_str(include_str!("../../p4kr-dataforge/tests/golden.json")).unwrap();
    let c = v
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("case {name}"))
        .clone();
    (
        unhex(c["input_hex"].as_str().unwrap()),
        c["records"].as_array().unwrap().clone(),
    )
}

fn run(input: &[u8], dir: &Path, taken: &HashSet<PathBuf>) -> p4kr_core::dcb::DcbReport {
    let db = Db::open(input).expect("database opened");
    write_records(&db, dir, taken, 2, None).expect("write started")
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                found.push(p);
            }
        }
    }
    found
}

#[test]
fn writes_each_written_record_byte_for_byte() {
    let (input, records) = case("records-nested");
    let tmp = tempfile::tempdir().unwrap();
    let report = run(&input, tmp.path(), &HashSet::new());
    let written: Vec<&Value> = records
        .iter()
        .filter(|r| r["outcome"] == "written")
        .collect();
    assert_eq!(report.records, written.len());
    assert!(report.failed.is_empty());
    for r in &written {
        let want = unhex(r["xml_hex"].as_str().unwrap());
        let got = std::fs::read(tmp.path().join(r["path"].as_str().unwrap())).unwrap();
        assert_eq!(got, want, "{}", r["path"]);
    }
    assert_eq!(files_under(tmp.path()).len(), written.len());
    assert_eq!(
        report.bytes,
        written
            .iter()
            .map(|r| r["xml_hex"].as_str().unwrap().len() as u64 / 2)
            .sum::<u64>()
    );
}

#[test]
fn skips_records_the_reference_does_not_write() {
    let v: Value =
        serde_json::from_str(include_str!("../../p4kr-dataforge/tests/golden.json")).unwrap();
    let mut checked = 0;
    for c in v.as_array().unwrap() {
        let skipped: Vec<&str> = c["records"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["outcome"] == "skipped")
            .map(|r| r["path"].as_str().unwrap())
            .collect();
        let input = unhex(c["input_hex"].as_str().unwrap());
        let Ok(db) = Db::open(&input) else {
            continue;
        };
        let tmp = tempfile::tempdir().unwrap();
        write_records(&db, tmp.path(), &HashSet::new(), 1, None).unwrap();
        for p in skipped {
            checked += 1;
            assert!(!tmp.path().join(p).exists(), "{}: {p}", c["name"]);
        }
    }
    assert!(checked > 0, "no \"skipped\" case in the expected file");
}

#[test]
fn backslash_is_a_literal_file_name_character() {
    let (input, records) = case("path-backslash");
    let tmp = tempfile::tempdir().unwrap();
    let report = run(&input, tmp.path(), &HashSet::new());
    let name = records[0]["path"].as_str().unwrap();
    assert!(name.contains('\\'));
    if cfg!(windows) {
        // '\\' is a separator on Windows: refused, nothing is written
        assert_eq!(report.records, 0);
        assert_eq!(report.failed[0].1, RecordError::UnsafePath);
        assert!(files_under(tmp.path()).is_empty());
        return;
    }
    assert_eq!(report.records, 1);
    let got = std::fs::read(tmp.path().join(name)).unwrap();
    assert_eq!(got, unhex(records[0]["xml_hex"].as_str().unwrap()));
    assert_eq!(files_under(tmp.path()).len(), 1);
}

#[test]
fn dotdot_path_is_refused_and_nothing_escapes() {
    let (input, _) = case("path-dotdot");
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out");
    let report = run(&input, &out, &HashSet::new());
    assert_eq!(report.records, 0);
    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].1, RecordError::UnsafePath);
    assert_eq!(report.failed[0].0.len(), 64);
    assert!(files_under(tmp.path()).is_empty());
}

#[test]
fn longer_old_file_is_replaced_entirely() {
    let (input, records) = case("records-nested");
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(records[0]["path"].as_str().unwrap());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, vec![b'z'; 100_000]).unwrap();
    run(&input, tmp.path(), &HashSet::new());
    let want = unhex(records[0]["xml_hex"].as_str().unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), want);
}

#[test]
fn orphan_temporary_disappears() {
    let (input, _) = case("records-nested");
    let tmp = tempfile::tempdir().unwrap();
    let orphan = tmp.path().join(format!("{TMP}d0-0"));
    std::fs::write(&orphan, b"leftover").unwrap();
    run(&input, tmp.path(), &HashSet::new());
    assert!(!orphan.exists());
    assert!(
        files_under(tmp.path())
            .iter()
            .all(|p| !p.to_string_lossy().contains(".p4kr-tmp"))
    );
}

#[test]
fn taken_target_is_a_collision_and_keeps_the_existing_file() {
    let (input, records) = case("records-nested");
    let tmp = tempfile::tempdir().unwrap();
    let rel = records[3]["path"].as_str().unwrap();
    let taken_path = tmp.path().join(rel);
    std::fs::write(&taken_path, b"existing").unwrap();
    let taken: HashSet<PathBuf> = [taken_path.clone()].into();
    let report = run(&input, tmp.path(), &taken);
    assert_eq!(std::fs::read(&taken_path).unwrap(), b"existing");
    assert_eq!(report.records, records.len() - 1);
    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].1, RecordError::Collision);
}

#[test]
fn duplicate_path_keeps_the_place_of_the_first_and_the_bytes_of_the_last_record() {
    let (input, records) = case("duplicate-path");
    let tmp = tempfile::tempdir().unwrap();
    let report = run(&input, tmp.path(), &HashSet::new());
    assert_eq!(report.records, 2);
    assert!(report.failed.is_empty());
    for r in &records {
        let got = std::fs::read(tmp.path().join(r["path"].as_str().unwrap())).unwrap();
        assert_eq!(got, unhex(r["xml_hex"].as_str().unwrap()));
    }
}

#[test]
fn orphan_temporaries_are_swept_in_every_output_directory() {
    let (input, _) = case("records-nested");
    let tmp = tempfile::tempdir().unwrap();
    let orphans = [
        format!("{TMP}d0-77"),
        format!("lib0/a/{TMP}d0-78"),
        format!("lib0/c/d/e/{TMP}d0-79"),
    ];
    for o in &orphans {
        let p = tmp.path().join(o);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"leftover").unwrap();
    }
    let other = tmp.path().join(format!("lib0/a/{TMP}d1-0"));
    std::fs::write(&other, b"other dcb").unwrap();
    run(&input, tmp.path(), &HashSet::new());
    for o in &orphans {
        assert!(!tmp.path().join(o).exists(), "{o}");
    }
    assert!(other.exists(), "the temporary file of another entry stays");
}

#[test]
fn backslash_traversal_is_refused_on_windows_and_nothing_escapes() {
    let (input, _) = case("path-backslash");
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out");
    let db = Db::open(&input).unwrap();
    let outputs = [p4kr_dataforge::Output {
        path: "a\\..\\..\\x.xml".into(),
        record: 0,
    }];
    let r = p4kr_core::dcb::write_outputs(&db, &outputs, &out, &HashSet::new(), 2, None).unwrap();
    if cfg!(windows) {
        assert_eq!(r.records, 0);
        assert_eq!(r.failed[0].1, RecordError::UnsafePath);
    }
    // nowhere outside the output folder, whatever the platform
    assert!(!tmp.path().join("x.xml").exists());
}

#[test]
fn unfolded_taken_path_collides_only_where_the_disk_folds_case() {
    let (input, records) = case("records-nested");
    let tmp = tempfile::tempdir().unwrap();
    let rel = records[3]["path"].as_str().unwrap();
    let path = tmp.path().join(rel);
    // same file with the case changed on the last component, as a caller passes it
    let upper = path.with_file_name(path.file_name().unwrap().to_string_lossy().to_uppercase());
    std::fs::write(&upper, b"existing").unwrap();
    let taken: HashSet<PathBuf> = [upper.clone()].into();
    let report = run(&input, tmp.path(), &taken);
    if cfg!(any(windows, target_os = "macos")) {
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].1, RecordError::Collision);
        assert_eq!(std::fs::read(&upper).unwrap(), b"existing");
    } else {
        assert!(report.failed.is_empty());
        assert!(path.exists() && upper.exists());
    }
}
