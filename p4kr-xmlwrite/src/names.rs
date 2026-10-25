//! XML names as the reference checks them (`CreateElement` / `CreateAttribute`) and writes them (`Save`,
//! `XmlWriter`).

mod chars;

use chars::{NAME_CHARS, START_CHARS};

/// Split at the first `:` if it is neither leading nor trailing: (prefix, local name), empty prefix otherwise.
pub fn split_name(name: &str) -> (&str, &str) {
    match name.find(':') {
        Some(i) if i > 0 && i + 1 < name.len() => (&name[..i], &name[i + 1..]),
        _ => ("", name),
    }
}

/// Non-empty local name; prefix and local name made of name characters without `:` (no first-character
/// rule in the reference's check).
pub fn is_valid_name(name: &str) -> bool {
    let (prefix, local) = split_name(name);
    !local.is_empty() && prefix.chars().chain(local.chars()).all(is_name_char)
}

/// Written element name: created without a namespace, it loses its prefix, whatever it is.
pub fn element_name(name: &str) -> &str {
    split_name(name).1
}

/// Written attribute name: the reserved prefixes `xml` and `xmlns` get their namespace and stay;
/// any other prefix disappears.
pub fn attribute_name(name: &str) -> &str {
    match split_name(name) {
        ("xml" | "xmlns", _) => name,
        (_, local) => local,
    }
}

fn is_name_char(c: char) -> bool {
    in_table(NAME_CHARS, c)
}

fn is_start_char(c: char) -> bool {
    in_table(START_CHARS, c)
}

fn in_table(table: &[(u16, u16)], c: char) -> bool {
    let Ok(u) = u16::try_from(u32::from(c)) else {
        return false;
    };
    table
        .binary_search_by(|&(lo, hi)| {
            if hi < u {
                std::cmp::Ordering::Less
            } else if lo > u {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameKind {
    Element,
    Attribute,
}

/// Failure of the reference on a name, with its .NET message. `Create`: `XmlDocument.CreateElement` /
/// `CreateAttribute`; `Write`: `XmlWriter`, later, when the document is written.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum NameFault {
    #[error("{0}")]
    Create(String),
    #[error("{0}")]
    Write(String),
}

const EMPTY_NAMESPACE: &str = "Cannot use a prefix with an empty namespace.";

/// The reference's first failure on this name alone, `None` if it passes both stages. Attribute values
/// add others: see `dotnet_attribute_fault`.
pub fn dotnet_name_fault(name: &str, kind: NameKind) -> Option<NameFault> {
    if plain_ascii(name) {
        return None;
    }
    create_fault(name, kind)
        .map(NameFault::Create)
        .or_else(|| write_fault(name, kind).map(NameFault::Write))
}

/// Ordinary ASCII name (letter or `_`, then `[A-Za-z0-9_.-]`): valid at both stages, without the tables.
fn plain_ascii(name: &str) -> bool {
    let mut b = name.bytes();
    matches!(b.next(), Some(c) if c.is_ascii_alphabetic() || c == b'_')
        && b.all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
}

/// `dotnet_name_fault` then the write failures that depend on the value: redefined `xmlns`, `xmlns:`
/// prefix without a namespace.
pub fn dotnet_attribute_fault(name: &str, value: &str) -> Option<NameFault> {
    dotnet_name_fault(name, NameKind::Attribute).or_else(|| {
        if name == "xmlns" && !value.is_empty() {
            Some(NameFault::Write(format!(
                "The prefix '' cannot be redefined from '' to '{value}' within the same start element tag."
            )))
        } else if split_name(name).0 == "xmlns" && value.is_empty() {
            Some(NameFault::Write(EMPTY_NAMESPACE.to_string()))
        } else {
            None
        }
    })
}

fn char_message(c: char) -> String {
    format!(
        "The '{c}' character, hexadecimal value 0x{:02X}, cannot be included in a name.",
        u32::from(c)
    )
}

/// Creation: non-empty name made of name characters, with at most one inner `:` (no first-character
/// rule).
fn create_fault(name: &str, kind: NameKind) -> Option<String> {
    if name.is_empty() {
        return Some(match kind {
            NameKind::Element => {
                "The local name for elements or attributes cannot be null or an empty string."
                    .into()
            }
            NameKind::Attribute => "The attribute local name cannot be empty.".into(),
        });
    }
    let colon = split_name(name).0.len();
    name.char_indices()
        .find(|&(i, c)| !(is_name_char(c) || (colon > 0 && i == colon)))
        .map(|(_, c)| char_message(c))
}

/// Writing: prefix then local name must start with a letter or `_`; an element keeps no
/// prefix (no namespace).
fn write_fault(name: &str, kind: NameKind) -> Option<String> {
    let (prefix, local) = split_name(name);
    for part in [prefix, local].into_iter().filter(|p| !p.is_empty()) {
        if let Some(c) = part.chars().next().filter(|&c| !is_start_char(c)) {
            return Some(format!(
                "Invalid name character in '{part}'. {}",
                char_message(c)
            ));
        }
    }
    (kind == NameKind::Element && !prefix.is_empty()).then(|| EMPTY_NAMESPACE.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ascii_fast_path_agrees_with_the_tables() {
        for (kind, code) in [NameKind::Element, NameKind::Attribute]
            .into_iter()
            .flat_map(|k| (0u8..=127).map(move |c| (k, char::from(c))))
        {
            for (pre, post) in [("", ""), ("", "x"), ("a", ""), ("a", "b")] {
                let name = format!("{pre}{code}{post}");
                let tables_accept =
                    create_fault(&name, kind).is_none() && write_fault(&name, kind).is_none();
                assert!(!plain_ascii(&name) || tables_accept, "{name:?} {kind:?}");
            }
        }
        assert!(plain_ascii("Ab_9.-") && !plain_ascii("9a") && !plain_ascii("a:b"));
    }

    #[test]
    fn names_split_at_the_first_inner_colon() {
        assert_eq!(split_name("p:Root"), ("p", "Root"));
        assert_eq!(split_name("Root"), ("", "Root"));
        assert_eq!(split_name("a:b:c"), ("a", "b:c"));
        assert_eq!(split_name(":a"), ("", ":a"));
        assert_eq!(split_name("a:"), ("", "a:"));
    }

    #[test]
    fn written_names_follow_the_reference() {
        assert_eq!(element_name("p:Root"), "Root");
        assert_eq!(element_name("xml:foo"), "foo");
        assert_eq!(element_name("xmlns:foo"), "foo");
        assert_eq!(attribute_name("p:q"), "q");
        assert_eq!(attribute_name("xml:lang"), "xml:lang");
        assert_eq!(attribute_name("xmlns:foo"), "xmlns:foo");
        assert_eq!(attribute_name("xmlns"), "xmlns");
    }

    #[test]
    fn names_follow_the_reference_checks() {
        for ok in [
            "Root",
            "1abc",
            "a-b.c",
            "p:Root",
            "Élément", // check-french: allow Unicode test input
            "x_y",
            "a\u{b7}",
            "\u{e01}",
            "\u{4e00}",
        ] {
            assert!(is_valid_name(ok), "{ok}");
        }
        for bad in [
            "",
            "a b",
            "a:",
            ":a",
            "a:b:c",
            "a<b",
            ":",
            "a\u{fffd}",
            "\u{132}",
            "\u{370}",
            "\u{2c00}",
            "\u{1f600}",
        ] {
            assert!(!is_valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn the_tables_are_sorted_and_disjoint() {
        for table in [NAME_CHARS, START_CHARS] {
            assert!(table.iter().all(|&(lo, hi)| lo <= hi));
            assert!(table.windows(2).all(|w| w[0].1 + 1 < w[1].0));
        }
    }

    /// Both tables against the exhaustive survey of the basic plane made with the reference (`tests/namechars.json`):
    /// every mismatch is listed.
    #[test]
    fn the_tables_match_the_exhaustive_reference_run() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/namechars.json")).expect("namechars.json");
        let mut failures = Vec::new();
        for (key, accepts) in [
            ("name_chars", is_name_char as fn(char) -> bool),
            ("start_chars", is_start_char as fn(char) -> bool),
        ] {
            let mut want = vec![false; 0x10000];
            for range in fixture[key].as_array().expect("ranges") {
                let (lo, hi) = (
                    range[0].as_u64().expect("lo"),
                    range[1].as_u64().expect("hi"),
                );
                (lo..=hi).for_each(|c| want[usize::try_from(c).expect("u16")] = true);
            }
            for (code, &expected) in want.iter().enumerate() {
                let c = u32::try_from(code).ok().and_then(char::from_u32);
                let got = c.is_some_and(accepts);
                if c.is_some() && got != expected {
                    failures.push(format!(
                        "{key} U+{code:04X}: expected {expected}, got {got}"
                    ));
                }
            }
        }
        assert!(
            failures.is_empty(),
            "{} mismatches:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    #[test]
    fn start_characters_are_name_characters() {
        let all = (0..=0xFFFFu32).filter_map(char::from_u32);
        assert!(all.filter(|&c| is_start_char(c)).all(is_name_char));
        assert!(is_start_char('_') && is_start_char('é')); // check-french: allow Unicode test input
        assert!(!is_start_char('1') && !is_start_char('-') && !is_start_char('.'));
    }

    #[test]
    fn faults_carry_the_stage_of_the_reference() {
        use NameKind::{Attribute, Element};
        let is_create = |n, k| dotnet_name_fault(n, k).map(|f| matches!(f, NameFault::Create(_)));
        assert_eq!(is_create("a b", Element), Some(true));
        assert_eq!(is_create("a:b:c", Attribute), Some(true));
        assert_eq!(is_create("1a", Element), Some(false));
        assert_eq!(is_create("a:b", Element), Some(false));
        assert_eq!(is_create("a:b", Attribute), None);
        assert_eq!(
            dotnet_name_fault("", Attribute),
            Some(NameFault::Create(
                "The attribute local name cannot be empty.".into()
            ))
        );
    }
}
