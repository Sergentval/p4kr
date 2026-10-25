//! Guide tests: driven through a buffer, without a terminal.

use super::*;
use crate::lang;
use std::io::Cursor;

const DCB: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../testdata/dcb.p4k");

fn drive_in(input: &[u8], launch: Launch, cfg: &Path, lang: Lang) -> (io::Result<()>, String) {
    let mut reader = Cursor::new(input.to_vec());
    let mut out = Vec::new();
    let mut ctx = Ctx {
        input: &mut reader,
        out: &mut out,
        config: Some(cfg.to_path_buf()),
        home: None,
        today: "2026-10-24".to_string(),
        interactive: false,
        opener: |_| Ok(()),
        lang,
    };
    let r = run(&mut ctx, launch);
    (r, String::from_utf8(out).unwrap())
}

fn drive(input: &[u8], launch: Launch, cfg: &Path) -> (io::Result<()>, String) {
    drive_in(input, launch, cfg, Lang::Fr)
}

/// The part of the French "done" line before the duration.
fn done_prefix() -> String {
    let line = lang::done_in(Lang::Fr, 0.0);
    line[..line.find('0').unwrap()].to_string()
}

static OPENED: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

fn fake_open(p: &Path) -> io::Result<()> {
    OPENED.lock().unwrap().push(p.to_path_buf());
    Ok(())
}

fn refuse_open(_: &Path) -> io::Result<()> {
    Err(io::Error::other("no file manager"))
}

fn open_question(answer: &str, interactive: bool, opener: fn(&Path) -> io::Result<()>) -> String {
    let d = tempfile::tempdir().unwrap();
    let out_dir = d.path().join("output");
    let quit = if answer.is_empty() { "" } else { "q\n" };
    let input = format!("1\n{DCB}\n\n{}\n\n{answer}{quit}", out_dir.display());
    let mut reader = Cursor::new(input.into_bytes());
    let mut out = Vec::new();
    let mut ctx = Ctx {
        input: &mut reader,
        out: &mut out,
        config: None,
        home: None,
        today: "2026-10-24".to_string(),
        interactive,
        opener,
        lang: Lang::Fr,
    };
    run(&mut ctx, Launch::Explicit).unwrap();
    String::from_utf8(out).unwrap()
}

// Only one test touches `OPENED`: the scenarios run in sequence to stay independent of test order.
#[test]
fn the_open_question_is_asked_only_on_a_terminal_and_eof_means_no() {
    let t = open_question("o\n", true, fake_open);
    assert!(t.contains(&done_prefix()), "{t}");
    let ask = format!("{} [O/n]", lang::open_folder_prompt(Lang::Fr));
    assert!(t.contains(&ask), "{t}");
    let opened = OPENED.lock().unwrap().clone();
    assert_eq!(opened.len(), 1, "{t}");
    assert!(opened[0].is_absolute() && opened[0].ends_with("output"));
    OPENED.lock().unwrap().clear();
    let t = open_question("n\n", true, fake_open);
    assert!(t.contains(lang::open_folder_prompt(Lang::Fr)), "{t}");
    let t = open_question("", true, fake_open);
    assert!(t.contains(lang::open_folder_prompt(Lang::Fr)), "{t}");
    assert!(OPENED.lock().unwrap().is_empty());
    let t = open_question("o\n", false, fake_open);
    assert!(!t.contains(lang::open_folder_prompt(Lang::Fr)), "{t}");
    assert!(OPENED.lock().unwrap().is_empty());
    let t = open_question("\n", true, refuse_open);
    let refused = lang::cannot_open_folder(Lang::Fr, &"no file manager");
    assert!(t.contains(&refused), "{t}");
}

#[test]
fn a_missing_output_folder_is_not_offered() {
    let mut reader = Cursor::new(Vec::new());
    let mut out = Vec::new();
    let mut s = Session {
        input: &mut reader,
        out: &mut out,
        home: None,
        today: "2026-10-24",
        config: None,
        settings: Settings::default(),
        interactive: true,
        opener: refuse_open,
        lang: Lang::Fr,
    };
    let gone = tempfile::tempdir().unwrap().path().join("absent");
    assert!(s.offer_open(&gone).is_ok());
    assert!(out.is_empty(), "{}", String::from_utf8_lossy(&out));
}

#[test]
fn bare_launch_waits_for_enter_on_quit() {
    let d = tempfile::tempdir().unwrap();
    let (r, out) = drive(b"q\n\n", Launch::Bare, d.path());
    r.unwrap();
    assert!(out.contains(lang::press_enter_to_close(Lang::Fr)), "{out}");
}

#[test]
fn explicit_launch_quits_without_waiting() {
    let d = tempfile::tempdir().unwrap();
    let (r, out) = drive(b"q\n", Launch::Explicit, d.path());
    r.unwrap();
    assert!(!out.contains(lang::press_enter_to_close(Lang::Fr)), "{out}");
}

/// For each `k`, the first `k` lines then end of input: always `Ok`, and the output folder never exists
/// (the answer to the launch question is "n", so nothing is written, whatever the free space).
fn eof_after_every_line(lines: &[&str], launch: Launch, out_dir: &Path) {
    let cfg = out_dir.parent().unwrap().join("cfg");
    for k in 0..=lines.len() {
        let input: String = lines.iter().take(k).map(|l| format!("{l}\n")).collect();
        let (r, out) = drive(input.as_bytes(), launch, &cfg);
        assert!(r.is_ok(), "k = {k}");
        assert!(!out_dir.exists(), "k = {k}");
        assert_eq!(
            launch == Launch::Bare,
            out.contains(lang::press_enter_to_close(Lang::Fr))
        );
    }
}

#[test]
fn eof_at_every_question_ends_cleanly() {
    let d = tempfile::tempdir().unwrap();
    let out_dir = d.path().join("out");
    let o = out_dir.to_str().unwrap();
    eof_after_every_line(&["1", DCB, "", o, "n", "q"], Launch::Explicit, &out_dir);
    eof_after_every_line(
        &["2", DCB, "", "e", ".xml", "n", o, "n", "q"],
        Launch::Bare,
        &out_dir,
    );
    // Verification writes nothing: the run can go all the way through.
    eof_after_every_line(&["3", DCB, "", "", "q"], Launch::Bare, &out_dir);
}

#[test]
fn dash_means_no_key_even_without_a_remembered_key() {
    let d = tempfile::tempdir().unwrap();
    let input = format!("3\n{DCB}\n-\nn\nq\n");
    let (r, out) = drive(input.as_bytes(), Launch::Explicit, d.path());
    r.unwrap();
    let unreadable = lang::key_unreadable(Lang::Fr, &"");
    assert!(!out.contains(unreadable.trim_end()), "{out}");
    assert!(out.contains(lang::launch_prompt(Lang::Fr)), "{out}");
}

#[test]
fn unknown_choice_shows_the_menu_again() {
    let d = tempfile::tempdir().unwrap();
    let (r, out) = drive(b"7\nq\n", Launch::Explicit, d.path());
    r.unwrap();
    assert!(out.contains(&lang::unknown_choice(Lang::Fr, "7")), "{out}");
    assert_eq!(out.matches(lang::menu_title(Lang::Fr)).count(), 2);
}

#[test]
fn unreadable_input_ends_cleanly_with_a_message() {
    let d = tempfile::tempdir().unwrap();
    let (r, out) = drive(b"\xff\xfe\n", Launch::Explicit, d.path());
    r.unwrap();
    assert!(out.contains(lang::input_unreadable(Lang::Fr)), "{out}");
}

/// Catalogue texts that are the same in both languages (a name, a path hint or a symbol, not a sentence):
/// `archive_prompt` is "Archive (.p4k)" and `action_dataforge` is "DataForge → XML" in English and French.
const SAME_IN_BOTH: &[&str] = &["archive_prompt", "action_dataforge"];

/// The separator is punctuation that cannot be searched for in a whole output without false matches.
const PUNCTUATION: &[&str] = &["prompt_separator"];

#[test]
fn the_catalogue_texts_differ_between_languages_except_the_named_ones() {
    assert!(lang::PLAIN.len() > 30);
    for (name, f) in lang::PLAIN {
        let same = f(Lang::En) == f(Lang::Fr);
        assert_eq!(same, SAME_IN_BOTH.contains(name), "{name}");
    }
}

#[test]
fn an_english_run_shows_no_french_text() {
    let d = tempfile::tempdir().unwrap();
    let out_dir = d.path().join("out");
    let input = format!("1\n{DCB}\n\n{}\ny\nq\n", out_dir.display());
    let (r, out) = drive_in(
        input.as_bytes(),
        Launch::Bare,
        &d.path().join("cfg"),
        Lang::En,
    );
    r.unwrap();
    assert!(out.contains(lang::menu_title(Lang::En)), "{out}");
    assert!(out.contains(lang::launch_prompt(Lang::En)), "{out}");
    assert!(out.contains(lang::press_enter_to_close(Lang::En)), "{out}");
    assert!(out_dir.exists(), "{out}");
    for (name, f) in lang::PLAIN {
        if SAME_IN_BOTH.contains(name) || PUNCTUATION.contains(name) {
            continue;
        }
        assert!(!out.contains(f(Lang::Fr)), "{name}\n{out}");
    }
    let french = [
        lang::unknown_choice(Lang::Fr, ""),
        lang::action_line(Lang::Fr, ""),
        lang::archive_line(Lang::Fr, ""),
        lang::output_line(Lang::Fr, ""),
        lang::header_fields(Lang::Fr, "", "", ""),
        lang::yes_no_hint(Lang::Fr, true).to_string(),
        lang::yes_no_hint(Lang::Fr, false).to_string(),
        done_prefix(),
    ];
    for f in french {
        assert!(!out.contains(f.trim()), "{f}\n{out}");
    }
}

#[test]
fn the_folder_key_is_f_in_english_and_d_in_french_and_both_work() {
    assert!(lang::filter_kind_prompt(Lang::En).contains("(f)"));
    assert!(lang::filter_kind_prompt(Lang::Fr).contains("(d)"));
    for lang in [Lang::En, Lang::Fr] {
        for key in ["d", "f"] {
            let d = tempfile::tempdir().unwrap();
            let input = format!("2\n{DCB}\n\n{key}\n\n");
            let (r, out) = drive_in(input.as_bytes(), Launch::Explicit, d.path(), lang);
            r.unwrap();
            assert!(
                out.contains(lang::filter_folder_label(lang)),
                "{lang:?} {key}\n{out}"
            );
            assert!(
                !out.contains(lang::answer_d_or_e(lang)),
                "{lang:?} {key}\n{out}"
            );
        }
        let (_, out) = drive_in(
            format!("2\n{DCB}\n\nx\n").as_bytes(),
            Launch::Explicit,
            tempfile::tempdir().unwrap().path(),
            lang,
        );
        assert!(out.contains(lang::answer_d_or_e(lang)), "{out}");
    }
}
