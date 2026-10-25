//! Settings the guide remembers (`p4kr.toml`): three paths, never the content of the key. Minimal TOML reading
//! and writing (known keys, quoted text values, `\\` and `\"` escapes).

use crate::lang::{self, Lang};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const FILE: &str = "p4kr.toml";
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);
const KEYS: [&str; 3] = ["archive", "key_file", "out_dir"];

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Settings {
    pub archive: Option<PathBuf>,
    pub key_file: Option<PathBuf>,
    pub out_dir: Option<PathBuf>,
}

impl Settings {
    fn slot(&mut self, key: &str) -> Option<&mut Option<PathBuf>> {
        match key {
            "archive" => Some(&mut self.archive),
            "key_file" => Some(&mut self.key_file),
            "out_dir" => Some(&mut self.out_dir),
            _ => None,
        }
    }
    fn values(&self) -> [(&'static str, &Option<PathBuf>); 3] {
        [
            (KEYS[0], &self.archive),
            (KEYS[1], &self.key_file),
            (KEYS[2], &self.out_dir),
        ]
    }
}

pub fn config_dir(home: Option<&Path>) -> Option<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(d) = var("P4KR_CONFIG_DIR") {
        return Some(d);
    }
    let base = if cfg!(windows) {
        var("APPDATA")?
    } else if cfg!(target_os = "macos") {
        home?.join("Library/Application Support")
    } else {
        var("XDG_CONFIG_HOME").or_else(|| home.map(|h| h.join(".config")))?
    };
    Some(base.join("p4kr"))
}

fn unquote(v: &str) -> Option<String> {
    let inner = v.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next()? {
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                _ => return None,
            },
            '"' => return None,
            c => out.push(c),
        }
    }
    Some(out)
}

pub fn parse(text: &str, lang: Lang) -> Result<Settings, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut s = Settings::default();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (k, v) = line
            .split_once('=')
            .ok_or_else(|| lang::settings_line_expected(lang, n + 1))?;
        let slot = s
            .slot(k.trim())
            .ok_or_else(|| lang::settings_unknown_key(lang, n + 1, k.trim()))?;
        let value = unquote(v.trim()).ok_or_else(|| lang::settings_quoted_expected(lang, n + 1))?;
        *slot = Some(PathBuf::from(value));
    }
    Ok(s)
}

/// First line of the saved file: one English comment whatever the display language.
const HEADER: &str = "# p4kr guided mode settings (paths only)\n";

pub fn render(s: &Settings) -> String {
    let mut out = String::from(HEADER);
    for (k, v) in s.values() {
        if let Some(p) = v {
            let esc = p
                .to_string_lossy()
                .replace('\\', "\\\\")
                .replace('"', "\\\"");
            out.push_str(&format!("{k} = \"{esc}\"\n"));
        }
    }
    out
}

/// Windows denies access for a moment while another save replaces the file (rename over it).
const IO_ATTEMPTS: u32 = if cfg!(windows) { 50 } else { 1 };
const IO_RETRY: std::time::Duration = std::time::Duration::from_millis(10);

/// Runs `op`, retrying a transient PermissionDenied (Windows) a bounded number of times.
fn retry_denied<T>(mut op: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let mut attempt = 1;
    loop {
        match op() {
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied && attempt < IO_ATTEMPTS => {
                attempt += 1;
                std::thread::sleep(IO_RETRY);
            }
            other => return other,
        }
    }
}

/// Reads the settings file, retrying a transient PermissionDenied (Windows) a few times.
fn read_settings(path: &Path) -> io::Result<String> {
    retry_denied(|| std::fs::read_to_string(path))
}

pub fn load(dir: &Path, lang: Lang) -> (Settings, Option<String>) {
    let path = dir.join(FILE);
    match read_settings(&path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => (Settings::default(), None),
        Err(e) => (
            Settings::default(),
            Some(lang::settings_unreadable(
                lang,
                &path.display().to_string(),
                &e,
            )),
        ),
        Ok(text) => match parse(&text, lang) {
            Ok(s) => (s, None),
            Err(e) => (
                Settings::default(),
                Some(lang::settings_invalid(
                    lang,
                    &path.display().to_string(),
                    &e,
                )),
            ),
        },
    }
}

/// Refuses (writing nothing) a path that `render` could not read back faithfully.
fn check_storable(s: &Settings, lang: Lang) -> io::Result<()> {
    for (k, v) in s.values() {
        let Some(p) = v else { continue };
        let ok = p.to_str().is_some_and(|t| !t.chars().any(char::is_control));
        if !ok {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                lang::path_not_storable(lang, k),
            ));
        }
    }
    Ok(())
}

/// Atomic write: unique temporary file (pid + counter), `sync_all`, then rename.
pub fn save(dir: &Path, s: &Settings, lang: Lang) -> io::Result<()> {
    check_storable(s, lang)?;
    std::fs::create_dir_all(dir)?;
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!("{FILE}.{}.{n}.tmp", std::process::id()));
    let result = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(render(s).as_bytes())?;
        f.sync_all()?;
        drop(f);
        let dest = dir.join(FILE);
        retry_denied(|| std::fs::rename(&tmp, &dest))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Barrier, Mutex};

    /// Serializes the tests that touch the process environment.
    static ENV: Mutex<()> = Mutex::new(());

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("p4kr-settings-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn read_settings_on_a_missing_file_returns_not_found_without_retrying() {
        let d = temp_dir("read-missing");
        let start = std::time::Instant::now();
        let e = read_settings(&d.join(FILE)).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
        assert!(start.elapsed() < IO_RETRY * 20);
    }

    #[test]
    fn render_then_parse_round_trips() {
        let s = Settings {
            archive: Some(PathBuf::from("C:\\My Folder\\a \"b\".p4k")),
            key_file: Some(PathBuf::from("/h/key é file")), // check-french: allow Unicode path test input
            out_dir: None,
        };
        assert_eq!(parse(&render(&s), Lang::Fr).unwrap(), s);
    }

    #[test]
    fn render_does_not_depend_on_the_language() {
        let s = Settings {
            archive: Some(PathBuf::from("/a")),
            ..Settings::default()
        };
        assert!(render(&s).starts_with(HEADER));
        let d = temp_dir("render-lang");
        save(&d, &s, Lang::En).unwrap();
        let en = std::fs::read(d.join(FILE)).unwrap();
        save(&d, &s, Lang::Fr).unwrap();
        assert_eq!(en, std::fs::read(d.join(FILE)).unwrap());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_file_with_the_old_french_header_still_loads() {
        let d = temp_dir("old-header");
        std::fs::create_dir_all(&d).unwrap();
        let old =
            "# R\u{e9}glages du mode guid\u{e9} de p4kr (chemins seulement)\narchive = \"/a\"\n"; // check-french: allow old French header data
        std::fs::write(d.join(FILE), old).unwrap();
        let (s, w) = load(&d, Lang::En);
        assert_eq!(w, None);
        assert_eq!(s.archive, Some(PathBuf::from("/a")));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn parse_ignores_blank_lines_and_comments() {
        let s = parse("# c\n\n  \narchive = \"/a\"\n# end\n", Lang::Fr).unwrap();
        assert_eq!(s.archive, Some(PathBuf::from("/a")));
        assert_eq!(s.key_file, None);
    }

    #[test]
    fn parse_rejects_unknown_keys_and_bad_quotes() {
        let e = parse("archive = \"/a\"\nfoo = \"x\"\n", Lang::Fr).unwrap_err();
        assert!(
            e.contains(&lang::settings_unknown_key(Lang::Fr, 2, "foo")),
            "{e}"
        );
        for bad in [
            "archive = /a",
            "archive = \"/a",
            "archive = \"a\"b\"",
            "archive = \"a\\n\"",
        ] {
            let e = parse(&format!("\n{bad}\n"), Lang::Fr).unwrap_err();
            assert!(
                e.contains(&lang::settings_quoted_expected(Lang::Fr, 2)),
                "{bad}: {e}"
            );
        }
        assert_eq!(
            parse("archive", Lang::Fr).unwrap_err(),
            lang::settings_line_expected(Lang::Fr, 1)
        );
    }

    #[test]
    fn load_missing_file_is_default_without_warning() {
        let d = temp_dir("missing");
        assert_eq!(load(&d, Lang::Fr), (Settings::default(), None));
    }

    #[test]
    fn load_broken_file_is_default_with_warning() {
        let d = temp_dir("broken");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(FILE), "nope = \"x\"\n").unwrap();
        let (s, w) = load(&d, Lang::Fr);
        assert_eq!(s, Settings::default());
        assert!(
            w.unwrap()
                .contains(&lang::settings_unknown_key(Lang::Fr, 1, "nope"))
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn save_creates_the_directory_and_never_writes_key_bytes() {
        let d = temp_dir("save").join("sub");
        let s = Settings {
            archive: Some(PathBuf::from("/a.p4k")),
            key_file: Some(PathBuf::from("/k.txt")),
            out_dir: Some(PathBuf::from("/o")),
        };
        save(&d, &s, Lang::Fr).unwrap();
        let text = std::fs::read_to_string(d.join(FILE)).unwrap();
        let keys: Vec<&str> = text
            .lines()
            .filter(|l| !l.starts_with('#'))
            .map(|l| l.split('=').next().unwrap().trim())
            .collect();
        assert_eq!(keys, KEYS);
        assert!(!d.join(format!("{FILE}.tmp")).exists());
        assert_eq!(load(&d, Lang::Fr), (s, None));
        std::fs::remove_dir_all(d.parent().unwrap()).unwrap();
    }

    /// Restores the variables to their original state, even if an assertion fails.
    struct EnvGuard(Vec<(&'static str, Option<OsString>)>);

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (name, old) in self.0.drain(..) {
                // SAFETY: the ENV lock serializes the crate's tests and std serializes its own environment access.
                unsafe {
                    match old {
                        Some(v) => std::env::set_var(name, v),
                        None => std::env::remove_var(name),
                    }
                }
            }
        }
    }

    #[test]
    fn config_dir_prefers_p4kr_config_dir() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let names = ["P4KR_CONFIG_DIR", "XDG_CONFIG_HOME", "APPDATA"];
        let _restore = EnvGuard(names.iter().map(|n| (*n, std::env::var_os(n))).collect());

        // SAFETY: the ENV lock serializes the tests, std serializes its own environment access; no other test of the
        // crate spawns a process.
        unsafe {
            std::env::set_var("P4KR_CONFIG_DIR", "/x");
        }
        assert_eq!(config_dir(Some(Path::new("/h"))), Some(PathBuf::from("/x")));

        // SAFETY: same.
        unsafe {
            std::env::remove_var("P4KR_CONFIG_DIR");
            std::env::remove_var("XDG_CONFIG_HOME");
            std::env::remove_var("APPDATA");
        }
        let got = config_dir(Some(Path::new("/h")));
        if cfg!(windows) {
            assert_eq!(got, None);
        } else if cfg!(target_os = "macos") {
            assert_eq!(
                got,
                Some(Path::new("/h").join("Library/Application Support/p4kr"))
            );
        } else {
            assert_eq!(got, Some(Path::new("/h").join(".config").join("p4kr")));
        }
        assert_eq!(config_dir(None), None);
    }

    #[test]
    fn parse_strips_a_leading_bom() {
        let s = parse("\u{feff}archive = \"/a\"\n", Lang::Fr).unwrap();
        assert_eq!(s.archive, Some(PathBuf::from("/a")));
    }

    #[test]
    fn save_refuses_control_characters_and_keeps_the_file() {
        let d = temp_dir("ctrl");
        let good = Settings {
            archive: Some(PathBuf::from("/ok")),
            ..Settings::default()
        };
        save(&d, &good, Lang::Fr).unwrap();
        let bad = Settings {
            out_dir: Some(PathBuf::from("/a\nb")),
            ..Settings::default()
        };
        let e = save(&d, &bad, Lang::Fr).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(load(&d, Lang::Fr), (good, None));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn save_refuses_non_utf8_paths() {
        use std::os::unix::ffi::OsStrExt;
        let d = temp_dir("nonutf8");
        std::fs::create_dir_all(&d).unwrap();
        let bad = Settings {
            archive: Some(PathBuf::from(std::ffi::OsStr::from_bytes(b"a\xff"))),
            ..Settings::default()
        };
        let e = save(&d, &bad, Lang::Fr).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
        assert!(!d.join(FILE).exists());
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn concurrent_saves_are_atomic_and_leave_no_temp_file() {
        let d = temp_dir("concurrent");
        std::fs::create_dir_all(&d).unwrap();
        let a = Settings {
            archive: Some(PathBuf::from("/a/archive")),
            key_file: Some(PathBuf::from("/a/key")),
            out_dir: Some(PathBuf::from("/a/out")),
        };
        let b = Settings {
            archive: Some(PathBuf::from("/b/archive with space")),
            key_file: Some(PathBuf::from("/b/key")),
            out_dir: Some(PathBuf::from("/b/out")),
        };
        // The reader is already loading when the writers start, and they only stop after a load done while they
        // write.
        let barrier = Barrier::new(3);
        let finished = AtomicUsize::new(0);
        let during = AtomicUsize::new(0);
        let check = |d: &Path| {
            let (s, w) = load(d, Lang::Fr);
            assert_eq!(w, None);
            assert!(s == a || s == b || s == Settings::default(), "{s:?}");
        };
        std::thread::scope(|sc| {
            sc.spawn(|| {
                check(&d);
                barrier.wait();
                loop {
                    let active = finished.load(Ordering::Acquire) < 2;
                    check(&d);
                    if !active {
                        break;
                    }
                    during.fetch_add(1, Ordering::Release);
                }
            });
            for v in [&a, &b] {
                let (d, barrier, finished, during) = (&d, &barrier, &finished, &during);
                sc.spawn(move || {
                    barrier.wait();
                    for _ in 0..50 {
                        save(d, v, Lang::Fr).unwrap();
                    }
                    while during.load(Ordering::Acquire) == 0 {
                        save(d, v, Lang::Fr).unwrap();
                    }
                    finished.fetch_add(1, Ordering::Release);
                });
            }
        });
        assert!(during.load(Ordering::Acquire) > 0);
        let (last, w) = load(&d, Lang::Fr);
        assert_eq!(w, None);
        assert!(last == a || last == b);
        let left: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, vec![OsString::from(FILE)]);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
