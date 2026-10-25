//! Selection filters (aligned with bench 1c) and safe output path.

use crate::Archive;
use crate::extract::TMP_PREFIX;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Filter {
    All,
    Exact(String),
    Prefix(String),
    Ext(String),
    /// At least one of the filters (at least two: see [`Filter::any`]).
    Any(Vec<Filter>),
    /// All the filters (at least two: see [`Filter::every`]).
    Every(Vec<Filter>),
}

fn sep(b: u8) -> u8 {
    if b == b'\\' { b'/' } else { b }
}

fn norm(s: &str) -> String {
    s.replace('\\', "/")
}

impl Filter {
    pub fn exact(path: &str) -> Self {
        Filter::Exact(norm(path))
    }
    pub fn prefix(dir: &str) -> Self {
        Filter::Prefix(format!("{}/", norm(dir).trim_end_matches('/')))
    }
    pub fn ext(ext: &str) -> Self {
        Filter::Ext(norm(ext).to_ascii_lowercase())
    }
    /// OR of the filters; no filter = everything (option absent), a single one = itself.
    pub fn any(filters: Vec<Filter>) -> Self {
        if filters.contains(&Filter::All) {
            return Filter::All;
        }
        Self::collapse(filters, Filter::Any)
    }
    /// AND of the filters; `All` is neutral, no filter = everything, a single one = itself.
    pub fn every(filters: Vec<Filter>) -> Self {
        let kept = filters.into_iter().filter(|f| *f != Filter::All).collect();
        Self::collapse(kept, Filter::Every)
    }
    fn collapse(mut filters: Vec<Filter>, many: fn(Vec<Filter>) -> Filter) -> Self {
        filters.dedup();
        match filters.len() {
            0 => Filter::All,
            1 => filters.pop().unwrap_or(Filter::All),
            _ => many(filters),
        }
    }

    pub fn matches(&self, name: &str) -> bool {
        let n = name.as_bytes();
        match self {
            Filter::All => true,
            Filter::Exact(p) => {
                n.len() == p.len() && n.iter().zip(p.bytes()).all(|(&a, b)| sep(a) == b)
            }
            Filter::Prefix(d) => {
                n.len() >= d.len() && n.iter().zip(d.bytes()).all(|(&a, b)| sep(a) == b)
            }
            Filter::Ext(e) => {
                n.len() >= e.len()
                    && n[n.len() - e.len()..]
                        .iter()
                        .zip(e.bytes())
                        .all(|(&a, b)| sep(a).to_ascii_lowercase() == b)
            }
            Filter::Any(fs) => fs.iter().any(|f| f.matches(name)),
            Filter::Every(fs) => fs.iter().all(|f| f.matches(name)),
        }
    }
}

impl Archive {
    pub fn select(&self, f: &Filter) -> Vec<usize> {
        self.entries()
            .filter(|e| f.matches(e.name))
            .map(|e| e.index)
            .collect()
    }
}

/// Key of an output path, to detect two outputs that name the same file. Windows and macOS
/// (default volumes) ignore case: we compare in lowercase (character by character, without the
/// contextual final-sigma rule); Linux is case-sensitive. Unicode normalization is out of scope.
pub(crate) fn claim_key(path: &Path) -> PathBuf {
    if cfg!(any(windows, target_os = "macos")) {
        PathBuf::from(
            path.to_string_lossy()
                .chars()
                .flat_map(char::to_lowercase)
                .collect::<String>(),
        )
    } else {
        path.to_path_buf()
    }
}

/// The name starts with `TMP_PREFIX`, ignoring case (ASCII): `.P4KR-TMP:x` is the same file on macOS.
pub(crate) fn has_tmp_prefix(name: &str) -> bool {
    name.len() >= TMP_PREFIX.len()
        && name.as_bytes()[..TMP_PREFIX.len()].eq_ignore_ascii_case(TMP_PREFIX.as_bytes())
}

/// Output path of `name` under `root`, or `None` if it would escape (`..`, absolute, drive `C:`, `./`, empty).
pub fn target(root: &Path, name: &str) -> Option<PathBuf> {
    let rel = norm(name);
    let rel = Path::new(&rel);
    let mut comps = rel.components().peekable();
    comps.peek()?;
    let safe = rel.components().all(|c| {
        matches!(c, Component::Normal(s) if {
            let s = s.to_string_lossy();
            !s.contains(':') && !(cfg!(windows) && has_tmp_prefix(&s))
        })
    });
    safe.then(|| root.join(rel))
}

#[cfg(test)]
mod claim_tests {
    use super::claim_key;
    use std::path::Path;

    #[test]
    fn claim_key_folds_case_only_where_the_disk_does() {
        let a = claim_key(Path::new("out/Data/Ab.xml"));
        let b = claim_key(Path::new("out/Data/aB.xml"));
        if cfg!(any(windows, target_os = "macos")) {
            assert_eq!(a, b);
        } else {
            assert_ne!(a, b);
        }
    }

    #[test]
    fn windows_refuses_components_with_the_temporary_prefix() {
        let t = |n: &str| super::target(Path::new("o"), n).is_some();
        assert!(t("a/.p4kr-tmpx"));
        assert_eq!(t("a/.P4KR-TMP~x"), !cfg!(windows));
        assert_eq!(t(".p4kr-tmp~/y"), !cfg!(windows));
        assert!(!t("a/.p4kr-tmp:1"));
    }
}
