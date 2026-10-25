//! Guided mode questions: one line read, default value, cleaning of pasted or dropped paths.

use crate::lang::{self, Lang};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum Answer {
    Text(String),
    Eof,
}

fn read_line(input: &mut dyn BufRead, out: &mut dyn Write) -> io::Result<Option<String>> {
    out.flush()?;
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        writeln!(out)?;
        return Ok(None);
    }
    Ok(Some(line.trim_end_matches(['\r', '\n']).trim().to_string()))
}

pub fn ask(
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    prompt: &str,
    default: Option<&str>,
    lang: Lang,
) -> io::Result<Answer> {
    let sep = lang::prompt_separator(lang);
    match default.filter(|d| !d.is_empty()) {
        Some(d) => write!(out, "{prompt} [{d}]{sep}")?,
        None => write!(out, "{prompt}{sep}")?,
    }
    Ok(match read_line(input, out)? {
        None => Answer::Eof,
        Some(t) if t.is_empty() => Answer::Text(default.unwrap_or_default().to_string()),
        Some(t) => Answer::Text(t),
    })
}

pub fn yes_no(
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    prompt: &str,
    default: bool,
    lang: Lang,
) -> io::Result<Option<bool>> {
    let hint = lang::yes_no_hint(lang, default);
    loop {
        write!(out, "{prompt} {hint}{}", lang::prompt_separator(lang))?;
        let Some(t) = read_line(input, out)? else {
            return Ok(None);
        };
        match t.to_lowercase().as_str() {
            "" => return Ok(Some(default)),
            "o" | "oui" | "y" | "yes" => return Ok(Some(true)),
            "n" | "non" | "no" => return Ok(Some(false)),
            _ => writeln!(out, "{}", lang::answer_y_or_n(lang))?,
        }
    }
}

fn strip_quotes(t: &str) -> &str {
    for q in ['"', '\''] {
        if t.len() >= 2 && t.starts_with(q) && t.ends_with(q) {
            return &t[1..t.len() - 1];
        }
    }
    t
}

/// Characters that the macOS terminal (and shells) escape with a backslash on drag and drop.
const ESCAPED: &str = " ()'\"&!$;`[]{}*?<>|#~=\\";

/// Removes the protective backslashes: `\c` becomes `c` for the characters of `ESCAPED`, any other
/// backslash stays as is.
fn unescape_shell(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        match chars.peek() {
            Some(&n) if c == '\\' && ESCAPED.contains(n) => {
                out.push(n);
                chars.next();
            }
            _ => out.push(c),
        }
    }
    out
}

/// Typed, pasted or dropped path: surrounding spaces, one pair of quotes (Windows), protective backslashes
/// (macOS and Linux terminals), leading `~`.
pub fn clean_path(raw: &str, home: Option<&Path>) -> PathBuf {
    let t = strip_quotes(raw.trim());
    // The `~` is read before the backslashes are removed: `\~` is a literal tilde.
    let unescape = |s: &str| {
        if cfg!(windows) {
            s.to_string()
        } else {
            unescape_shell(s)
        }
    };
    match (t.strip_prefix('~'), home) {
        (Some(rest), Some(h)) if rest.is_empty() || rest.starts_with(['/', '\\']) => {
            h.join(unescape(rest.trim_start_matches(['/', '\\'])))
        }
        _ => PathBuf::from(unescape(t)),
    }
}

pub fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn run_ask(input: &str, default: Option<&str>) -> (Answer, String) {
        run_ask_in(input, default, Lang::Fr)
    }

    fn run_ask_in(input: &str, default: Option<&str>, lang: Lang) -> (Answer, String) {
        let mut out = Vec::new();
        let a = ask(
            &mut Cursor::new(input.as_bytes()),
            &mut out,
            "Q",
            default,
            lang,
        )
        .unwrap();
        (a, String::from_utf8(out).unwrap())
    }

    #[test]
    fn empty_answer_takes_the_default() {
        let (a, out) = run_ask("\n", Some("x"));
        assert_eq!(a, Answer::Text("x".into()));
        assert_eq!(out, "Q [x] : ");
    }

    #[test]
    fn english_prompts_have_no_space_before_the_colon() {
        assert_eq!(run_ask_in("\n", Some("x"), Lang::En).1, "Q [x]: ");
        assert_eq!(run_ask_in("a\n", None, Lang::En).1, "Q: ");
    }

    #[test]
    fn no_default_shows_no_brackets() {
        let (_, out) = run_ask("a\n", None);
        assert_eq!(out, "Q : ");
    }

    #[test]
    fn end_of_input_is_eof() {
        assert_eq!(run_ask("", Some("x")).0, Answer::Eof);
    }

    #[test]
    fn crlf_is_stripped() {
        assert_eq!(run_ask("abc\r\n", None).0, Answer::Text("abc".into()));
    }

    fn ask_yes_no(input: &str, default: bool, lang: Lang) -> (Option<bool>, String) {
        let mut out = Vec::new();
        let r = yes_no(
            &mut Cursor::new(input.as_bytes()),
            &mut out,
            "Q",
            default,
            lang,
        )
        .unwrap();
        (r, String::from_utf8(out).unwrap())
    }

    #[test]
    fn yes_no_accepts_both_languages_and_reasks() {
        for lang in [Lang::En, Lang::Fr] {
            let (r, out) = ask_yes_no("maybe\no\n", false, lang);
            assert_eq!(r, Some(true));
            assert_eq!(out.matches(lang::answer_y_or_n(lang)).count(), 1);
            for (s, want) in [
                ("o", true),
                ("oui", true),
                ("y", true),
                ("yes", true),
                ("n", false),
                ("non", false),
                ("no", false),
                ("", true),
            ] {
                let (r, _) = ask_yes_no(&format!("{s}\n"), true, lang);
                assert_eq!(r, Some(want), "{lang:?} {s:?}");
            }
        }
    }

    #[test]
    fn yes_no_hint_follows_the_language() {
        assert!(ask_yes_no("\n", true, Lang::En).1.contains("Q [Y/n]: "));
        assert!(ask_yes_no("\n", false, Lang::En).1.contains("Q [y/N]: "));
        assert!(ask_yes_no("\n", true, Lang::Fr).1.contains("Q [O/n] : "));
        assert!(ask_yes_no("\n", false, Lang::Fr).1.contains("Q [o/N] : "));
    }

    #[test]
    fn yes_no_eof_is_none() {
        let r = yes_no(
            &mut Cursor::new(&b""[..]),
            &mut Vec::new(),
            "Q",
            true,
            Lang::En,
        )
        .unwrap();
        assert_eq!(r, None);
    }

    #[test]
    fn clean_path_strips_spaces_and_one_pair_of_quotes() {
        assert_eq!(
            clean_path("  \"C:\\a b\\x.p4k\"  ", None),
            PathBuf::from("C:\\a b\\x.p4k")
        );
        assert_eq!(clean_path("'x'", None), PathBuf::from("x"));
        assert_eq!(clean_path("\"x", None), PathBuf::from("\"x"));
    }

    #[test]
    fn clean_path_expands_tilde_only_at_start() {
        let h = Path::new("/h");
        assert_eq!(clean_path("~/a", Some(h)), Path::new("/h").join("a"));
        assert_eq!(clean_path("a~b", Some(h)), PathBuf::from("a~b"));
        assert_eq!(clean_path("~", Some(h)), PathBuf::from("/h"));
    }

    #[test]
    #[cfg(not(windows))]
    fn clean_path_keeps_an_escaped_tilde_literal() {
        let h = Path::new("/h");
        assert_eq!(clean_path("\\~/x", Some(h)), PathBuf::from("~/x"));
    }

    #[test]
    fn clean_path_unescapes_backslash_space_outside_windows() {
        let want = if cfg!(windows) {
            "/a/My\\ Folder/x"
        } else {
            "/a/My Folder/x"
        };
        assert_eq!(clean_path("/a/My\\ Folder/x", None), PathBuf::from(want));
    }

    #[test]
    fn clean_path_unescapes_shell_special_characters_outside_windows() {
        let cases = [
            ("/a/Data\\ \\(1\\).p4k", "/a/Data (1).p4k"),
            ("a\\\\b", "a\\b"),
            ("a\\xb", "a\\xb"),
        ];
        for (raw, unix) in cases {
            let want = if cfg!(windows) { raw } else { unix };
            assert_eq!(clean_path(raw, None), PathBuf::from(want), "{raw}");
        }
    }
}
