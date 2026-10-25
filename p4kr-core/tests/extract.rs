mod common;

use p4kr_core::extract::{Options, extract};
use p4kr_core::{Archive, Filter, Key, P4kError, Verify, target};
use sha2::{Digest, Sha256};
use std::path::Path;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn opts<'a>(out: Option<&'a Path>, key: &'a Key, verify: Verify, threads: usize) -> Options<'a> {
    Options {
        out,
        key: Some(key),
        verify,
        threads,
        fail_fast: false,
    }
}

fn walk(p: &Path) -> Vec<std::path::PathBuf> {
    let mut v = Vec::new();
    for e in std::fs::read_dir(p).unwrap() {
        let path = e.unwrap().path();
        if path.is_dir() {
            v.extend(walk(&path))
        } else {
            v.push(path)
        }
    }
    v
}

#[test]
fn filters_follow_the_bench_rules() {
    let a = Archive::open(&common::synth().archive).unwrap();
    assert_eq!(a.select(&Filter::exact("Data\\Synth\\zstd.bin")), vec![1]);
    assert_eq!(a.select(&Filter::prefix("Data/Synth/")).len(), 10);
    assert_eq!(a.select(&Filter::prefix("Data/Synt")).len(), 0);
    assert_eq!(a.select(&Filter::ext(".BIN")).len(), 10);
    assert_eq!(a.select(&Filter::All).len(), 10);
}

#[test]
fn repeated_filters_are_or_and_prefix_with_ext_is_and() {
    // release.p4k: 0 stored.txt, 1 packed.bin, 2 secret.bin, 3 nested.xml, 4 base.dcb under Data/ref/
    let a = Archive::open(&common::testdata("release.p4k")).unwrap();
    let any = |v: &[&str], f: fn(&str) -> Filter| Filter::any(v.iter().map(|s| f(s)).collect());
    assert_eq!(a.select(&any(&[".xml", ".DCB"], Filter::ext)), vec![3, 4]);
    assert_eq!(a.select(&any(&[".xml", ".xml"], Filter::ext)), vec![3]);
    let exacts = any(
        &["Data/ref/base.dcb", "Data\\ref\\stored.txt"],
        Filter::exact,
    );
    assert_eq!(a.select(&exacts), vec![0, 4]);
    let both =
        |p: &[&str], e: &[&str]| Filter::every(vec![any(p, Filter::prefix), any(e, Filter::ext)]);
    assert_eq!(
        a.select(&both(&["Data/ref"], &[".txt", ".bin"])),
        vec![0, 1, 2]
    );
    assert_eq!(
        a.select(&both(&["Data/none", "Data/ref/"], &[".xml"])),
        vec![3]
    );
    assert!(a.select(&both(&["Data/none"], &[".xml"])).is_empty());
    assert_eq!(Filter::any(vec![]), Filter::All);
    assert_eq!(Filter::any(vec![Filter::ext(".x")]), Filter::ext(".x"));
    assert_eq!(
        Filter::every(vec![Filter::All, Filter::ext(".x")]),
        Filter::ext(".x")
    );
}

#[test]
fn traversal_and_drive_names_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for bad in [
        "../x.bin",
        "..\\x.bin",
        "/etc/x",
        "C:\\x.bin",
        "./x.bin",
        "",
        "a/../../x",
    ] {
        assert!(target(root, bad).is_none(), "{bad}");
    }
    assert_eq!(
        target(root, "Data\\a.bin").unwrap(),
        root.join("Data/a.bin")
    );
}

#[test]
fn disk_extraction_writes_ok_entries_and_reports_errors() {
    let dir = tempfile::tempdir().unwrap();
    let s = common::synth();
    let a = Archive::open(&s.archive).unwrap();
    let key = Key::from_file(&s.key).unwrap();
    let out = dir.path().join("out");
    let all: Vec<usize> = (0..a.len()).collect();
    let r = extract(&a, &all, &opts(Some(&out), &key, Verify::Crc, 0)).unwrap();
    let kinds: Vec<(usize, &str)> = r.errors.iter().map(|(i, e)| (*i, e.kind())).collect();
    assert_eq!(kinds, vec![(6, "crc"), (8, "zstd"), (9, "zstd")]);
    let mut total = 0u64;
    for case in s.expected.iter().filter(|c| c["expect"] == "ok") {
        let name = format!("Data/Synth/{}.bin", case["case"].as_str().unwrap());
        let data = std::fs::read(out.join(&name)).unwrap();
        assert_eq!(
            hex(&Sha256::digest(&data)),
            case["sha256"].as_str().unwrap()
        );
        total += data.len() as u64;
    }
    // sha-faux (7) is written in CRC mode: content intact
    total += std::fs::metadata(out.join("Data/Synth/sha-faux.bin"))
        .unwrap()
        .len();
    assert_eq!(r.bytes, total);
    let leftovers = walk(&out)
        .into_iter()
        .filter(|p| p.to_string_lossy().contains(".p4kr-tmp"))
        .count();
    assert_eq!(leftovers, 0);
}

#[test]
fn full_mode_adds_the_sha_error() {
    let s = common::synth();
    let a = Archive::open(&s.archive).unwrap();
    let key = Key::from_file(&s.key).unwrap();
    let all: Vec<usize> = (0..a.len()).collect();
    let r = extract(&a, &all, &opts(None, &key, Verify::Full, 0)).unwrap();
    let kinds: Vec<&str> = r.errors.iter().map(|(_, e)| e.kind()).collect();
    assert_eq!(kinds, vec!["crc", "sha5003", "zstd", "zstd"]);
    assert_eq!(r.entries, 10);
}

#[test]
fn same_result_with_one_or_many_threads() {
    let s = common::synth();
    let a = Archive::open(&s.archive).unwrap();
    let key = Key::from_file(&s.key).unwrap();
    let all: Vec<usize> = (0..a.len()).collect();
    let one = extract(&a, &all, &opts(None, &key, Verify::Full, 1)).unwrap();
    let many = extract(&a, &all, &opts(None, &key, Verify::Full, 8)).unwrap();
    let view = |r: &p4kr_core::extract::Report| {
        (
            r.bytes,
            r.errors
                .iter()
                .map(|(i, e)| (*i, e.kind()))
                .collect::<Vec<_>>(),
        )
    };
    assert_eq!(view(&one), view(&many));
    assert_eq!((one.threads, many.threads), (1, 8));
}

#[test]
fn fail_fast_stops_at_the_first_error() {
    let s = common::synth();
    let a = Archive::open(&s.archive).unwrap();
    let key = Key::from_file(&s.key).unwrap();
    let all: Vec<usize> = (0..a.len()).collect();
    let o = Options {
        fail_fast: true,
        ..opts(None, &key, Verify::Full, 1)
    };
    assert_eq!(extract(&a, &all, &o).unwrap().errors.len(), 1);
}

// Lock reserved for any test that touches the process environment.
static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn free_bytes_reads_the_filesystem_without_df() {
    let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let free = p4kr_core::extract::free_bytes(dir.path()).unwrap();
    assert!(free > 0);
    // Empty `PATH`: no external program can be found, the measurement must come from the system
    let old = std::env::var_os("PATH");
    // SAFETY: no test in this binary spawns a process or reads the environment outside std
    // (std::env::var takes std's internal lock).
    unsafe { std::env::set_var("PATH", "") };
    let again = p4kr_core::extract::free_bytes(dir.path());
    // SAFETY: see above.
    match old {
        Some(p) => unsafe { std::env::set_var("PATH", p) },
        None => unsafe { std::env::remove_var("PATH") },
    }
    assert!(again.unwrap() > 0);
}

#[test]
fn min_free_parses_the_variable_without_touching_the_environment() {
    use p4kr_core::extract::parse_min_free;
    assert_eq!(parse_min_free(None), 20 << 30);
    assert_eq!(parse_min_free(Some("3")), 3 << 30);
    assert_eq!(parse_min_free(Some("abc")), 20 << 30);
    assert_eq!(parse_min_free(Some(" 5 ")), 5 << 30);
    assert_eq!(parse_min_free(Some("0")), 0);
}

#[test]
fn a_bad_name_refuses_everything_before_any_write() {
    let dir = tempfile::tempdir().unwrap();
    let s = common::synth();
    let a = Archive::open(&s.archive).unwrap();
    let key = Key::from_file(&s.key).unwrap();
    // No extra Python: we prove the global refusal with an out-of-table index (name not found).
    let out = dir.path().join("out");
    let err = extract(&a, &[0, 1, 999], &opts(Some(&out), &key, Verify::Crc, 1)).unwrap_err();
    assert!(matches!(err, P4kError::Refused(999)), "{err:?}");
    assert!(walk(&out).is_empty());
}

/// Archive of two entries in two folders (`A/ok.bin`, `B/blocked.bin`), synth key.
fn two_dirs(dir: &Path) -> (std::path::PathBuf, Key) {
    let archive = dir.join("two.p4k");
    std::fs::copy(common::testdata("blocked.p4k"), &archive).unwrap();
    (archive, Key::from_file(&common::synth().key).unwrap())
}

#[test]
fn a_blocked_directory_only_fails_its_own_entries() {
    let dir = tempfile::tempdir().unwrap();
    let (archive, key) = two_dirs(dir.path());
    let a = Archive::open(&archive).unwrap();
    assert_eq!(a.len(), 2);
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("B"), b"ordinary file").unwrap();
    let r = extract(&a, &[0, 1], &opts(Some(&out), &key, Verify::Crc, 1)).unwrap();
    assert_eq!(r.errors.len(), 1);
    assert_eq!(r.errors[0].0, 1);
    assert!(
        matches!(r.errors[0].1, P4kError::Io(_)),
        "{:?}",
        r.errors[0].1
    );
    assert_eq!(std::fs::read(out.join("A/ok.bin")).unwrap(), b"ok-data");
}

#[test]
fn a_name_equal_to_the_old_tmp_name_of_another_entry_is_extracted_intact() {
    for threads in [1, 8] {
        let dir = tempfile::tempdir().unwrap();
        let archive = common::testdata("stored-tmpnames.p4k");
        let key = Key::from_file(&common::synth().key).unwrap();
        let a = Archive::open(&archive).unwrap();
        let out = dir.path().join("out");
        let all: Vec<usize> = (0..a.len()).collect();
        let r = extract(&a, &all, &opts(Some(&out), &key, Verify::Crc, threads)).unwrap();
        assert!(r.errors.is_empty(), "{threads} threads: {:?}", r.errors);
        for n in 0..40 {
            assert_eq!(
                std::fs::read(out.join(format!("Data/f{n}"))).unwrap(),
                format!("main-{n}-").repeat(50).as_bytes()
            );
            assert_eq!(
                std::fs::read(out.join(format!("Data/f{n}.p4kr-tmp"))).unwrap(),
                format!("tmp-{n}-").repeat(50).as_bytes()
            );
        }
        assert_eq!(walk(&out).len(), 80);
    }
}

#[test]
fn duplicate_targets_fail_per_entry_and_leave_no_temporary_file() {
    for threads in [1, 8] {
        let dir = tempfile::tempdir().unwrap();
        let archive = common::testdata("stored-duplicates.p4k");
        let key = Key::from_file(&common::synth().key).unwrap();
        let a = Archive::open(&archive).unwrap();
        assert_eq!(a.len(), 6);
        let out = dir.path().join("out");
        let all: Vec<usize> = (0..a.len()).collect();
        let r = extract(&a, &all, &opts(Some(&out), &key, Verify::Crc, threads)).unwrap();
        let failed: Vec<(usize, &str)> = r.errors.iter().map(|(i, e)| (*i, e.kind())).collect();
        assert_eq!(
            failed,
            vec![
                (2, "exception:Io"),
                (3, "exception:Io"),
                (5, "exception:Io")
            ],
            "{threads} threads"
        );
        // the first of each group wins, whatever the execution order
        assert_eq!(std::fs::read(out.join("Data/d")).unwrap(), b"first");
        assert_eq!(std::fs::read(out.join("Data/ok")).unwrap(), b"intact");
        let names: Vec<String> = walk(&out)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 3, "{names:?}");
        assert!(
            names.iter().all(|n| !n.starts_with(".p4kr-tmp")),
            "{names:?}"
        );
        let lossy = out.join("Data/\u{fffd}1");
        assert_eq!(std::fs::read(lossy).unwrap(), b"lossy-a");
    }
}

#[test]
fn names_differing_only_by_case_never_overwrite_each_other() {
    let s = common::synth();
    let key = Key::from_file(&s.key).unwrap();
    let a = Archive::open(&common::testdata("case.p4k")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    let all: Vec<usize> = (0..a.len()).collect();
    let r = extract(&a, &all, &opts(Some(&out), &key, Verify::Crc, 8)).unwrap();
    if cfg!(any(windows, target_os = "macos")) {
        // a single file is possible: the first wins, the second is an error
        let failed: Vec<usize> = r.errors.iter().map(|(i, _)| *i).collect();
        assert_eq!(failed, vec![1]);
        assert_eq!(std::fs::read(out.join("Data/Ab")).unwrap(), b"first");
    } else {
        assert!(r.errors.is_empty());
        assert_eq!(std::fs::read(out.join("Data/Ab")).unwrap(), b"first");
        assert_eq!(std::fs::read(out.join("Data/aB")).unwrap(), b"second");
    }
}

#[test]
fn the_local_header_variant_is_extracted_and_an_unknown_signature_fails_alone() {
    let dir = tempfile::tempdir().unwrap();
    let a = Archive::open(&common::testdata("local-variant.p4k")).unwrap();
    let key = Key::from_file(&common::synth().key).unwrap();
    let out = dir.path().join("out");
    let all: Vec<usize> = (0..a.len()).collect();
    let r = extract(&a, &all, &opts(Some(&out), &key, Verify::Full, 0)).unwrap();
    let kinds: Vec<(usize, &str)> = r.errors.iter().map(|(i, e)| (*i, e.kind())).collect();
    assert_eq!(kinds, vec![(6, "local-header")]);
    assert_eq!(walk(&out).len(), 6);
    assert!(!out.join("Data/variant/unknown.bin").exists());
}
