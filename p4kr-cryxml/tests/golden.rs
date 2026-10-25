//! Every synthetic case must give exactly the reference output (`tests/golden.json`, generated
//! by the maintainers from the reference implementation).

use p4kr_cryxml::{Outcome, convert};

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn label(o: Outcome) -> &'static str {
    match o {
        Outcome::Converted => "converted",
        Outcome::Text => "text",
        Outcome::Other => "other",
        Outcome::Failed => "failed",
    }
}

#[test]
fn every_case_matches_the_reference_byte_for_byte() {
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("golden.json")).expect("golden.json");
    let cases = golden.as_array().expect("list of cases");
    assert!(cases.len() >= 45, "only {} cases", cases.len());
    let mut failures = Vec::new();
    for case in cases {
        let name = case["name"].as_str().expect("name");
        let got = convert(&unhex(case["input_hex"].as_str().expect("input_hex")));
        let want = case["output_hex"].as_str().map(str::to_string);
        let have = got.xml.as_deref().map(hex);
        if case["outcome"] != label(got.outcome) || have != want {
            failures.push(format!(
                "{name}: expected {} {want:?}, got {} {have:?} ({:?})",
                case["outcome"],
                label(got.outcome),
                got.error
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatch(es):\n{}",
        failures.len(),
        failures.join("\n")
    );
}
