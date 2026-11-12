use std::time::{SystemTime, UNIX_EPOCH};

/// Format a token count the way people say it out loud: `812`, `4.1k`, `74.5k`, `1.2M`.
pub fn fmt_tokens(n: f64) -> String {
    let n = n.max(0.0);
    if n < 1000.0 {
        format!("{}", n.round() as u64)
    } else if n < 1_000_000.0 {
        format!("{:.1}k", n / 1000.0)
    } else {
        format!("{:.1}M", n / 1_000_000.0)
    }
}

/// A horizontal bar `width` cells wide at most, scaled against `max`.
pub fn bar(value: f64, max: f64, width: usize) -> String {
    if max <= 0.0 || value <= 0.0 {
        return String::new();
    }
    let eighths = ((value / max) * (width * 8) as f64).round() as usize;
    let eighths = eighths.clamp(1, width * 8);
    let full = eighths / 8;
    let rest = eighths % 8;
    let mut s = "█".repeat(full);
    if rest > 0 {
        s.push(['▏', '▎', '▍', '▌', '▋', '▊', '▉'][rest - 1]);
    }
    s
}

pub fn plural<'a>(n: usize, one: &'a str, many: &'a str) -> &'a str {
    if n == 1 { one } else { many }
}

/// `YYYY-MM-DDTHH-MM-SS` in UTC, safe for file names.
pub fn timestamp_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, mo, d) = civil_from_days((secs / 86_400) as i64);
    let rem = secs % 86_400;
    format!(
        "{y:04}-{mo:02}-{d:02}T{:02}-{:02}-{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Shorten an ISO-8601 timestamp to `YYYY-MM-DD HH:MM`.
pub fn short_time(iso: &str) -> String {
    let s: String = iso.chars().take(16).collect();
    s.replace('T', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_human_readable() {
        assert_eq!(fmt_tokens(0.0), "0");
        assert_eq!(fmt_tokens(812.4), "812");
        assert_eq!(fmt_tokens(4_120.0), "4.1k");
        assert_eq!(fmt_tokens(74_535.0), "74.5k");
        assert_eq!(fmt_tokens(1_250_000.0), "1.2M");
    }

    #[test]
    fn bars_scale_and_never_vanish() {
        assert_eq!(bar(10.0, 10.0, 4), "████");
        assert_eq!(bar(5.0, 10.0, 4), "██");
        assert_eq!(bar(0.01, 10.0, 4), "▏");
        assert_eq!(bar(0.0, 10.0, 4), "");
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_404), (2025, 11, 12));
        assert_eq!(civil_from_days(11_017), (2000, 3, 1));
    }
}
