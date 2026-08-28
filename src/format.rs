//! Turning numbers into something readable at a glance.

use std::time::Duration;

/// Binary byte sizes: `4.2 GiB`.
pub fn bytes(value: u64) -> String {
    let (number, unit) = bytes_parts(value);
    format!("{number} {unit}")
}

/// A byte size split into its number and its unit, for layouts that set the
/// two in different sizes or styles.
pub fn bytes_parts(value: u64) -> (String, &'static str) {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];

    let mut value = value as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }

    let number = match unit {
        0 => format!("{value:.0}"),
        // One decimal is enough to tell 4.2 GiB from 4.3 GiB, and the extra
        // digits only make the number harder to read across a row.
        _ if value >= 100.0 => format!("{value:.0}"),
        _ => format!("{value:.1}"),
    };

    (number, UNITS[unit])
}

/// Compact durations: `3d 4h`, `12m 30s`, `450ms`.
pub fn duration(value: Duration) -> String {
    let total = value.as_secs();
    let (days, hours, minutes, seconds) = (
        total / 86_400,
        (total % 86_400) / 3_600,
        (total % 3_600) / 60,
        total % 60,
    );

    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else if total > 0 {
        format!("{seconds}s")
    } else {
        format!("{}ms", value.as_millis())
    }
}

/// A duration rounded to one unit, for axis labels: `3m`, `2h`, `45s`.
pub fn span(seconds: f64) -> String {
    let seconds = seconds.max(0.0).round() as u64;
    match seconds {
        0..=90 => format!("{seconds}s"),
        91..=5_400 => format!("{}m", (seconds as f64 / 60.0).round() as u64),
        _ => format!("{}h", (seconds as f64 / 3_600.0).round() as u64),
    }
}

/// How long ago something happened: `just now`, `12s ago`, `4m ago`.
pub fn ago(value: Duration) -> String {
    match value.as_secs() {
        0..=2 => "just now".into(),
        seconds => format!("{} ago", duration(Duration::from_secs(seconds))),
    }
}

/// A fraction as a percentage: `6.4%`.
pub fn percent(fraction: f64) -> String {
    let value = fraction * 100.0;
    if value >= 100.0 {
        format!("{value:.0}%")
    } else {
        format!("{value:.1}%")
    }
}

/// A large count with thousands separators: `1 284 302`.
pub fn count(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push('\u{202f}');
        }
        out.push(digit);
    }
    out
}

/// A value that may be missing, rendered with an em dash when it is.
pub fn optional<T>(value: Option<T>, render: impl FnOnce(T) -> String) -> String {
    value.map_or_else(|| "—".to_owned(), render)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_byte_sizes() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(999), "999 B");
        assert_eq!(bytes(1024), "1.0 KiB");
        assert_eq!(bytes(4_509_715_660), "4.2 GiB");
        assert_eq!(bytes(500 * 1024 * 1024), "500 MiB");
    }

    #[test]
    fn byte_parts_agree_with_the_joined_form() {
        assert_eq!(bytes_parts(0), ("0".into(), "B"));
        assert_eq!(bytes_parts(4_509_715_660), ("4.2".into(), "GiB"));
        let (number, unit) = bytes_parts(1024);
        assert_eq!(format!("{number} {unit}"), bytes(1024));
    }

    #[test]
    fn abbreviates_durations() {
        assert_eq!(duration(Duration::from_millis(450)), "450ms");
        assert_eq!(duration(Duration::from_secs(45)), "45s");
        assert_eq!(duration(Duration::from_secs(750)), "12m 30s");
        assert_eq!(duration(Duration::from_secs(3 * 3600 + 240)), "3h 4m");
        assert_eq!(
            duration(Duration::from_secs(3 * 86_400 + 4 * 3600)),
            "3d 4h"
        );
    }

    #[test]
    fn axis_spans_round_to_a_single_unit() {
        assert_eq!(span(45.0), "45s");
        assert_eq!(span(163.0), "3m");
        assert_eq!(span(7_200.0), "2h");
        assert_eq!(span(-5.0), "0s");
    }

    #[test]
    fn describes_how_long_ago() {
        assert_eq!(ago(Duration::from_secs(1)), "just now");
        assert_eq!(ago(Duration::from_secs(12)), "12s ago");
        assert_eq!(ago(Duration::from_secs(240)), "4m 0s ago");
    }

    #[test]
    fn renders_percentages() {
        assert_eq!(percent(0.064), "6.4%");
        assert_eq!(percent(1.0), "100%");
        assert_eq!(percent(0.0), "0.0%");
    }

    #[test]
    fn groups_digits() {
        assert_eq!(count(7), "7");
        assert_eq!(count(1_284_302), "1\u{202f}284\u{202f}302");
    }

    #[test]
    fn missing_values_read_as_a_dash() {
        assert_eq!(optional(None::<u64>, bytes), "—");
        assert_eq!(optional(Some(1024), bytes), "1.0 KiB");
    }
}
