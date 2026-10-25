//! Writing the records of a `.dcb` (DataForge) into a folder: one file per output, via a
//! temporary file `{TMP_PREFIX}d{entry}-{n}` (`.p4kr-tmp:` off Windows, `.p4kr-tmp~` on Windows) then rename, in parallel.

use crate::extract::{TMP_PREFIX, ensure_free, xml_target};
use crate::select::{claim_key, has_tmp_prefix, target};
use crate::{Archive, P4kError, Result};
use p4kr_dataforge::{Db, DcbError, Output, RecordError, safe_relative};
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashSet};
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

/// Summary of one `.dcb`.
#[derive(Debug, Default)]
pub struct DcbReport {
    /// Records written.
    pub records: usize,
    /// Failures: sha256 (hex) of the output path, never the path itself.
    pub failed: Vec<(String, RecordError)>,
    pub bytes: u64,
    /// For `RecordError::Write` failures: the kind of the I/O error (without a path).
    pub io_errors: Vec<(String, io::ErrorKind)>,
    /// Entries displaced in favour of a record: (original path, new path).
    pub displaced: Vec<(PathBuf, PathBuf)>,
}

enum Done {
    Written(u64, Option<(PathBuf, PathBuf)>),
    Skipped,
    Failed(RecordError),
    Io(io::ErrorKind),
}

fn path_digest(path: &str) -> String {
    use std::fmt::Write as _;
    Sha256::digest(path.as_bytes())
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// Safe relative path of an output: that of `safe_relative`, whose last component is not
/// a temporary name (it must never coincide with `TMP_PREFIX…`, i.e. `.p4kr-tmp:…` off Windows and `.p4kr-tmp~…` on Windows).
fn usable_relative(path: &str) -> Option<&Path> {
    let rel = safe_relative(path)?;
    let temp_name = rel.file_name().is_some_and(|n| {
        let n = n.to_string_lossy();
        if cfg!(any(windows, target_os = "macos")) {
            has_tmp_prefix(&n)
        } else {
            n.starts_with(TMP_PREFIX)
        }
    });
    // On Windows, ':' would open an alternate stream and the `.p4kr-tmp~` prefix must not be that of any
    // component (a folder with that name would sit next to the temporaries).
    // Still on Windows: '\\' is a separator there, which `safe_relative` does not see (`a\..\..\x`).
    let windows_unsafe = cfg!(windows)
        && (path.contains('\\')
            || rel.components().any(|c| {
                let c = c.as_os_str().to_string_lossy();
                c.contains(':') || has_tmp_prefix(&c)
            }));
    (!temp_name && !windows_unsafe).then_some(rel)
}

/// Suffix of the name under which a yielded entry is kept next to the record.
const ASIDE_SUFFIX: &str = ".p4k-entry";

/// Fate of a record target: free, refused, or taken from an entry already written, which is
/// then kept under `to`.
#[derive(Debug, PartialEq)]
enum Claim {
    Free,
    Clash,
    Displace { from: PathBuf, to: PathBuf },
}

/// Targets reserved before a `.dcb`: `taken` is refused (raw files of the `.dcb`s, outputs of the
/// previous `.dcb`s), `yielding` (non-`.dcb` entries) gives way to the record. Both
/// sets contain keys (`claim_key`).
pub(crate) struct Reserved<'a> {
    pub taken: &'a HashSet<PathBuf>,
    pub yielding: &'a HashSet<PathBuf>,
}

/// `<name without extension>.p4k-entry.<extension>` in the same folder (`<name>.p4k-entry` without extension).
fn aside_name(path: &Path) -> Option<PathBuf> {
    let stem = path.file_stem()?;
    let mut name = stem.to_os_string();
    name.push(ASIDE_SUFFIX);
    if let Some(ext) = path.extension() {
        name.push(".");
        name.push(ext);
    }
    Some(path.with_file_name(name))
}

fn is_absent(path: &Path) -> bool {
    matches!(std::fs::symlink_metadata(path), Err(e) if e.kind() == io::ErrorKind::NotFound)
}

/// Fate of a yieldable target `real`: free if the file does not exist, displaced if it exists and its
/// side name is free (`reserved` = keys already taken, the disk is also consulted), refused otherwise
/// (folder, side name taken, unreadable disk).
fn yield_claim(real: &Path, reserved: impl Fn(&Path) -> bool) -> Claim {
    match std::fs::symlink_metadata(real) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Claim::Free,
        Ok(m) if m.is_file() => match aside_name(real) {
            Some(to) if !reserved(&claim_key(&to)) && is_absent(&to) => Claim::Displace {
                from: real.to_path_buf(),
                to,
            },
            _ => Claim::Clash,
        },
        _ => Claim::Clash,
    }
}

/// Keys of the parent folders of `rel` under `dir` (nearest first).
fn parent_keys(dir: &Path, rel: &Path) -> Vec<PathBuf> {
    rel.ancestors()
        .skip(1)
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| claim_key(&dir.join(p)))
        .collect()
}

/// `path` names an already planned file, an already planned folder, or has a planned file as a parent.
fn overlaps(
    path: &Path,
    parents: &[PathBuf],
    files: &HashSet<PathBuf>,
    dirs: &HashSet<PathBuf>,
) -> bool {
    files.contains(path) || dirs.contains(path) || parents.iter().any(|p| files.contains(p))
}

/// Fate of each output: refused if its target is already taken by the extraction in progress (`taken`), by
/// a previous output that names the same file (`a//b` and `a/b`), or that makes it a folder (`a`
/// then `a/b.xml`, or the reverse); yielded if it is an entry of `yielding`. The first one wins, whatever
/// the order of the parallel writes.
fn collisions(
    outputs: &[Output],
    dir: &Path,
    taken: &HashSet<PathBuf>,
    yielding: &HashSet<PathBuf>,
) -> Vec<Claim> {
    let records: HashSet<PathBuf> = outputs
        .iter()
        .filter_map(|o| usable_relative(&o.path))
        .map(|rel| claim_key(&dir.join(rel)))
        .collect();
    let mut files: HashSet<PathBuf> = HashSet::new();
    let mut dirs: HashSet<PathBuf> = HashSet::new();
    let mut asides: HashSet<PathBuf> = HashSet::new();
    outputs
        .iter()
        .map(|o| {
            let Some(rel) = usable_relative(&o.path) else {
                return Claim::Free;
            };
            let path = claim_key(&dir.join(rel));
            let parents = parent_keys(dir, rel);
            if taken.contains(&path) || overlaps(&path, &parents, &files, &dirs) {
                return Claim::Clash;
            }
            let claim = if yielding.contains(&path) {
                yield_claim(&dir.join(rel), |k| {
                    taken.contains(k)
                        || yielding.contains(k)
                        || records.contains(k)
                        || asides.contains(k)
                })
            } else {
                Claim::Free
            };
            if let Claim::Displace { to, .. } = &claim {
                asides.insert(claim_key(to));
            }
            if claim != Claim::Clash {
                files.insert(path);
                dirs.extend(parents);
            }
            claim
        })
        .collect()
}

/// Green light for an output.
#[derive(Clone)]
enum Gate {
    Open,
    Clash,
    /// Opened once the record is rendered: the `from` entry is then kept under `to`.
    Displace {
        from: PathBuf,
        to: PathBuf,
    },
}

impl From<Claim> for Gate {
    fn from(claim: Claim) -> Self {
        match claim {
            Claim::Free => Gate::Open,
            Claim::Clash => Gate::Clash,
            Claim::Displace { from, to } => Gate::Displace { from, to },
        }
    }
}

/// Keeps the `from` entry under `to`. Returns `false` if it vanished in the meantime (nothing to keep).
fn set_aside(from: &Path, to: &Path) -> io::Result<bool> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Publishes a record: `stage` writes its temporary file, `aside` keeps the entry aside (returns whether it
/// moved), `place` puts the temporary file at the path. On failure, `cleanup` removes the temporary file and the entry
/// is put back (`undo`) if it had moved: never a displaced entry without a record.
fn publish(
    stage: impl FnOnce() -> io::Result<()>,
    aside: impl FnOnce() -> io::Result<bool>,
    place: impl FnOnce() -> io::Result<()>,
    undo: impl FnOnce(),
    cleanup: impl FnOnce(),
) -> io::Result<bool> {
    if let Err(e) = stage() {
        cleanup();
        return Err(e);
    }
    let moved = match aside() {
        Ok(moved) => moved,
        Err(e) => {
            cleanup();
            return Err(e);
        }
    };
    if let Err(e) = place() {
        cleanup();
        if moved {
            undo();
        }
        return Err(e);
    }
    Ok(moved)
}

fn write_tmp(tmp: &Path, data: &[u8]) -> io::Result<()> {
    // An orphaned temporary file from an interrupted run is replaced.
    let _ = std::fs::remove_file(tmp);
    let mut f = OpenOptions::new().write(true).create_new(true).open(tmp)?;
    f.write_all(data)
}

/// Writes the record at `path` via `tmp`. With `gate` = `Displace`, the entry is kept aside only
/// once the temporary file is written, and put back if the final rename fails. Returns the
/// completed displacement.
fn write_record(
    path: &Path,
    tmp: &Path,
    data: &[u8],
    gate: &Gate,
) -> io::Result<Option<(PathBuf, PathBuf)>> {
    let aside = match gate {
        Gate::Displace { from, to } => Some((from, to)),
        _ => None,
    };
    let moved = publish(
        || {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            write_tmp(tmp, data)
        },
        || aside.map_or(Ok(false), |(from, to)| set_aside(from, to)),
        || std::fs::rename(tmp, path),
        || {
            if let Some((from, to)) = aside {
                let _ = std::fs::rename(to, from);
            }
        },
        || {
            let _ = std::fs::remove_file(tmp);
        },
    )?;
    Ok(aside.filter(|_| moved).map(|(f, t)| (f.clone(), t.clone())))
}

/// An output to write: its place in the list, its original `.dcb` (temporary file number) and the green
/// light of its target.
struct Slot<'a> {
    n: usize,
    entry: usize,
    gate: &'a Gate,
}

fn write_one(db: &Db, o: &Output, dir: &Path, slot: &Slot) -> Done {
    let Some(rel) = usable_relative(&o.path) else {
        return Done::Failed(RecordError::UnsafePath);
    };
    if matches!(slot.gate, Gate::Clash) {
        return Done::Failed(RecordError::Collision);
    }
    let data = match db.render(o) {
        Ok(Some(data)) => data,
        Ok(None) => return Done::Skipped,
        Err(e) => return Done::Failed(e),
    };
    let path = dir.join(rel);
    let tmp = path.with_file_name(format!("{TMP_PREFIX}d{}-{}", slot.entry, slot.n));
    // The entry is kept aside only once a record is rendered and its temporary file written (see `publish`).
    match write_record(&path, &tmp, &data, slot.gate) {
        Ok(moved) => Done::Written(data.len() as u64, moved),
        Err(e) => Done::Io(e.kind()),
    }
}

/// Removes the orphaned temporary files of `entry` left by an interrupted run in each
/// folder where this run will write.
fn sweep_orphans(outputs: &[Output], dir: &Path, entry: usize) {
    let prefix = format!("{TMP_PREFIX}d{entry}-");
    let dirs: BTreeSet<PathBuf> = outputs
        .iter()
        .filter_map(|o| usable_relative(&o.path))
        .filter_map(|rel| dir.join(rel).parent().map(Path::to_path_buf))
        .chain([dir.to_path_buf()])
        .collect();
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with(&prefix) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

fn other(msg: String) -> P4kError {
    P4kError::Io(io::Error::other(msg))
}

/// Writes `outputs` (from `db.outputs()`) under `dir`, in parallel. `entry` numbers the temporary files (two
/// `.dcb`s in the same folder never write the same one). Returns the summary and the written paths. Error
/// only if the write cannot start; a failed record does not stop the others.
/// The `reserved.yielding` entries targeted by a record are first displaced (see `Claim`).
pub(crate) fn write_job(
    db: &Db<'_>,
    outputs: &[Output],
    dir: &Path,
    reserved: &Reserved<'_>,
    threads: usize,
    entry: usize,
    progress: Option<&AtomicUsize>,
) -> Result<(DcbReport, Vec<PathBuf>)> {
    std::fs::create_dir_all(dir)?;
    ensure_free(dir)?;
    sweep_orphans(outputs, dir, entry);
    // `collisions` compares keys: we key the sets once (case folding is idempotent).
    let key =
        |set: &HashSet<PathBuf>| -> HashSet<PathBuf> { set.iter().map(|p| claim_key(p)).collect() };
    let claims = collisions(outputs, dir, &key(reserved.taken), &key(reserved.yielding));
    let gates: Vec<Gate> = claims.into_iter().map(Gate::from).collect();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .map_err(|e| other(e.to_string()))?;
    let results: Vec<Done> = pool.install(|| {
        outputs
            .par_iter()
            .zip(gates.par_iter())
            .enumerate()
            .map(|(n, (o, gate))| {
                let slot = Slot { n, entry, gate };
                let done = write_one(db, o, dir, &slot);
                if let Some(p) = progress {
                    p.fetch_add(1, Relaxed);
                }
                done
            })
            .collect()
    });
    Ok(tally(outputs, dir, results))
}

/// Summary and paths taken (written, plus the side names of displaced entries).
fn tally(outputs: &[Output], dir: &Path, results: Vec<Done>) -> (DcbReport, Vec<PathBuf>) {
    let mut written = Vec::new();
    let mut report = DcbReport::default();
    for (o, done) in outputs.iter().zip(results) {
        match done {
            Done::Written(n, moved) => {
                report.records += 1;
                report.bytes += n;
                if let Some((from, to)) = moved {
                    written.push(to.clone());
                    report.displaced.push((from, to));
                }
                written.extend(usable_relative(&o.path).map(|rel| dir.join(rel)));
            }
            Done::Skipped => {}
            Done::Failed(e) => report.failed.push((path_digest(&o.path), e)),
            Done::Io(kind) => {
                let digest = path_digest(&o.path);
                report.io_errors.push((digest.clone(), kind));
                report.failed.push((digest, RecordError::Write));
            }
        }
    }
    (report, written)
}

/// Writes `outputs` (computed once by the caller with `db.outputs()`) under `dir`. `taken` contains
/// ordinary paths (the case key is computed on entry to `write_job`); no entry is
/// ever displaced.
pub fn write_outputs(
    db: &Db<'_>,
    outputs: &[Output],
    dir: &Path,
    taken: &HashSet<PathBuf>,
    threads: usize,
    progress: Option<&AtomicUsize>,
) -> Result<DcbReport> {
    let reserved = Reserved {
        taken,
        yielding: &HashSet::new(),
    };
    write_job(db, outputs, dir, &reserved, threads, 0, progress).map(|(r, _)| r)
}

/// Writes the records of `db` under `dir`.
pub fn write_records(
    db: &Db<'_>,
    dir: &Path,
    taken: &HashSet<PathBuf>,
    threads: usize,
    progress: Option<&AtomicUsize>,
) -> Result<DcbReport> {
    write_outputs(db, &db.outputs(), dir, taken, threads, progress)
}

/// Targets of the extraction in progress: raw files of the entries (excluding folders) and side XML of the
/// converted `sides` entries.
pub(crate) fn taken_targets(
    a: &Archive,
    root: &Path,
    order: &[usize],
    sides: &[usize],
) -> HashSet<PathBuf> {
    let raw = order.iter().filter_map(|&i| {
        let name = a.name(i).filter(|n| !n.ends_with(['/', '\\']))?;
        target(root, name)
    });
    let side = sides
        .iter()
        .filter_map(|&i| a.name(i).and_then(|n| target(root, n)))
        .map(|p| xml_target(&p));
    raw.chain(side).map(|p| claim_key(&p)).collect()
}

/// DataForge phase: converts each `.dcb` already extracted (by increasing index), under the folder of
/// its target. Each `.dcb` is re-read from its target, one at a time in memory. `taken` (targets of
/// the extraction) is shared: raw `.dcb`s stay refused, other entries give way to a
/// record (see `Claim`). Paths taken by a `.dcb` are refused to the following ones. A refusal to
/// start writing is returned in `errors`.
pub(crate) fn convert_all(
    a: &Archive,
    root: &Path,
    dcbs: &[usize],
    taken: &HashSet<PathBuf>,
    threads: usize,
    records: Option<&crate::extract::RecordProgress>,
    errors: &mut Vec<(usize, P4kError)>,
) -> Vec<(usize, std::result::Result<DcbReport, DcbError>)> {
    let mut sorted = dcbs.to_vec();
    sorted.sort_unstable();
    let mut hard: HashSet<PathBuf> = dcbs
        .iter()
        .filter_map(|&i| a.name(i).and_then(|n| target(root, n)))
        .map(|p| claim_key(&p))
        .collect();
    let yielding: HashSet<PathBuf> = taken.difference(&hard).cloned().collect();
    let mut done = Vec::new();
    for i in sorted {
        let reserved = Reserved {
            taken: &hard,
            yielding: &yielding,
        };
        match convert_one(a, root, i, &reserved, threads, records) {
            Ok(Ok((r, written))) => {
                hard.extend(written.iter().map(|p| claim_key(p)));
                done.push((i, Ok(r)));
            }
            Ok(Err(e)) => done.push((i, Err(e))),
            Err(e) => errors.push((i, e)),
        }
    }
    done
}

type Converted = std::result::Result<(DcbReport, Vec<PathBuf>), DcbError>;

/// One `.dcb`: re-read from its target, opened, written; its bytes are freed on return.
fn convert_one(
    a: &Archive,
    root: &Path,
    i: usize,
    reserved: &Reserved<'_>,
    threads: usize,
    records: Option<&crate::extract::RecordProgress>,
) -> Result<Converted> {
    let path = a
        .name(i)
        .and_then(|n| target(root, n))
        .ok_or(P4kError::Refused(i))?;
    let dir = path.parent().ok_or(P4kError::Refused(i))?;
    let bytes = std::fs::read(&path)?;
    let db = match Db::open(&bytes) {
        Ok(db) => db,
        Err(e) => return Ok(Err(e)),
    };
    let outputs = db.outputs();
    if let Some(r) = records {
        r.total.fetch_add(outputs.len(), Relaxed);
    }
    write_job(
        &db,
        &outputs,
        dir,
        reserved,
        threads,
        i,
        records.map(|r| &r.done),
    )
    .map(Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporary_names_are_not_usable_record_paths() {
        assert!(usable_relative("lib0/a.xml").is_some());
        assert!(usable_relative("lib0/.p4kr-tmp:d0-1").is_none());
        assert!(usable_relative(".p4kr-tmp:x").is_none());
        assert!(usable_relative("../a.xml").is_none());
        assert_eq!(
            usable_relative("x/.p4kr-tmp:y/z.xml").is_some(),
            !cfg!(windows)
        );
        let upper = usable_relative("x/.P4KR-TMP~y").is_some();
        assert_eq!(upper, !cfg!(windows));
        assert_eq!(
            usable_relative("x/.P4KR-TMP:y").is_some(),
            !cfg!(any(windows, target_os = "macos"))
        );
        assert_eq!(usable_relative("a:b/c.xml").is_some(), !cfg!(windows));
    }

    fn out(p: &str, record: u32) -> Output {
        Output {
            path: p.into(),
            record,
        }
    }

    /// Binary verdict of `collisions` without a yieldable entry.
    fn clashes(outputs: &[Output], dir: &Path, taken: &HashSet<PathBuf>) -> Vec<bool> {
        collisions(outputs, dir, taken, &HashSet::new())
            .iter()
            .map(|c| matches!(c, Claim::Clash))
            .collect()
    }

    #[test]
    fn collisions_fold_case_only_where_the_disk_does() {
        let fold = cfg!(any(windows, target_os = "macos"));
        let none = HashSet::new();
        assert_eq!(
            clashes(
                &[out("a/B.xml", 0), out("A/b.xml", 1)],
                Path::new("d"),
                &none
            ),
            [false, fold]
        );
        assert_eq!(
            clashes(&[out("a", 0), out("A/x.xml", 1)], Path::new("d"), &none),
            [false, fold]
        );
        let taken: HashSet<PathBuf> = [claim_key(Path::new("d/C.xml"))].into();
        assert_eq!(clashes(&[out("c.xml", 0)], Path::new("d"), &taken), [fold]);
    }

    #[test]
    fn same_file_spelled_twice_collides_and_first_wins() {
        let o = |p: &str, record| Output {
            path: p.into(),
            record,
        };
        let outs = [o("a/b.xml", 0), o("a//b.xml", 1), o("c.xml", 2)];
        let taken: HashSet<PathBuf> = [PathBuf::from("d/c.xml")].into();
        assert_eq!(clashes(&outs, Path::new("d"), &taken), [false, true, true]);
    }

    #[test]
    fn a_file_and_a_directory_of_the_same_name_collide_and_first_wins() {
        let o = |p: &str, record| Output {
            path: p.into(),
            record,
        };
        let outs = [
            o("a", 0),
            o("a/b.xml", 1),
            o("c/d/e.xml", 2),
            o("c/d", 3),
            o("c", 4),
            o("c/f.xml", 5),
        ];
        assert_eq!(
            clashes(&outs, Path::new("d"), &HashSet::new()),
            [false, true, false, true, true, false]
        );
    }

    #[test]
    fn written_paths_of_one_dcb_are_taken_by_the_next() {
        let mut taken = HashSet::new();
        taken.extend([PathBuf::from("d/x.xml")]);
        let outs = [Output {
            path: "x.xml".into(),
            record: 0,
        }];
        assert_eq!(clashes(&outs, Path::new("d"), &taken), [true]);
    }

    #[test]
    fn the_aside_name_goes_before_the_last_extension() {
        let aside = |p: &str| aside_name(Path::new(p)).unwrap();
        assert_eq!(aside("d/a.xml"), Path::new("d/a.p4k-entry.xml"));
        assert_eq!(aside("d/noext"), Path::new("d/noext.p4k-entry"));
        assert_eq!(aside("d/a.b.c.xml"), Path::new("d/a.b.c.p4k-entry.xml"));
        assert_eq!(aside("d/.hidden"), Path::new("d/.hidden.p4k-entry"));
        assert!(aside_name(Path::new("")).is_none());
    }

    fn plan(yielding: &[&Path], outs: &[Output], dir: &Path) -> Vec<Claim> {
        let soft: HashSet<PathBuf> = yielding.iter().map(|p| claim_key(p)).collect();
        collisions(outs, dir, &HashSet::new(), &soft)
    }

    #[test]
    fn a_yielding_entry_on_disk_is_displaced_and_an_absent_one_is_not() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        std::fs::write(d.join("a.xml"), b"old").unwrap();
        let outs = [out("a.xml", 0), out("gone.xml", 1)];
        let claims = plan(&[&d.join("a.xml"), &d.join("gone.xml")], &outs, d);
        assert_eq!(
            claims,
            [
                Claim::Displace {
                    from: d.join("a.xml"),
                    to: d.join("a.p4k-entry.xml")
                },
                Claim::Free
            ]
        );
    }

    #[test]
    fn a_taken_aside_name_keeps_the_entry_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        for n in ["a.xml", "b.xml", "c.xml", "e.xml", "e.p4k-entry.xml"] {
            std::fs::write(d.join(n), b"old").unwrap();
        }
        let ys: Vec<PathBuf> = ["a", "b", "c", "e"]
            .iter()
            .map(|n| d.join(format!("{n}.xml")))
            .chain([d.join("b.p4k-entry.xml")])
            .collect();
        let yielding: Vec<&Path> = ys.iter().map(PathBuf::as_path).collect();
        let outs = [
            out("a.xml", 0),
            out("a.p4k-entry.xml", 1), // taken by a record
            out("b.xml", 2),           // taken by another entry
            out("c.xml", 3),
            out("e.xml", 4), // already exists on disk
        ];
        let kinds: Vec<bool> = plan(&yielding, &outs, d)
            .iter()
            .map(|c| matches!(c, Claim::Displace { .. }))
            .collect();
        assert_eq!(kinds, [false, false, false, true, false]);
    }

    #[test]
    fn a_hard_target_is_never_displaced_and_a_directory_is_not_an_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        std::fs::create_dir(d.join("dir.xml")).unwrap();
        let taken: HashSet<PathBuf> = [claim_key(&d.join("b.dcb"))].into();
        let soft: HashSet<PathBuf> = [claim_key(&d.join("dir.xml"))].into();
        let claims = collisions(&[out("b.dcb", 0), out("dir.xml", 1)], d, &taken, &soft);
        assert_eq!(claims, [Claim::Clash, Claim::Clash]);
    }

    use std::cell::RefCell;

    /// Plays `publish` with steps that note their passing; `fail` names the step that fails.
    fn run_publish(fail: &str, moves: bool) -> (io::Result<bool>, Vec<&'static str>) {
        let log = RefCell::new(Vec::new());
        let step = |name: &'static str| -> io::Result<()> {
            log.borrow_mut().push(name);
            if name == fail {
                Err(io::Error::other(name))
            } else {
                Ok(())
            }
        };
        let r = publish(
            || step("stage"),
            || step("aside").map(|()| moves),
            || step("place"),
            || {
                log.borrow_mut().push("undo");
            },
            || {
                log.borrow_mut().push("cleanup");
            },
        );
        (r, log.into_inner())
    }

    #[test]
    fn publish_stages_then_sets_the_entry_aside_then_places() {
        let (r, log) = run_publish("", true);
        assert!(r.unwrap());
        assert_eq!(log, ["stage", "aside", "place"]);
    }

    #[test]
    fn a_failed_staging_or_aside_leaves_the_entry_and_cleans_up() {
        let (r, log) = run_publish("stage", true);
        assert!(r.is_err());
        assert_eq!(log, ["stage", "cleanup"]);
        let (r, log) = run_publish("aside", true);
        assert!(r.is_err());
        assert_eq!(log, ["stage", "aside", "cleanup"]);
    }

    #[test]
    fn a_failed_placing_restores_the_entry_only_if_it_was_moved() {
        let (r, log) = run_publish("place", true);
        assert!(r.is_err());
        assert_eq!(log, ["stage", "aside", "place", "cleanup", "undo"]);
        let (r, log) = run_publish("place", false);
        assert!(r.is_err());
        assert_eq!(log, ["stage", "aside", "place", "cleanup"]);
    }
}
