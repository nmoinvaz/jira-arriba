use anyhow::Result;
use chrono::{DateTime, Local};
use serde::Serialize;

/// Print a value as pretty JSON on stdout.
pub fn print_json<T: Serialize + ?Sized>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// Align rows into columns separated by two spaces. The last column is left unpadded.
pub fn table(rows: &[Vec<String>], indent: &str) -> String {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..cols)
        .map(|c| {
            rows.iter()
                .filter_map(|r| r.get(c))
                .map(|s| s.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    rows.iter()
        .map(|row| {
            let cells: Vec<String> = row
                .iter()
                .enumerate()
                .map(|(i, cell)| {
                    if i + 1 == row.len() {
                        cell.clone()
                    } else {
                        let pad = widths[i] - cell.chars().count();
                        format!("{cell}{}", " ".repeat(pad))
                    }
                })
                .collect();
            format!("{indent}{}", cells.join("  ").trim_end())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Byte count with a binary unit, as in `512 B` or `1.5 MB`.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// A span such as `45s`, `3m12s`, or `1h05m`. Negative spans, from clock skew, read as `0s`.
pub fn duration(seconds: i64) -> String {
    let s = seconds.max(0);
    let (h, m, s) = (s / 3600, s % 3600 / 60, s % 60);
    if h > 0 {
        format!("{h}h{m:02}m")
    } else if m > 0 {
        format!("{m}m{s:02}s")
    } else {
        format!("{s}s")
    }
}

/// A Jira timestamp, such as `2026-10-26T14:25:34.396-0700`, in local time without seconds.
/// Anything else is returned as is.
pub fn date(text: &str) -> String {
    parse_date(text)
        .map(|d| d.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| text.to_string())
}

/// Parse the timestamp formats Jira uses: RFC 3339, or its own with a zone lacking a colon.
pub fn parse_date(text: &str) -> Option<DateTime<chrono::FixedOffset>> {
    DateTime::parse_from_rfc3339(text)
        .or_else(|_| DateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f%z"))
        .ok()
}

/// Whether a field value looks like a Jira timestamp.
pub fn is_date(text: &str) -> bool {
    text.len() >= 19 && text.as_bytes()[10] == b'T' && parse_date(text).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_aligns_columns() {
        let rows = vec![
            vec!["D4T-3".to_string(), "a".to_string(), "x".to_string()],
            vec!["D4T-12".to_string(), "bbb".to_string(), "y".to_string()],
        ];
        assert_eq!(table(&rows, "  "), "  D4T-3   a    x\n  D4T-12  bbb  y");
    }

    #[test]
    fn sizes_use_binary_units() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(1023), "1023 B");
        assert_eq!(size(1536), "1.5 KB");
        assert_eq!(size(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration(45), "45s");
        assert_eq!(duration(192), "3m12s");
        assert_eq!(duration(3900), "1h05m");
        assert_eq!(duration(-3), "0s");
    }

    #[test]
    fn jira_timestamps_parse() {
        assert!(parse_date("2026-10-26T14:25:34.396-0700").is_some());
        assert!(parse_date("2026-10-27T19:19:51.438432-07:00").is_some());
        assert!(parse_date("2026-10-26").is_none());
        assert!(is_date("2026-10-26T14:25:34.396-0700"));
        assert!(!is_date("Nathan"));
        assert_eq!(date("not a date"), "not a date");
    }
}
