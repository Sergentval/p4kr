//! Summaries of extraction and DataForge commands: the lines to print and the exit code.

use crate::lang::{self, ExtractCounts, Lang};
use p4kr_core::Archive;
use p4kr_core::dcb::DcbReport;
use p4kr_core::extract::Report;

/// Summary of a command, ready to be shown.
pub struct Summary {
    /// Summary line (standard output).
    pub line: String,
    /// One line per error, refusal or failure (standard error), in the current order.
    pub problems: Vec<String>,
    /// One line per warning (standard error, with the language's warning prefix), without effect on `code`.
    pub notices: Vec<String>,
    /// Exit code: 1 if at least one entry, XML conversion or record failed, otherwise 0.
    pub code: u8,
}

/// Failure and I/O error lines of a `.dcb`, prefixed by `name`.
fn failed_lines(lang: Lang, name: &str, d: &DcbReport) -> Vec<String> {
    let failed = d
        .failed
        .iter()
        .map(|(digest, e)| lang::record_failed(lang, name, digest, e));
    let io = d
        .io_errors
        .iter()
        .map(|(digest, kind)| lang::record_io_error(lang, name, digest, kind));
    failed.chain(io).collect()
}

/// Warnings of a `.dcb`: an entry moved aside in favour of a record.
fn notice_lines(lang: Lang, d: &DcbReport) -> Vec<String> {
    d.displaced
        .iter()
        .map(|(from, to)| {
            lang::entry_displaced(lang, &from.display().to_string(), &to.display().to_string())
        })
        .collect()
}

/// Summary of an extraction or a verification.
pub fn extract_summary(a: &Archive, r: &Report, xml: bool, lang: Lang) -> Summary {
    let sep = lang::prompt_separator(lang);
    let mut problems = Vec::new();
    let mut notices = Vec::new();
    for (i, e) in &r.errors {
        problems.push(format!(
            "{}{sep}{}{sep}{e}",
            a.name(*i).unwrap_or("?"),
            e.kind()
        ));
    }
    for i in &r.xml_failed {
        problems.push(lang::xml_conversion_failed(lang, a.name(*i).unwrap_or("?")));
    }
    let mut records = 0;
    let mut dcb_failed = false;
    for (i, res) in &r.dcb {
        let name = a.name(*i).unwrap_or("?");
        match res {
            Ok(d) => {
                records += d.records;
                dcb_failed |= !d.failed.is_empty();
                problems.extend(failed_lines(lang, name, d));
                notices.extend(notice_lines(lang, d));
            }
            Err(e) => {
                dcb_failed = true;
                problems.push(lang::dataforge_refused(lang, name, e));
            }
        }
    }
    let line = lang::extract_line(
        lang,
        &ExtractCounts {
            entries: r.entries,
            bytes: r.bytes,
            errors: r.errors.len(),
            threads: r.threads,
            xml: xml.then_some((r.converted, r.xml_failed.len(), records)),
        },
    );
    let code = u8::from(!r.errors.is_empty() || !r.xml_failed.is_empty() || dcb_failed);
    Summary {
        line,
        problems,
        notices,
        code,
    }
}

/// Summary of `p4kr dcb`.
pub fn dcb_summary(d: &DcbReport, lang: Lang) -> Summary {
    Summary {
        line: lang::dcb_line(lang, d.records, d.failed.len(), d.bytes),
        problems: failed_lines(lang, lang::record_word(lang), d),
        notices: Vec::new(),
        code: u8::from(!d.failed.is_empty()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use p4kr_dataforge::RecordError;

    fn failing_dcb() -> DcbReport {
        DcbReport {
            records: 2,
            failed: vec![("ab12".to_string(), RecordError::Collision)],
            bytes: 10,
            io_errors: Vec::new(),
            displaced: Vec::new(),
        }
    }

    #[test]
    fn dcb_with_failure() {
        let s = dcb_summary(&failing_dcb(), Lang::Fr);
        assert_eq!(s.line, lang::dcb_line(Lang::Fr, 2, 1, 10));
        assert_eq!(s.problems.len(), 1);
        let prefix = format!(
            "{0}{1}{0} ab12{1}",
            lang::record_word(Lang::Fr),
            lang::prompt_separator(Lang::Fr)
        );
        assert!(s.problems[0].starts_with(&prefix), "{:?}", s.problems);
        assert_eq!(s.code, 1);
    }

    #[test]
    fn dcb_with_failure_in_english() {
        let s = dcb_summary(&failing_dcb(), Lang::En);
        assert_eq!(s.line, "2 records written, 1 failed, 10 bytes");
        assert_eq!(
            s.problems,
            ["record: record ab12: path already taken by another output"]
        );
        assert_eq!(s.code, 1);
    }

    #[test]
    fn dcb_without_failure() {
        let d = DcbReport {
            records: 2,
            bytes: 10,
            ..DcbReport::default()
        };
        for lang in [Lang::En, Lang::Fr] {
            let s = dcb_summary(&d, lang);
            assert!(s.problems.is_empty());
            assert_eq!(s.code, 0);
        }
    }

    #[test]
    fn io_errors_name_the_kind() {
        let d = DcbReport {
            io_errors: vec![("ab12".to_string(), std::io::ErrorKind::NotFound)],
            ..DcbReport::default()
        };
        let en = dcb_summary(&d, Lang::En).problems;
        assert_eq!(en, ["record: record ab12: I/O error (entity not found)"]);
        let fr = dcb_summary(&d, Lang::Fr).problems;
        assert_eq!(
            fr,
            [lang::record_io_error(
                Lang::Fr,
                lang::record_word(Lang::Fr),
                "ab12",
                &std::io::ErrorKind::NotFound
            )]
        );
    }

    fn displaced_report() -> Report {
        let mut r = empty_report();
        r.dcb.push((
            0,
            Ok(DcbReport {
                records: 1,
                displaced: vec![("o/a.xml".into(), "o/a.p4k-entry.xml".into())],
                ..DcbReport::default()
            }),
        ));
        r
    }

    #[test]
    fn a_displaced_entry_is_a_notice_and_not_a_failure() {
        let a = archive();
        let s = extract_summary(&a, &displaced_report(), true, Lang::Fr);
        assert_eq!(
            s.notices,
            [lang::entry_displaced(
                Lang::Fr,
                "o/a.xml",
                "o/a.p4k-entry.xml"
            )]
        );
        assert!(s.problems.is_empty());
        assert_eq!(s.code, 0);
        let s = extract_summary(&a, &displaced_report(), true, Lang::En);
        assert_eq!(
            s.notices,
            ["warning: o/a.xml: replaced by a DataForge record, entry kept as o/a.p4k-entry.xml"]
        );
        assert!(s.problems.is_empty());
        assert_eq!(s.code, 0);
    }

    fn empty_report() -> Report {
        Report {
            entries: 0,
            bytes: 0,
            threads: 1,
            errors: Vec::new(),
            converted: 0,
            xml_failed: Vec::new(),
            dcb: Vec::new(),
        }
    }

    fn archive() -> Archive {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/synth.p4k");
        Archive::open(&path).unwrap()
    }

    #[test]
    fn extract_empty_report() {
        let a = archive();
        let s = extract_summary(&a, &empty_report(), true, Lang::Fr);
        let counts = ExtractCounts {
            entries: 0,
            bytes: 0,
            errors: 0,
            threads: 1,
            xml: Some((0, 0, 0)),
        };
        assert_eq!(s.line, lang::extract_line(Lang::Fr, &counts));
        assert!(s.problems.is_empty());
        assert_eq!(s.code, 0);
    }

    #[test]
    fn extract_empty_report_in_english() {
        let a = archive();
        let s = extract_summary(&a, &empty_report(), false, Lang::En);
        assert_eq!(s.line, "0 entries, 0 bytes, 0 errors, 1 thread");
        assert!(s.problems.is_empty());
        assert_eq!(s.code, 0);
        let s = extract_summary(&a, &empty_report(), true, Lang::En);
        assert_eq!(
            s.line,
            "0 entries, 0 bytes, 0 errors, 1 thread, 0 converted to XML, 0 failed conversions, 0 DataForge records"
        );
    }

    #[test]
    fn the_error_kind_appears_once_in_the_report_line() {
        let a = archive();
        let mut r = empty_report();
        r.errors
            .push((0, p4kr_core::P4kError::DecompressError("bad frame".into())));
        let s = extract_summary(&a, &r, false, Lang::En);
        assert_eq!(s.problems.len(), 1);
        assert_eq!(s.problems[0].matches("zstd").count(), 1, "{:?}", s.problems);
        assert!(s.problems[0].contains("zstd: decompression: bad frame"));
    }
}
