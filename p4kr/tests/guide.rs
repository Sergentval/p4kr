//! The real binary, guided mode, input through a pipe. The environment goes through `Command`, never through
//! this process.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

// The guide's text catalogue, shared with the binary so the assertions never repeat a literal.
#[allow(dead_code)]
#[path = "../src/lang.rs"]
mod lang;
use lang::Lang;

/// The `cannot_open` message in French, up to the cause.
fn cannot_open() -> String {
    lang::cannot_open(Lang::Fr, &"").trim_end().to_string()
}

const BIN: &str = env!("CARGO_BIN_EXE_p4kr");
const TESTDATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../testdata");

struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }
    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
    fn cfg(&self) -> PathBuf {
        self.path("cfg")
    }
    /// Copies an archive from `testdata` into the sandbox.
    fn fixture(&self, name: &str) -> PathBuf {
        let to = self.path(name);
        std::fs::copy(Path::new(TESTDATA).join(name), &to).unwrap();
        to
    }
    fn command(&self) -> Command {
        let mut c = Command::new(BIN);
        c.env("P4KR_CONFIG_DIR", self.cfg())
            .env("P4KR_MIN_FREE_GB", "0")
            .env("P4KR_LANG", "fr")
            .env("HOME", self.dir.path())
            .env("USERPROFILE", self.dir.path());
        c
    }
    fn guide(&self, input: &[u8]) -> Output {
        self.guide_in("fr", input)
    }
    /// Runs the guide with `P4KR_LANG` set to `lang`.
    fn guide_in(&self, lang: &str, input: &[u8]) -> Output {
        let mut child = self
            .command()
            .env("P4KR_LANG", lang)
            .arg("guide")
            .current_dir(self.dir.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // The guide may end before reading everything: the write may then fail.
        let _ = child.stdin.take().unwrap().write_all(input);
        child.wait_with_output().unwrap()
    }
    fn cli(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }
}

fn text(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, into: &mut BTreeMap<String, Vec<u8>>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.map(Result::unwrap) {
            let p = e.path();
            if p.is_dir() {
                walk(root, &p, into);
            } else {
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
                into.insert(rel, std::fs::read(&p).unwrap());
            }
        }
    }
    let mut into = BTreeMap::new();
    walk(root, root, &mut into);
    into
}

#[test]
fn dataforge_choice_writes_the_same_tree_as_extract_xml() {
    let sb = Sandbox::new();
    let a = sb.fixture("dcb.p4k");
    let (df, cli) = (sb.path("df"), sb.path("cli"));
    let o = sb.guide(format!("1\n{}\n\n{}\n\nq\n", a.display(), df.display()).as_bytes());
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(
        text(&o).contains(&lang::output_line(Lang::Fr, "")),
        "{}",
        text(&o)
    );
    let c = sb.cli(&[
        "extract",
        a.to_str().unwrap(),
        "--out",
        cli.to_str().unwrap(),
        "--xml",
        "--ext",
        ".dcb",
        "--quiet",
    ]);
    assert_eq!(c.status.code(), Some(0));
    let (got, want) = (tree(&df), tree(&cli));
    assert!(!want.is_empty());
    assert_eq!(got, want);
}

#[test]
fn quoted_and_escaped_paths_are_accepted() {
    let sb = Sandbox::new();
    let a = sb.fixture("dcb.p4k");
    let quoted = format!("3\n\"{}\"\n\nn\nq\n", a.display());
    let o = sb.guide(quoted.as_bytes());
    assert!(!text(&o).contains(&cannot_open()), "{}", text(&o));
    assert!(
        text(&o).contains(lang::verify_only(Lang::Fr)),
        "{}",
        text(&o)
    );
    if cfg!(not(windows)) {
        let spaced = sb.path("a b");
        std::fs::create_dir_all(&spaced).unwrap();
        std::fs::copy(&a, spaced.join("x.p4k")).unwrap();
        let escaped = spaced.join("x.p4k").to_str().unwrap().replace(' ', "\\ ");
        let o = sb.guide(format!("3\n{escaped}\n\nn\nq\n").as_bytes());
        assert!(!text(&o).contains(&cannot_open()), "{}", text(&o));
        assert!(
            text(&o).contains(lang::verify_only(Lang::Fr)),
            "{}",
            text(&o)
        );
    }
}

#[test]
fn verify_choice_prints_the_same_line_as_the_verify_command() {
    let sb = Sandbox::new();
    let a = sb.fixture("synth.p4k");
    let k = sb.fixture("synth.key.toml");
    let o = sb.guide_in(
        "en",
        format!("3\n{}\n{}\n\nq\n", a.display(), k.display()).as_bytes(),
    );
    assert_eq!(o.status.code(), Some(0));
    let c = sb.cli(&[
        "verify",
        a.to_str().unwrap(),
        "--key-file",
        k.to_str().unwrap(),
        "--quiet",
    ]);
    let line = String::from_utf8(c.stdout).unwrap();
    let line = line.trim_end();
    assert!(!line.is_empty(), "{line}");
    assert!(text(&o).lines().any(|l| l == line), "{line}\n{}", text(&o));
    // The progress bar may share the line, so the action line is matched by its end.
    let action = lang::action_line(Lang::En, lang::action_verify(Lang::En));
    assert!(
        text(&o).lines().any(|l| l.ends_with(&action)),
        "{}",
        text(&o)
    );
    let archive_line = lang::archive_line(Lang::En, &a.display().to_string());
    assert!(
        text(&o).lines().any(|l| l == archive_line),
        "{archive_line}\n{}",
        text(&o)
    );
}

#[test]
fn targeted_choice_extracts_only_the_extension() {
    let sb = Sandbox::new();
    let a = sb.fixture("xml.p4k");
    let (x, cli) = (sb.path("x"), sb.path("cli"));
    let o =
        sb.guide(format!("2\n{}\n\ne\n.xml\nn\n{}\n\nq\n", a.display(), x.display()).as_bytes());
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let c = sb.cli(&[
        "extract",
        a.to_str().unwrap(),
        "--out",
        cli.to_str().unwrap(),
        "--ext",
        ".xml",
        "--quiet",
    ]);
    assert_eq!(c.status.code(), Some(0));
    let got = tree(&x);
    assert!(!got.is_empty());
    assert!(got.keys().all(|k| k.to_lowercase().ends_with(".xml")));
    assert_eq!(got, tree(&cli));
}

#[test]
fn non_empty_output_folder_needs_confirmation() {
    let sb = Sandbox::new();
    let a = sb.fixture("dcb.p4k");
    let out = sb.path("full");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("keep.txt"), b"keep").unwrap();
    let o = sb.guide(format!("1\n{}\n\n{}\nn\nq\n", a.display(), out.display()).as_bytes());
    assert_eq!(o.status.code(), Some(0));
    assert!(
        text(&o).contains(lang::folder_not_empty(Lang::Fr)),
        "{}",
        text(&o)
    );
    assert_eq!(tree(&out).keys().collect::<Vec<_>>(), ["keep.txt"]);
}

#[test]
fn missing_archive_is_asked_again() {
    let sb = Sandbox::new();
    let nope = sb.path("nope.p4k");
    let o = sb.guide(format!("1\n{}\n", nope.display()).as_bytes());
    assert_eq!(o.status.code(), Some(0));
    let t = text(&o);
    assert!(t.contains(&cannot_open()), "{t}");
    assert_eq!(t.matches(lang::archive_prompt(Lang::Fr)).count(), 2, "{t}");
}

#[test]
fn settings_are_remembered_between_runs() {
    let sb = Sandbox::new();
    let a = sb.fixture("dcb.p4k");
    let first =
        sb.guide(format!("1\n{}\n\n{}\n\nq\n", a.display(), sb.path("df").display()).as_bytes());
    assert_eq!(first.status.code(), Some(0));
    let saved = std::fs::read_to_string(sb.cfg().join("p4kr.toml")).unwrap();
    assert!(saved.contains("dcb.p4k"), "{saved}");
    let df2 = sb.path("df2");
    let second = sb.guide(format!("1\n\n\n{}\n\nq\n", df2.display()).as_bytes());
    assert_eq!(second.status.code(), Some(0));
    assert!(text(&second).contains("dcb.p4k"), "{}", text(&second));
    assert!(!tree(&df2).is_empty(), "{}", text(&second));
}

#[test]
fn broken_settings_file_only_warns() {
    let sb = Sandbox::new();
    std::fs::create_dir_all(sb.cfg()).unwrap();
    std::fs::write(sb.cfg().join("p4kr.toml"), "nonsense").unwrap();
    let o = sb.guide(b"q\n");
    assert_eq!(o.status.code(), Some(0));
    let t = text(&o);
    let broken = lang::settings_invalid(
        Lang::Fr,
        &sb.cfg().join("p4kr.toml").display().to_string(),
        &lang::settings_line_expected(Lang::Fr, 1),
    );
    assert!(t.contains(&broken), "{t}");
    assert!(t.contains(lang::menu_title(Lang::Fr)), "{t}");
}

#[test]
fn archive_without_dataforge_says_so() {
    let sb = Sandbox::new();
    let a = sb.fixture("synth.p4k");
    let o = sb.guide(format!("1\n{}\n\nq\n", a.display()).as_bytes());
    assert_eq!(o.status.code(), Some(0));
    assert!(
        text(&o).contains(lang::no_dataforge(Lang::Fr)),
        "{}",
        text(&o)
    );
}

#[test]
fn unreadable_input_ends_cleanly() {
    let sb = Sandbox::new();
    let o = sb.guide(b"\xff\xfe\n");
    assert_eq!(o.status.code(), Some(0));
    assert!(
        text(&o).contains(lang::input_unreadable(Lang::Fr)),
        "{}",
        text(&o)
    );
}

#[test]
fn bare_launch_outside_a_terminal_prints_help() {
    let sb = Sandbox::new();
    let mut child = sb
        .command()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdin.take());
    let o = child.wait_with_output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    let help = String::from_utf8_lossy(&o.stderr);
    assert!(help.contains("guide"), "{help}");
    assert!(o.stdout.is_empty(), "{}", text(&o));
}

/// Value of `archive` in `p4kr.toml`.
fn saved_archive(sb: &Sandbox) -> String {
    let saved = std::fs::read_to_string(sb.cfg().join("p4kr.toml")).unwrap();
    let line = saved.lines().find(|l| l.starts_with("archive")).unwrap();
    let value = line.split_once('=').unwrap().1.trim().trim_matches('"');
    value.replace("\\\\", "\\")
}

#[test]
fn relative_paths_are_saved_as_absolute() {
    let sb = Sandbox::new();
    sb.fixture("dcb.p4k");
    let o = sb.guide(b"3\ndcb.p4k\n\n\nq\n");
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(
        text(&o).contains(&lang::action_line(Lang::Fr, lang::action_verify(Lang::Fr))),
        "{}",
        text(&o)
    );
    let saved = saved_archive(&sb);
    assert!(Path::new(&saved).is_absolute(), "{saved}");
    assert!(saved.ends_with("dcb.p4k"), "{saved}");
}

#[test]
fn output_path_that_is_a_file_is_asked_again() {
    let sb = Sandbox::new();
    let a = sb.fixture("dcb.p4k");
    let file = sb.path("plain.txt");
    std::fs::write(&file, b"x").unwrap();
    let o = sb.guide(format!("1\n{}\n\n{}\n", a.display(), file.display()).as_bytes());
    assert_eq!(o.status.code(), Some(0));
    let t = text(&o);
    assert!(
        t.contains(&lang::not_a_folder(Lang::Fr, &file.display().to_string())),
        "{t}"
    );
    assert_eq!(t.matches(lang::output_prompt(Lang::Fr)).count(), 2, "{t}");
}

#[test]
fn missing_retained_folder_warns_and_is_forgotten() {
    let sb = Sandbox::new();
    std::fs::create_dir_all(sb.cfg()).unwrap();
    let gone = sb.path("gone");
    let toml = format!("out_dir = \"{}\"\n", gone.display()).replace('\\', "\\\\");
    std::fs::write(sb.cfg().join("p4kr.toml"), toml).unwrap();
    let o = sb.guide(b"q\n");
    assert_eq!(o.status.code(), Some(0));
    let t = text(&o);
    let missing = lang::not_found(
        Lang::Fr,
        lang::saved_folder_label(Lang::Fr),
        &gone.display().to_string(),
    );
    assert!(t.contains(&missing), "{t}");
    let header = lang::header_fields(Lang::Fr, "", "", lang::none_masculine(Lang::Fr));
    assert!(t.contains(header.lines().last().unwrap()), "{t}");
}

#[test]
fn empty_filter_answer_is_refused() {
    let sb = Sandbox::new();
    let a = sb.fixture("xml.p4k");
    let o = sb.guide(format!("2\n{}\n\ne\n\n", a.display()).as_bytes());
    assert_eq!(o.status.code(), Some(0));
    assert!(
        text(&o).contains(lang::empty_answer(Lang::Fr)),
        "{}",
        text(&o)
    );
}

#[test]
fn a_folder_holding_data_p4k_is_taken_as_the_archive() {
    let sb = Sandbox::new();
    let game = sb.path("game");
    std::fs::create_dir_all(&game).unwrap();
    std::fs::copy(Path::new(TESTDATA).join("dcb.p4k"), game.join("Data.p4k")).unwrap();
    let o = sb.guide(format!("3\n{}\n\nn\nq\n", game.display()).as_bytes());
    let t = text(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    let found = game.join("Data.p4k");
    assert!(
        t.contains(&lang::archive_found(Lang::Fr, &found.display().to_string())),
        "{t}"
    );
    assert!(!t.contains(&cannot_open()), "{t}");
    assert!(t.contains(lang::verify_only(Lang::Fr)), "{t}");
}

#[test]
fn a_folder_without_data_p4k_is_refused_and_asked_again() {
    let sb = Sandbox::new();
    let a = sb.fixture("dcb.p4k");
    let empty = sb.path("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let o = sb.guide(format!("3\n{}\n{}\n\nn\nq\n", empty.display(), a.display()).as_bytes());
    let t = text(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    assert!(
        t.contains(&lang::is_a_folder(Lang::Fr, &empty.display().to_string())),
        "{t}"
    );
    assert!(!t.contains(&cannot_open()), "{t}");
    assert!(t.contains(lang::verify_only(Lang::Fr)), "{t}");
}

#[test]
fn p4kr_lang_selects_the_language_of_the_guide() {
    let sb = Sandbox::new();
    let run = |lang: &str| {
        let mut c = sb.command();
        c.env("P4KR_LANG", lang)
            .arg("guide")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        let mut child = c.spawn().unwrap();
        let _ = child.stdin.take().unwrap().write_all(b"q\n");
        text(&child.wait_with_output().unwrap())
    };
    assert!(run("en").contains(lang::menu_title(Lang::En)));
    assert!(run("fr").contains(lang::menu_title(Lang::Fr)));
}
