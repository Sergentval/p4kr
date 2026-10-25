//! Parallel extraction: reads sorted by offset, one descriptor per thread (positional read),
//! temporary file then rename. Without an output folder: read in memory only.

use crate::dcb::{self, DcbReport};
use crate::select::{claim_key, target};
use crate::{Archive, Key, P4kError, Result, Verify};
use p4kr_cryxml::{Outcome, convert, is_dataforge};
use p4kr_dataforge::DcbError;
use rayon::prelude::*;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed};

pub const MIN_FREE: u64 = 20 << 30;
/// Prefix of the temporary files. Off Windows, `target()` refuses ':' in a name, so no entry
/// can bear this name. On Windows, `:` would name an NTFS alternate data stream: the prefix
/// is `.p4kr-tmp~`, and `target()` as well as the DataForge outputs refuse any component that bears it
/// (ignoring case) or that contains ':'. The suffix is the entry index: two
/// entries never write the same temporary file.
#[cfg(not(windows))]
pub(crate) const TMP_PREFIX: &str = ".p4kr-tmp:";
#[cfg(windows)]
pub(crate) const TMP_PREFIX: &str = ".p4kr-tmp~";

pub struct Options<'a> {
    pub out: Option<&'a Path>,
    pub key: Option<&'a Key>,
    pub verify: Verify,
    pub threads: usize,
    pub fail_fast: bool,
}

/// Extraction options added in block 1g-a.
#[derive(Default)]
pub struct Extras<'a> {
    /// Converts CryXmlB to text XML like unp4k + unforge (`p4kr_cryxml`).
    pub xml: bool,
    /// Entries visited (succeeded or not, including those skipped after `fail_fast`), for a progress bar.
    pub progress: Option<&'a AtomicUsize>,
    /// DataForge phase (after the entries): records to write and already written, for a second bar.
    pub records: Option<&'a RecordProgress>,
}

/// Counters of the DataForge phase. `total` grows by the number of records of each `.dcb` just
/// before it is written: it is 0 until the phase has started.
#[derive(Default, Debug)]
pub struct RecordProgress {
    pub total: AtomicUsize,
    pub done: AtomicUsize,
}

#[derive(Debug)]
pub struct Report {
    pub entries: usize,
    pub bytes: u64,
    pub threads: usize,
    pub errors: Vec<(usize, P4kError)>,
    /// Entries converted to XML.
    pub converted: usize,
    /// Failed conversions: raw kept, or partial XML written, like the reference.
    pub xml_failed: Vec<usize>,
    /// One `.dcb` per entry converted with `xml`: its summary, or the database's refusal.
    pub dcb: Vec<(usize, std::result::Result<DcbReport, DcbError>)>,
}

thread_local! {
    static FILE: RefCell<Option<(PathBuf, File)>> = const { RefCell::new(None) };
}

fn other(msg: String) -> P4kError {
    P4kError::Io(io::Error::other(msg))
}

/// Bytes available to an unprivileged user on the volume of `dir`.
#[cfg(unix)]
pub fn free_bytes(dir: &Path) -> Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes())
        .map_err(|_| other(format!("path with a NUL byte: {}", dir.display())))?;
    let mut s = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY: `c` is a valid NUL-terminated C string; `s` is a buffer of the right size that
    // `statvfs` fills entirely when it returns 0, the only case where we read it.
    let rc = unsafe { libc::statvfs(c.as_ptr(), s.as_mut_ptr()) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: rc == 0, the structure is initialized.
    let s = unsafe { s.assume_init() };
    #[allow(clippy::unnecessary_cast)] // field types differ between Linux and macOS
    Ok(s.f_bavail as u64 * s.f_frsize as u64)
}

#[cfg(windows)]
pub fn free_bytes(dir: &Path) -> Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
    let mut avail = 0u64;
    // SAFETY: `wide` is 0-terminated; the output pointers are valid, the other two are null
    // (allowed by the API).
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut avail,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(avail)
}

/// Threshold in bytes for a `P4KR_MIN_FREE_GB` value: integer in GiB, otherwise 20 GiB.
pub fn parse_min_free(value: Option<&str>) -> u64 {
    value
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map_or(MIN_FREE, |g| g.saturating_mul(1 << 30))
}

/// Free-space threshold before any write: 20 GiB, or `P4KR_MIN_FREE_GB` (integer, in GiB) if readable.
pub fn min_free() -> u64 {
    parse_min_free(std::env::var("P4KR_MIN_FREE_GB").ok().as_deref())
}

fn read_one(a: &Archive, i: usize, opts: &Options) -> Result<Vec<u8>> {
    if a.path().as_os_str().is_empty() {
        return a.read(i, opts.key, opts.verify);
    }
    FILE.with(|cell| {
        let mut slot = cell.borrow_mut();
        if !matches!(&*slot, Some((p, _)) if p == a.path()) {
            *slot = Some((a.path().to_path_buf(), File::open(a.path())?));
        }
        let Some((_, file)) = &*slot else {
            return Err(other("descriptor missing".into()));
        };
        a.read_at(file, i, opts.key, opts.verify)
    })
}

fn is_dir_name(name: &str) -> bool {
    name.ends_with('/') || name.ends_with('\\')
}

fn write_tmp(tmp: &Path, data: &[u8]) -> io::Result<()> {
    use io::Write;
    let mut f = OpenOptions::new().write(true).create_new(true).open(tmp)?;
    f.write_all(data)
}

/// `Path.ChangeExtension(path, "xml")`: extension replaced from the last dot of the name, or added.
pub fn xml_target(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = name.rfind('.').map_or(name.as_str(), |i| &name[..i]);
    path.with_file_name(format!("{stem}.xml"))
}

/// What is reported back from an entry in the report.
enum Issue {
    Error(P4kError),
    XmlFailed,
}

/// What an entry produced.
struct Done {
    xml: Option<Outcome>,
    /// Side XML not written: its target is claimed by another entry.
    conflict: bool,
    /// `.dcb` written raw, to be converted by the DataForge phase (which re-reads it from its target).
    dcb: bool,
}

impl Done {
    fn of(xml: Option<Outcome>) -> Self {
        Self {
            xml,
            conflict: false,
            dcb: false,
        }
    }
    const CONFLICT: Self = Self {
        xml: None,
        conflict: true,
        dcb: false,
    };
}

/// Writes then counts the bytes: `bytes` stays exact even if a later write of the entry fails.
fn write_file(path: &Path, data: &[u8], i: usize, tag: &str, bytes: &AtomicU64) -> Result<()> {
    let tmp = path.with_file_name(format!("{TMP_PREFIX}{i}{tag}"));
    let written = write_tmp(&tmp, data).and_then(|()| std::fs::rename(&tmp, path));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    bytes.fetch_add(data.len() as u64, Relaxed);
    Ok(())
}

fn refuse_duplicate(dups: &HashSet<usize>, i: usize) -> Result<()> {
    if dups.contains(&i) {
        return Err(other(format!(
            "target already taken by another entry (index {i})"
        )));
    }
    Ok(())
}

fn extract_one(
    a: &Archive,
    i: usize,
    opts: &Options,
    extras: &Extras,
    dups: &HashSet<usize>,
    conflicts: &HashSet<usize>,
    bytes: &AtomicU64,
) -> Result<Done> {
    refuse_duplicate(dups, i)?;
    let data = read_one(a, i, opts)?;
    let name = a.name(i).ok_or(P4kError::Refused(i))?;
    let path = opts
        .out
        .map(|root| target(root, name).ok_or(P4kError::Refused(i)))
        .transpose()?;
    let conv = (extras.xml && !is_dir_name(name) && !is_dataforge(name)).then(|| convert(&data));
    let outcome = conv.as_ref().map(|c| c.outcome);
    let xml = conv.as_ref().and_then(|c| c.xml.as_deref());
    let Some(path) = path else {
        let n = xml.map_or(data.len(), <[u8]>::len) as u64;
        bytes.fetch_add(n, Relaxed);
        return Ok(Done::of(outcome));
    };
    if is_dir_name(name) {
        std::fs::create_dir_all(&path)?;
        return Ok(Done::of(None));
    }
    let Some(xml) = xml else {
        write_file(&path, &data, i, "", bytes)?;
        return Ok(Done {
            dcb: extras.xml && is_dataforge(name),
            ..Done::of(outcome)
        });
    };
    let side = xml_target(&path);
    if side == path {
        write_file(&path, xml, i, "", bytes)?;
        return Ok(Done::of(outcome));
    }
    write_file(&path, &data, i, "", bytes)?;
    if conflicts.contains(&i) {
        return Ok(Done::CONFLICT);
    }
    write_file(&side, xml, i, "x", bytes)?;
    Ok(Done::of(outcome))
}

/// Creates the parent folders once. Best effort: a folder that cannot be created does not stop the rest,
/// the entries that depend on it fail individually on write (`Io` error in the report).
fn make_dirs(a: &Archive, root: &Path, order: &[usize]) {
    let dirs: BTreeSet<PathBuf> = order
        .iter()
        .filter_map(|&i| a.name(i).and_then(|n| target(root, n)))
        .filter_map(|p| p.parent().map(Path::to_path_buf))
        .collect();
    for d in dirs {
        let _ = std::fs::create_dir_all(&d);
    }
}

/// Entries whose target is already that of an entry with a smaller index (duplicate name, `a/b` versus `a\\b`,
/// or distinct raw names that `from_utf8_lossy` folds together). The first one wins, the others fail alone.
/// Folder entries are excluded: creating a folder twice has no effect.
fn duplicate_targets(a: &Archive, root: &Path, indices: &[usize]) -> HashSet<usize> {
    let mut sorted = indices.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut seen = HashSet::new();
    let mut dups = HashSet::new();
    for i in sorted {
        let Some(name) = a.name(i).filter(|n| !is_dir_name(n)) else {
            continue;
        };
        if let Some(path) = target(root, name)
            && !seen.insert(claim_key(&path))
        {
            dups.insert(i);
        }
    }
    dups
}

/// Entries whose side XML would target a path claimed twice (raw target of an entry or side XML
/// of another one). Only their conversion is prevented; their raw file is written.
fn side_conflicts(a: &Archive, root: &Path, indices: &[usize]) -> HashSet<usize> {
    let mut claims: HashMap<PathBuf, usize> = HashMap::new();
    let mut sides = Vec::new();
    for &i in indices {
        let Some(name) = a.name(i).filter(|n| !is_dir_name(n)) else {
            continue;
        };
        let Some(path) = target(root, name) else {
            continue;
        };
        let side = xml_target(&path);
        *claims.entry(claim_key(&path)).or_default() += 1;
        if side != path && !is_dataforge(name) {
            *claims.entry(claim_key(&side)).or_default() += 1;
            sides.push((i, side));
        }
    }
    sides
        .into_iter()
        .filter(|(_, s)| claims[&claim_key(s)] > 1)
        .map(|(i, _)| i)
        .collect()
}

/// Counters shared between the workers.
#[derive(Default)]
struct Shared {
    bytes: AtomicU64,
    converted: AtomicUsize,
    /// Raised at the first failure under `fail_fast`: the following entries are skipped.
    stop: AtomicBool,
    /// Indices of the `.dcb`s extracted raw, to convert after the extraction (re-read one by one from their target).
    dcbs: Mutex<Vec<usize>>,
    /// Entries whose side XML was produced: their target is taken for the DataForge phase.
    sides: Mutex<Vec<usize>>,
}

/// Verdict of a finished entry; files away what the DataForge phase claims.
fn settle(i: usize, done: Done, state: &Shared) -> Option<Issue> {
    if done.conflict {
        return Some(Issue::Error(other(format!(
            "side XML target already taken by another entry (index {i})"
        ))));
    }
    if done.dcb
        && let Ok(mut dcbs) = state.dcbs.lock()
    {
        dcbs.push(i);
    }
    match done.xml {
        Some(Outcome::Converted) => {
            state.converted.fetch_add(1, Relaxed);
        }
        Some(Outcome::Failed) => {}
        _ => return None,
    }
    if let Ok(mut sides) = state.sides.lock() {
        sides.push(i);
    }
    matches!(done.xml, Some(Outcome::Failed)).then_some(Issue::XmlFailed)
}

/// Extracts `order` in parallel (in the current pool) and returns the problems per entry.
fn run_entries(
    a: &Archive,
    order: &[usize],
    opts: &Options,
    extras: &Extras,
    dups: &HashSet<usize>,
    conflicts: &HashSet<usize>,
    state: &Shared,
) -> Vec<(usize, Issue)> {
    order
        .par_iter()
        .filter_map(|&i| {
            let skipped = state.stop.load(Relaxed);
            if let Some(p) = extras.progress {
                p.fetch_add(1, Relaxed);
            }
            if skipped {
                return None;
            }
            let fail = |e: P4kError| {
                if opts.fail_fast {
                    state.stop.store(true, Relaxed);
                }
                Some((i, Issue::Error(e)))
            };
            match extract_one(a, i, opts, extras, dups, conflicts, &state.bytes) {
                Ok(done) => match settle(i, done, state) {
                    Some(Issue::Error(e)) => fail(e),
                    other => other.map(|issue| (i, issue)),
                },
                Err(e) => fail(e),
            }
        })
        .collect()
}

pub fn extract(a: &Archive, indices: &[usize], opts: &Options) -> Result<Report> {
    extract_with(a, indices, opts, &Extras::default())
}

/// Refuses if the volume of `dir` has less free space than the threshold (`min_free`).
pub(crate) fn ensure_free(dir: &Path) -> Result<()> {
    let free = free_bytes(dir)?;
    let need = min_free();
    if free < need {
        return Err(other(format!(
            "{} GiB free, minimum {} GiB (P4KR_MIN_FREE_GB)",
            free >> 30,
            need >> 30
        )));
    }
    Ok(())
}

/// Refusal before any write: insufficient free space, or an entry name that would escape the folder.
fn preflight(a: &Archive, indices: &[usize], root: &Path) -> Result<()> {
    std::fs::create_dir_all(root)?;
    ensure_free(root)?;
    match indices
        .iter()
        .find(|&&i| a.name(i).and_then(|n| target(root, n)).is_none())
    {
        Some(&bad) => Err(P4kError::Refused(bad)),
        None => Ok(()),
    }
}

/// A repeated index would share its temporary file: a single extraction per index, in the order
/// of the archive offsets.
fn extraction_order(a: &Archive, indices: &[usize]) -> Vec<usize> {
    let mut order = indices.to_vec();
    order.sort_unstable();
    order.dedup();
    order.sort_by_key(|&i| a.entry(i).map_or(u64::MAX, |e| e.local_offset));
    order
}

/// Entries whose side XML would target an already claimed path (only with `--xml`).
fn xml_conflicts(
    a: &Archive,
    opts: &Options,
    extras: &Extras,
    order: &[usize],
    dups: &HashSet<usize>,
) -> HashSet<usize> {
    match opts.out {
        Some(root) if extras.xml => {
            let live: Vec<usize> = order
                .iter()
                .copied()
                .filter(|i| !dups.contains(i))
                .collect();
            side_conflicts(a, root, &live)
        }
        _ => HashSet::new(),
    }
}

pub fn extract_with(
    a: &Archive,
    indices: &[usize],
    opts: &Options,
    extras: &Extras,
) -> Result<Report> {
    if let Some(root) = opts.out {
        preflight(a, indices, root)?;
    }
    let dups = opts
        .out
        .map_or_else(HashSet::new, |root| duplicate_targets(a, root, indices));
    let order = extraction_order(a, indices);
    if let Some(root) = opts.out {
        make_dirs(a, root, &order);
    }
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(opts.threads)
        .build()
        .map_err(|e| other(e.to_string()))?;
    let conflicts = xml_conflicts(a, opts, extras, &order, &dups);
    let state = Shared::default();
    let results = pool.install(|| run_entries(a, &order, opts, extras, &dups, &conflicts, &state));
    let (mut errors, xml_failed) = split_issues(results);
    let dcb = match opts.out {
        Some(root) if extras.xml && !(opts.fail_fast && !errors.is_empty()) => {
            let (dcbs, sides) = (take(&state.dcbs), take(&state.sides));
            let taken = dcb::taken_targets(a, root, &order, &sides);
            dcb::convert_all(
                a,
                root,
                &dcbs,
                &taken,
                opts.threads,
                extras.records,
                &mut errors,
            )
        }
        _ => Vec::new(),
    };
    errors.sort_by_key(|(i, _)| *i);
    Ok(Report {
        entries: order.len(),
        bytes: state.bytes.into_inner(),
        threads: pool.current_num_threads(),
        errors,
        converted: state.converted.into_inner(),
        xml_failed,
        dcb,
    })
}

fn take<T: Default>(m: &Mutex<T>) -> T {
    m.lock()
        .map(|mut g| std::mem::take(&mut *g))
        .unwrap_or_default()
}

/// Splits the errors from the failed conversions; both lists are sorted by index.
fn split_issues(results: Vec<(usize, Issue)>) -> (Vec<(usize, P4kError)>, Vec<usize>) {
    let mut errors = Vec::new();
    let mut xml_failed = Vec::new();
    for (i, issue) in results {
        match issue {
            Issue::Error(e) => errors.push((i, e)),
            Issue::XmlFailed => xml_failed.push(i),
        }
    }
    errors.sort_by_key(|(i, _)| *i);
    xml_failed.sort_unstable();
    (errors, xml_failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_xml_claims_fold_case_only_where_the_disk_does() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/case-sides.p4k");
        let a = Archive::open(&path).unwrap();
        let conflicts = side_conflicts(&a, Path::new("out"), &[0, 1]);
        let expected: HashSet<usize> = if cfg!(any(windows, target_os = "macos")) {
            [0, 1].into()
        } else {
            HashSet::new()
        };
        assert_eq!(conflicts, expected);
    }
}
