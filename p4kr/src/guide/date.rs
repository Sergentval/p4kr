//! Today's date in UTC, with no time-zone dependency.

/// Days since 1970-01-01 → (year, month, day), proleptic Gregorian calendar.
fn date_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

pub fn today_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (y, m, d) = date_from_days((secs / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_from_days_matches_known_dates() {
        assert_eq!(date_from_days(0), (1970, 1, 1));
        assert_eq!(date_from_days(19_723), (2024, 1, 1));
        assert_eq!(date_from_days(20_750), (2026, 10, 24));
        assert_eq!(date_from_days(11_016), (2000, 2, 29));
    }
}
