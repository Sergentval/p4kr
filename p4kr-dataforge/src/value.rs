//! Value formatting like .NET's `ToString()` (invariant culture). The thresholds and forms are
//! those surveyed by the probes (`tests/formats.json`): shortest digits that round-trip,
//! fixed notation while the decimal exponent stays within the type's window, otherwise `d.dddE±XX`.

/// Window of fixed notation, as the decimal exponent of the first digit: `FIXED_LOW..=fixed_high`.
const FIXED_LOW: i32 = -4;
const F32_FIXED_HIGH: i32 = 8;
const F64_FIXED_HIGH: i32 = 16;

pub fn f32_text(v: f32) -> String {
    float_text(v, F32_FIXED_HIGH)
}

pub fn f64_text(v: f64) -> String {
    float_text(v, F64_FIXED_HIGH)
}

trait Float: Copy + PartialEq + std::fmt::LowerExp + std::str::FromStr {
    fn special(self) -> Option<&'static str>;
    fn negative(self) -> bool;
}

macro_rules! float_impl {
    ($t:ty) => {
        impl Float for $t {
            fn special(self) -> Option<&'static str> {
                if self.is_nan() {
                    Some("NaN")
                } else if self.is_infinite() {
                    Some("Infinity")
                } else if self == 0.0 {
                    Some("0")
                } else {
                    None
                }
            }
            fn negative(self) -> bool {
                self.is_sign_negative()
            }
        }
    };
}
float_impl!(f32);
float_impl!(f64);

/// Shortest digits that round-trip, in the form `d.ddde±x`. On a tie between two
/// candidates (value exactly halfway), the reference takes the even digit: exact rounding to `n`
/// digits gives it, where Rust's shortest rounds up.
fn shortest<T: Float>(v: T) -> String {
    let short = format!("{v:e}");
    let places = short
        .split_once('e')
        .map_or(0, |(m, _)| m.matches(|c: char| c.is_ascii_digit()).count());
    let exact = format!("{:.*e}", places.saturating_sub(1), v);
    if exact.parse::<T>().is_ok_and(|back| back == v) {
        exact
    } else {
        short
    }
}

fn float_text<T: Float>(v: T, fixed_high: i32) -> String {
    let sign = if v.negative() { "-" } else { "" };
    if let Some(word) = v.special() {
        // NaN has no sign; infinity and zero keep theirs.
        let sign = if word == "NaN" { "" } else { sign };
        return format!("{sign}{word}");
    }
    let text = shortest(v);
    let abs = text.trim_start_matches('-');
    let (mantissa, exp) = abs.split_once('e').unwrap_or((abs, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    let body = if (FIXED_LOW..=fixed_high).contains(&exp) {
        fixed(digits, exp)
    } else {
        scientific(digits, exp)
    };
    format!("{sign}{body}")
}

/// `digits` without trailing zero, `exp`: decimal exponent of the first digit.
fn fixed(digits: &str, exp: i32) -> String {
    if exp < 0 {
        let zeros = "0".repeat(usize::try_from(-exp - 1).unwrap_or(0));
        return format!("0.{zeros}{digits}");
    }
    let int_len = usize::try_from(exp).unwrap_or(0) + 1;
    if digits.len() <= int_len {
        format!("{digits}{}", "0".repeat(int_len - digits.len()))
    } else {
        format!("{}.{}", &digits[..int_len], &digits[int_len..])
    }
}

fn scientific(digits: &str, exp: i32) -> String {
    let (first, rest) = digits.split_at(1);
    let dot = if rest.is_empty() { "" } else { "." };
    let sign = if exp < 0 { '-' } else { '+' };
    format!("{first}{dot}{rest}E{sign}{:02}", exp.unsigned_abs())
}

/// The reference's GUID (`ReadGuid(…, false)`): each 8-byte half is read backwards, then formatted
/// 4-2-2-2-6 in lowercase.
pub fn guid_text(b: &[u8; 16]) -> String {
    let hex: String = b[..8]
        .iter()
        .rev()
        .chain(b[8..].iter().rev())
        .map(|x| format!("{x:02x}"))
        .collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_bytes_00_to_0f() {
        let b: [u8; 16] = std::array::from_fn(|i| u8::try_from(i).unwrap_or(0));
        assert_eq!(guid_text(&b), "07060504-0302-0100-0f0e-0d0c0b0a0908");
    }

    #[test]
    fn floats_follow_the_reference_forms() {
        assert_eq!(f32_text(1e9), "1E+09");
        assert_eq!(f32_text(1e8), "100000000");
        assert_eq!(f64_text(1e16), "10000000000000000");
        assert_eq!(f64_text(1e17), "1E+17");
        assert_eq!(f64_text(0.0001), "0.0001");
        assert_eq!(f64_text(0.00001), "1E-05");
        assert_eq!(f64_text(-0.0), "-0");
        assert_eq!(f32_text(f32::NAN), "NaN");
        assert_eq!(f32_text(f32::NEG_INFINITY), "-Infinity");
        assert_eq!(f64_text(123.456), "123.456");
    }
}
