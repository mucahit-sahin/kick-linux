//! Formatting helpers (without adding dependencies).

/// 1234 -> "1.2K", 1_500_000 -> "1.5M"
pub fn human_count(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => {
            let v = n as f64 / 1_000.0;
            if v >= 100.0 {
                format!("{v:.0}K")
            } else if v >= 10.0 {
                format!("{v:.1}K")
            } else {
                format!("{v:.1}K")
            }
        }
        _ => {
            let v = n as f64 / 1_000_000.0;
            if v >= 100.0 {
                format!("{v:.0}M")
            } else {
                format!("{v:.1}M")
            }
        }
    }
}

/// Seconds -> "1:02:03" or "12:34"
pub fn duration(secs: f64) -> String {
    let total = secs.max(0.0) as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Bytes -> "1.2 MB", "540 kB", ...
pub fn size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let value = bytes as f64;
    if value < KB {
        format!("{bytes} B")
    } else if value < KB * KB {
        format!("{:.0} kB", value / KB)
    } else if value < KB * KB * KB {
        format!("{:.1} MB", value / (KB * KB))
    } else {
        format!("{:.2} GB", value / (KB * KB * KB))
    }
}

/// Shortens long titles for the window title.
pub fn shorten(text: &str, max_chars: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Converts Kick's "2026-11-13 22:04:01" (UTC) format to a relative time.
pub fn relative_time(stamp: &str) -> String {
    relative_time_lang(stamp, false)
}

pub fn relative_time_lang(stamp: &str, english: bool) -> String {
    match parse_utc(stamp) {
        Some(then) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let diff = now.saturating_sub(then);
            if diff < 0 {
                return stamp[..10.min(stamp.len())].to_string();
            }
            match diff {
                0..=59 => if english { "just now" } else { "az önce" }.to_string(),
                60..=3599 => if english { format!("{} minutes ago", diff / 60) } else { format!("{} dakika önce", diff / 60) },
                3600..=86_399 => if english { format!("{} hours ago", diff / 3600) } else { format!("{} saat önce", diff / 3600) },
                86_400..=172_799 => if english { "yesterday" } else { "dün" }.to_string(),
                172_800..=2_591_999 => if english { format!("{} days ago", diff / 86_400) } else { format!("{} gün önce", diff / 86_400) },
                _ => {
                    // DD.MM.YYYY
                    let date = &stamp[..10.min(stamp.len())];
                    let parts: Vec<&str> = date.split('-').collect();
                    if parts.len() == 3 {
                        format!("{}.{}.{}", parts[2], parts[1], parts[0])
                    } else {
                        date.to_string()
                    }
                }
            }
        }
        None => stamp.to_string(),
    }
}

/// "2026-11-13 22:04:01" -> non-local (UTC) Unix time
fn parse_utc(s: &str) -> Option<i64> {
    let bytes = s.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let y: i64 = s.get(0..4)?.parse().ok()?;
    let mo: i64 = s.get(5..7)?.parse().ok()?;
    let d: i64 = s.get(8..10)?.parse().ok()?;
    let h: i64 = s.get(11..13)?.parse().ok()?;
    let mi: i64 = s.get(14..16)?.parse().ok()?;
    let sec: i64 = s.get(17..19)?.parse().ok()?;
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + sec)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_counts() {
        assert_eq!(human_count(12), "12");
        assert_eq!(human_count(1_234), "1.2K");
        assert_eq!(human_count(1_500_000), "1.5M");
    }

    #[test]
    fn formats_durations() {
        assert_eq!(duration(65.0), "1:05");
        assert_eq!(duration(3723.0), "1:02:03");
    }

    #[test]
    fn shortens_titles() {
        assert_eq!(shorten("xqc", 10), "xqc");
        assert_eq!(shorten("abcdefghij", 5), "abcd…");
    }

    #[test]
    fn parses_dates() {
        // 2026-11-13 22:04:01 UTC = 1794607441
        assert_eq!(parse_utc("2026-11-13 22:04:01"), Some(1_794_607_441));
    }
}
