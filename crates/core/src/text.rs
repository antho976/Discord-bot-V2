//! Small text helpers shared by features.

/// Replace `{name}` placeholders. Unknown placeholders are left as they are.
pub fn fill(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = template.to_owned();
    for (k, v) in vars {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

/// Cut to at most `max` characters, ending with an ellipsis when cut.
pub fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// "3 min ago", "2 h ago", "5 days ago" for a past time in epoch milliseconds.
pub fn ago(ms: i64) -> String {
    let secs = ((crate::now_ms() - ms) / 1000).max(0);
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86400 => format!("{} h ago", secs / 3600),
        86400..172800 => "yesterday".into(),
        _ => format!("{} days ago", secs / 86400),
    }
}

/// "1 h 25 min" for a duration in seconds.
pub fn duration(secs: i64) -> String {
    let (h, m) = (secs / 3600, (secs % 3600) / 60);
    match (h, m) {
        (0, 0) => format!("{secs} s"),
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// Parse "10m", "2h", "1d 12h", "90" (minutes) into seconds.
pub fn parse_duration(s: &str) -> Option<i64> {
    let s = s.trim().to_lowercase();
    if let Ok(n) = s.parse::<i64>() {
        return Some(n * 60);
    }
    let mut total = 0i64;
    let mut num = String::new();
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            num.push(ch);
        } else if ch.is_whitespace() {
            continue;
        } else {
            let n: i64 = num.parse().ok()?;
            num.clear();
            total += n * match ch {
                's' => 1,
                'm' => 60,
                'h' => 3600,
                'd' => 86400,
                'w' => 604800,
                _ => return None,
            };
        }
    }
    if !num.is_empty() {
        return None;
    }
    (total > 0).then_some(total)
}

/// Thousands separators: 12345 -> "12,345".
pub fn num(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_parse() {
        assert_eq!(parse_duration("10m"), Some(600));
        assert_eq!(parse_duration("1d 12h"), Some(129600));
        assert_eq!(parse_duration("90"), Some(5400));
        assert_eq!(parse_duration("abc"), None);
        assert_eq!(parse_duration("5x"), None);
    }

    #[test]
    fn numbers_group() {
        assert_eq!(num(12345), "12,345");
        assert_eq!(num(999), "999");
        assert_eq!(num(-1000), "-1,000");
    }
}
