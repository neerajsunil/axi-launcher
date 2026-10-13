//! Human-readable times, durations and counts.

use axi_core::util::now_unix;

/// "Never", "Just now", "5 minutes ago", "3 days ago", or a date for older times.
pub fn time_ago(ts: Option<u64>) -> String {
    let Some(ts) = ts else { return "Never".into() };
    let secs = now_unix().saturating_sub(ts);
    let plural = |n: u64, unit: &str| format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
    match secs {
        0..60 => "Just now".into(),
        60..3600 => plural(secs / 60, "minute"),
        3600..86_400 => plural(secs / 3600, "hour"),
        86_400..2_592_000 => plural(secs / 86_400, "day"),
        _ => date(ts),
    }
}

/// "45m", "3h 12m", or "0m".
pub fn duration(secs: u64) -> String {
    let (h, m) = (secs / 3600, secs % 3600 / 60);
    if h > 0 { format!("{h}h {m}m") } else { format!("{m}m") }
}

/// "950", "12.3K", "4.5M".
pub fn count(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}K", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
    .replace(".0", "")
}

/// `YYYY-MM-DD` (UTC) for a Unix timestamp.
pub fn date(ts: u64) -> String {
    // Howard Hinnant's days-to-civil algorithm.
    let z = (ts / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts() {
        assert_eq!(count(950), "950");
        assert_eq!(count(12_345), "12.3K");
        assert_eq!(count(4_000_000), "4M");
    }

    #[test]
    fn dates() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(1_709_251_200), "2024-03-01");
        assert_eq!(date(951_782_400), "2000-02-29");
    }

    #[test]
    fn durations() {
        assert_eq!(duration(0), "0m");
        assert_eq!(duration(59 * 60), "59m");
        assert_eq!(duration(3 * 3600 + 12 * 60 + 5), "3h 12m");
    }

    #[test]
    fn relative_times() {
        let now = now_unix();
        assert_eq!(time_ago(None), "Never");
        assert_eq!(time_ago(Some(now)), "Just now");
        assert_eq!(time_ago(Some(now - 120)), "2 minutes ago");
        assert_eq!(time_ago(Some(now - 3600)), "1 hour ago");
    }
}
