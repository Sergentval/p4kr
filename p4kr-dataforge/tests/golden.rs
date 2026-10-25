//! Every case of `golden.json` that `Db::open` accepts: same paths in the same order as the reference,
//! and for each exactly its bytes (`written`), no file (`skipped`), or the deliberate deviations:
//! dangerous path refused (`UnsafePath`), output with a call stack (only the marker compared), first
//! `aborted` record (`BadName`). Every mismatch is listed together.

use p4kr_dataforge::{Db, RecordError, has_trace, safe_relative};

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).expect("hex"))
        .collect()
}

struct Expected {
    path: String,
    outcome: String,
    xml: Option<Vec<u8>>,
}

struct Case {
    name: String,
    input: Vec<u8>,
    records: Vec<Expected>,
}

fn cases() -> Vec<Case> {
    let json: serde_json::Value =
        serde_json::from_str(include_str!("golden.json")).expect("golden.json");
    let records = |c: &serde_json::Value| {
        c["records"]
            .as_array()
            .expect("records")
            .iter()
            .map(|r| Expected {
                path: r["path"].as_str().expect("path").to_string(),
                outcome: r["outcome"].as_str().expect("outcome").to_string(),
                xml: r["xml_hex"].as_str().map(unhex),
            })
            .collect()
    };
    json.as_array()
        .expect("array")
        .iter()
        .map(|c| Case {
            name: c["name"].as_str().expect("name").to_string(),
            input: unhex(c["input_hex"].as_str().expect("input_hex")),
            records: records(c),
        })
        .collect()
}

/// Marker of a call stack written in an attribute (that of `has_trace`).
const TRACE_MARK: &[u8] = b"&#xA;   at ";

/// The XML without the call-stack text: from each marker up to the quote closing the attribute.
fn without_traces(xml: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(at) = rest.windows(TRACE_MARK.len()).position(|w| w == TRACE_MARK) {
        out.extend_from_slice(&rest[..at]);
        let tail = &rest[at..];
        rest = &tail[tail.iter().position(|&b| b == b'"').unwrap_or(tail.len())..];
    }
    out.extend_from_slice(rest);
    out
}

#[test]
fn traces_are_cut_up_to_the_closing_quote() {
    let a = without_traces(b"<a v=\"E: m&#xA;   at X.Y()&#xA;   at Z\" w=\"1\" />");
    let b = without_traces(b"<a v=\"E: m&#xA;   at p4kr\" w=\"1\" />");
    assert_eq!(a, b"<a v=\"E: m\" w=\"1\" />");
    assert_eq!(a, b);
}

/// Compares an output; `Err`: mismatch, `Ok(Some)`: deliberate deviation, noted.
fn judge(
    want: &Expected,
    got: &Result<Option<Vec<u8>>, RecordError>,
    first_abort: bool,
) -> Result<Option<String>, String> {
    let shown = |g: &Result<Option<Vec<u8>>, RecordError>| match g {
        Ok(Some(x)) => String::from_utf8_lossy(x).into_owned(),
        other => format!("{other:?}"),
    };
    if safe_relative(&want.path).is_none() {
        return match got {
            Err(RecordError::UnsafePath) => Ok(Some(format!("path refused ({})", want.outcome))),
            other => Err(format!("dangerous path not refused: {}", shown(other))),
        };
    }
    match (want.outcome.as_str(), &want.xml, got) {
        ("written", Some(x), Ok(Some(y))) if has_trace(x) => {
            if !has_trace(y) {
                return Err(format!("call stack expected, got: {}", shown(got)));
            }
            match without_traces(x) == without_traces(y) {
                true => Ok(Some("call stack on both sides, rest identical".into())),
                false => Err(format!("outside the call stack, got: {}", shown(got))),
            }
        }
        ("written", Some(x), Ok(Some(y))) if x == y => Ok(None),
        ("skipped", _, Ok(None)) => Ok(None),
        ("aborted", _, Err(RecordError::BadName)) if first_abort => {
            Ok(Some("reference abort: BadName".into()))
        }
        ("aborted", _, _) if !first_abort => Ok(Some(format!("after an abort: {}", shown(got)))),
        _ => Err(format!(
            "expected {} {:?}, got {}",
            want.outcome,
            want.xml.as_deref().map(String::from_utf8_lossy),
            shown(got)
        )),
    }
}

#[test]
fn every_accepted_case_matches_the_reference() {
    let (mut faults, mut notes, mut rendered) = (Vec::new(), Vec::new(), 0);
    for case in cases() {
        let Ok(db) = Db::open(&case.input) else {
            continue;
        };
        let outputs = db.outputs();
        let paths: Vec<&str> = outputs.iter().map(|o| o.path.as_str()).collect();
        let want: Vec<&str> = case.records.iter().map(|r| r.path.as_str()).collect();
        if paths != want {
            faults.push(format!(
                "{}: paths {paths:?} instead of {want:?}",
                case.name
            ));
            continue;
        }
        let mut first_abort = true;
        for (o, expected) in outputs.iter().zip(&case.records) {
            let got = db.render(o);
            match judge(expected, &got, first_abort) {
                Ok(None) => {}
                Ok(Some(note)) => notes.push(format!("{} {}: {note}", case.name, o.path)),
                Err(e) => faults.push(format!("{} {}: {e}", case.name, o.path)),
            }
            first_abort &= expected.outcome != "aborted";
            rendered += 1;
        }
    }
    for n in &notes {
        println!("noted: {n}");
    }
    assert!(
        faults.is_empty(),
        "{} mismatch(es):\n{}",
        faults.len(),
        faults.join("\n")
    );
    assert!(rendered >= 40, "{rendered} outputs compared");
}

#[test]
fn safe_relative_refuses_only_escaping_paths() {
    for bad in ["", "/abs/x.xml", "../up.xml", "a/../../b.xml", "a/.."] {
        assert!(safe_relative(bad).is_none(), "{bad}");
    }
    for ok in [
        "a.xml",
        "lib0/a/b.xml",
        "lib0\\x\\..\\y.xml",
        "a/..b/c.xml",
        "./a.xml",
    ] {
        assert!(safe_relative(ok).is_some(), "{ok}");
    }
}

#[test]
fn trace_mark_is_the_reference_marker() {
    assert!(has_trace(b"<a v=\"x&#xA;   at y\" />"));
    assert!(!has_trace(b"<a v=\"x at y\" />"));
}
