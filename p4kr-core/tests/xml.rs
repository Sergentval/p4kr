//! `--xml` extraction on the synthetic archive in `testdata/`; expected bytes taken from the expected file
//! of p4kr-cryxml.

mod common;

use p4kr_core::extract::{Extras, Options, RecordProgress, extract_with, xml_target};
use p4kr_core::{Archive, Filter, Verify};
use p4kr_dataforge::RecordError;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

fn archives() -> (PathBuf, PathBuf) {
    (
        common::testdata("xml.p4k"),
        common::testdata("collision.p4k"),
    )
}

/// Cases of the expected file: name → (entry, output).
fn golden() -> HashMap<String, (Vec<u8>, Option<Vec<u8>>)> {
    let text = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../p4kr-cryxml/tests/golden.json"),
    )
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let unhex = |s: &str| {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect::<Vec<u8>>()
    };
    v.as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["name"].as_str().unwrap().to_string(),
                (
                    unhex(c["input_hex"].as_str().unwrap()),
                    c["output_hex"].as_str().map(unhex),
                ),
            )
        })
        .collect()
}

fn opts(out: &Path, threads: usize) -> Options<'_> {
    Options {
        out: Some(out),
        key: None,
        verify: Verify::Crc,
        threads,
        fail_fast: false,
    }
}

#[test]
fn xml_target_changes_or_appends_the_extension() {
    assert_eq!(xml_target(Path::new("/o/a.mtl")), PathBuf::from("/o/a.xml"));
    assert_eq!(xml_target(Path::new("/o/a.xml")), PathBuf::from("/o/a.xml"));
    assert_eq!(xml_target(Path::new("/o/a.XML")), PathBuf::from("/o/a.xml"));
    assert_eq!(
        xml_target(Path::new("/o/a.b.c")),
        PathBuf::from("/o/a.b.xml")
    );
    assert_eq!(
        xml_target(Path::new("/o/noext")),
        PathBuf::from("/o/noext.xml")
    );
}

#[test]
fn extraction_writes_the_same_tree_as_unp4k_then_unforge() {
    let g = golden();
    for threads in [1, 8] {
        let tmp = tempfile::tempdir().unwrap();
        let (xml, _) = archives();
        let out = tmp.path().join("out");
        let a = Archive::open(&xml).unwrap();
        let done = AtomicUsize::new(0);
        let r = extract_with(
            &a,
            &a.select(&Filter::All),
            &opts(&out, threads),
            &Extras {
                xml: true,
                progress: Some(&done),
                records: None,
            },
        )
        .unwrap();
        let d = out.join("Data/Xml");
        let read = |n: &str| std::fs::read(d.join(n)).unwrap();
        assert_eq!(read("nested.xml"), g["nested"].1.clone().unwrap());
        assert_eq!(read("side.mtl"), g["attributes"].0);
        assert_eq!(read("side.xml"), g["attributes"].1.clone().unwrap());
        assert_eq!(read("text.xml"), g["text-xml"].0);
        assert_eq!(read("other.bin"), g["other-binary"].0);
        assert_eq!(read("partial.xml"), g["cdata-end"].1.clone().unwrap());
        assert_eq!(read("broken.xml"), g["truncated-tables"].0);
        assert_eq!(read("base.dcb"), g["nested"].0);
        assert!(!d.join("base.xml").exists());
        assert_eq!(r.converted, 2);
        assert_eq!(r.xml_failed.len(), 2);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(done.load(Relaxed), 7);
        let on_disk: u64 = std::fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().metadata().unwrap().len())
            .sum();
        assert_eq!(r.bytes, on_disk);
        assert!(std::fs::read_dir(&d).unwrap().all(|e| {
            !e.unwrap()
                .file_name()
                .to_string_lossy()
                .contains("p4kr-tmp")
        }));
    }
}

#[test]
fn a_side_file_never_overwrites_another_entry() {
    let g = golden();
    let tmp = tempfile::tempdir().unwrap();
    let (_, collision) = archives();
    let out = tmp.path().join("out");
    let a = Archive::open(&collision).unwrap();
    let r = extract_with(
        &a,
        &a.select(&Filter::All),
        &opts(&out, 4),
        &Extras {
            xml: true,
            progress: None,
            records: None,
        },
    )
    .unwrap();
    let d = out.join("Data/Xml");
    assert_eq!(
        std::fs::read(d.join("a.xml")).unwrap(),
        g["attributes"].1.clone().unwrap()
    );
    assert_eq!(std::fs::read(d.join("a.mtl")).unwrap(), g["nested"].0);
    assert_eq!(r.errors.len(), 1);
    assert_eq!(
        a.name(r.errors[0].0).unwrap().replace('\\', "/"),
        "Data/Xml/a.mtl"
    );
    assert_eq!(r.bytes, files_size(&d));
    assert!(no_tmp(&d));
}

fn files_size(d: &Path) -> u64 {
    std::fs::read_dir(d)
        .unwrap()
        .map(|e| e.unwrap().metadata().unwrap())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}

fn no_tmp(d: &Path) -> bool {
    std::fs::read_dir(d).unwrap().all(|e| {
        !e.unwrap()
            .file_name()
            .to_string_lossy()
            .contains("p4kr-tmp")
    })
}

#[test]
fn two_side_files_on_one_target_are_both_refused() {
    let g = golden();
    let tmp = tempfile::tempdir().unwrap();
    let archive = common::testdata("sides.p4k");
    let out = tmp.path().join("out");
    let a = Archive::open(&archive).unwrap();
    let r = extract_with(
        &a,
        &a.select(&Filter::All),
        &opts(&out, 4),
        &Extras {
            xml: true,
            progress: None,
            records: None,
        },
    )
    .unwrap();
    let d = out.join("Data/Xml");
    assert_eq!(std::fs::read(d.join("s.mtl")).unwrap(), g["nested"].0);
    assert_eq!(std::fs::read(d.join("s.mat")).unwrap(), g["attributes"].0);
    assert!(!d.join("s.xml").exists());
    assert_eq!(r.errors.len(), 2);
    assert_eq!(r.converted, 0);
    assert_eq!(r.bytes, files_size(&d));
    assert!(no_tmp(&d));
}

#[test]
fn a_failed_side_write_keeps_the_byte_count_exact() {
    let g = golden();
    let tmp = tempfile::tempdir().unwrap();
    let (xml, _) = archives();
    let out = tmp.path().join("out");
    std::fs::create_dir_all(out.join("Data/Xml/side.xml")).unwrap();
    let a = Archive::open(&xml).unwrap();
    let done = AtomicUsize::new(0);
    let r = extract_with(
        &a,
        &a.select(&Filter::All),
        &opts(&out, 2),
        &Extras {
            xml: true,
            progress: Some(&done),
            records: None,
        },
    )
    .unwrap();
    let d = out.join("Data/Xml");
    assert_eq!(
        std::fs::read(d.join("side.mtl")).unwrap(),
        g["attributes"].0
    );
    assert!(d.join("side.xml").is_dir());
    assert!(
        r.errors
            .iter()
            .any(|(i, _)| a.name(*i).unwrap().replace('\\', "/") == "Data/Xml/side.mtl")
    );
    assert_eq!(r.bytes, files_size(&d));
    assert!(no_tmp(&d));
    assert_eq!(done.load(Relaxed), 7);
}

#[test]
fn without_xml_nothing_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let (xml, _) = archives();
    let out = tmp.path().join("out");
    let a = Archive::open(&xml).unwrap();
    let r = extract_with(
        &a,
        &a.select(&Filter::All),
        &opts(&out, 2),
        &Extras::default(),
    )
    .unwrap();
    assert_eq!(r.converted, 0);
    assert!(!out.join("Data/Xml/side.xml").exists());
}

#[test]
fn duplicated_indices_are_extracted_once() {
    let g = golden();
    let tmp = tempfile::tempdir().unwrap();
    let (xml, _) = archives();
    let out = tmp.path().join("out");
    let a = Archive::open(&xml).unwrap();
    let all = a.select(&Filter::All);
    let doubled: Vec<usize> = all.iter().chain(&all).copied().collect();
    let r = extract_with(&a, &doubled, &opts(&out, 8), &Extras::default()).unwrap();
    let d = out.join("Data/Xml");
    assert_eq!(r.entries, all.len());
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(
        std::fs::read(d.join("side.mtl")).unwrap(),
        g["attributes"].0
    );
    assert_eq!(r.bytes, files_size(&d));
    assert!(no_tmp(&d));
}

#[test]
fn the_record_counter_reaches_its_total_on_a_dataforge_archive() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out");
    let a = Archive::open(&common::testdata("dcb.p4k")).unwrap();
    let records = RecordProgress::default();
    let r = extract_with(
        &a,
        &a.select(&Filter::All),
        &opts(&out, 2),
        &Extras {
            xml: true,
            records: Some(&records),
            ..Extras::default()
        },
    )
    .unwrap();
    let total = records.total.load(Relaxed);
    assert!(total > 0);
    assert_eq!(records.done.load(Relaxed), total);
    let written: usize = r.dcb.iter().map(|(_, d)| d.as_ref().unwrap().records).sum();
    assert_eq!(written, total);
}

#[test]
fn without_xml_the_record_counter_stays_at_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out");
    let a = Archive::open(&common::testdata("dcb.p4k")).unwrap();
    let records = RecordProgress::default();
    extract_with(
        &a,
        &a.select(&Filter::All),
        &opts(&out, 2),
        &Extras {
            records: Some(&records),
            ..Extras::default()
        },
    )
    .unwrap();
    assert_eq!(records.total.load(Relaxed), 0);
}

fn extract_record_over_entry(out: &Path) -> p4kr_core::extract::Report {
    let a = Archive::open(&common::testdata("record-over-entry.p4k")).unwrap();
    extract_with(
        &a,
        &a.select(&Filter::All),
        &opts(out, 2),
        &Extras {
            xml: true,
            ..Extras::default()
        },
    )
    .unwrap()
}

#[test]
fn a_record_takes_the_path_of_an_entry_which_is_kept_beside_it() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out");
    let r = extract_record_over_entry(&out);
    let data = out.join("Data");
    let read = |n: &str| std::fs::read(data.join(n)).unwrap();
    let has = |n: &str, needle: &str| String::from_utf8_lossy(&read(n)).contains(needle);
    assert!(r.errors.is_empty() && r.xml_failed.is_empty());
    // raw text and side XML: the record takes the path, the old content stays aside
    assert_eq!(read("other.p4k-entry.xml"), b"<a/>");
    assert!(has("other.xml", "value=\"1\""));
    assert!(has("side.xml", "value=\"2\""));
    assert!(!has("side.p4k-entry.xml", "value=\"2\""));
    assert!(has("side.p4k-entry.xml", "<"));
    assert!(has("free.xml", "value=\"4\""));
    // a `.dcb` is never displaced; a side name taken by a record leaves the entry intact
    assert!(p4kr_dataforge::Db::open(&read("base.dcb")).is_ok());
    assert_eq!(read("taken.xml"), b"<t/>");
    assert!(has("taken.p4k-entry.xml", "value=\"6\""));
    assert!(!data.join("base.p4k-entry.dcb").exists());

    let (_, d) = &r.dcb[0];
    let d = d.as_ref().unwrap();
    assert_eq!(d.records, 4);
    let failed: Vec<_> = d.failed.iter().map(|(_, e)| e.to_string()).collect();
    assert_eq!(failed.len(), 2, "{failed:?}");
    assert!(d.failed.iter().all(|(_, e)| *e == RecordError::Collision));
    let mut displaced = d.displaced.clone();
    displaced.sort();
    assert_eq!(
        displaced,
        [
            (data.join("other.xml"), data.join("other.p4k-entry.xml")),
            (data.join("side.xml"), data.join("side.p4k-entry.xml")),
        ]
    );
}

#[test]
fn a_missing_entry_file_is_not_displaced() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out");
    let a = Archive::open(&common::testdata("record-over-entry.p4k")).unwrap();
    // only the `.dcb` is extracted: the targeted entries do not exist, so nothing is displaced
    let only = a.select(&Filter::ext(".dcb"));
    let r = extract_with(
        &a,
        &only,
        &opts(&out, 2),
        &Extras {
            xml: true,
            ..Extras::default()
        },
    )
    .unwrap();
    let d = r.dcb[0].1.as_ref().unwrap();
    assert!(d.displaced.is_empty());
    assert_eq!(d.failed.len(), 1); // base.dcb
    assert_eq!(d.records, 5);
}

#[test]
fn an_entry_stays_in_place_when_its_record_is_skipped_or_fails_to_render() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out");
    let a = Archive::open(&common::testdata("record-unrendered.p4k")).unwrap();
    let extras = Extras {
        xml: true,
        ..Extras::default()
    };
    let r = extract_with(&a, &a.select(&Filter::All), &opts(&out, 2), &extras).unwrap();
    let data = out.join("Data");
    let d = r.dcb[0].1.as_ref().unwrap();
    // skipped: the entry stays, with no side name or warning; on failure: same, the failure is reported
    assert_eq!(std::fs::read(data.join("skip.xml")).unwrap(), b"<skip/>");
    assert_eq!(std::fs::read(data.join("bad.xml")).unwrap(), b"<bad/>");
    assert!(!data.join("skip.p4k-entry.xml").exists());
    assert!(!data.join("bad.p4k-entry.xml").exists());
    assert_eq!(d.failed.len(), 1);
    assert_eq!(d.failed[0].1, RecordError::BadName);
    // witness: a successful render does displace the entry
    assert_eq!(
        std::fs::read(data.join("good.p4k-entry.xml")).unwrap(),
        b"<good/>"
    );
    assert_eq!(d.records, 1);
    assert_eq!(
        d.displaced,
        [(data.join("good.xml"), data.join("good.p4k-entry.xml"))]
    );
}
