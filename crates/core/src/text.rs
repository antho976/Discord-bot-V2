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

/// Make text from outside (a clip title, a name someone chose) show as typed in a Discord
/// message: markdown characters get a backslash, so "[Free nitro](https://x)" can't become a
/// masked link, and a line can't start a list, a heading or a quote.
pub fn escape_markdown(s: &str) -> String {
    const SPECIAL: &[char] = &['\\', '[', ']', '(', ')', '*', '_', '~', '`', '|', '>', '#'];
    let mut out = String::with_capacity(s.len() + 8);
    for line in s.split_inclusive('\n') {
        let body = line.trim_start_matches([' ', '\t']);
        out.push_str(&line[..line.len() - body.len()]);
        let mut rest = body;
        let digits = body.len() - body.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if body.starts_with(['-', '+']) && body[1..].starts_with([' ', '\t']) {
            // "- item" or "+ item"
            out.push('\\');
            out.push_str(&body[..1]);
            rest = &body[1..];
        } else if digits > 0 && body[digits..].starts_with(". ") {
            // "1. item"
            out.push_str(&body[..digits]);
            out.push_str("\\.");
            rest = &body[digits + 1..];
        }
        for c in rest.chars() {
            if SPECIAL.contains(&c) {
                out.push('\\');
            }
            out.push(c);
        }
    }
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

/// "today", "yesterday", "12 days ago", "5 months ago", "3 years ago": how long ago something
/// was made, such as an account (`made_ago(discord::snowflake_ms(user))`).
pub fn made_ago(ms: i64) -> String {
    let days = ((crate::now_ms() - ms) / 86_400_000).max(0);
    match days {
        0 => "today".into(),
        1 => "yesterday".into(),
        2..60 => format!("{days} days ago"),
        60..730 => format!("{} months ago", days / 30),
        _ => format!("{} years ago", days / 365),
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

/// Parse "10m", "2h", "1d 12h", "90" (minutes) into seconds. Zero, negative and absurdly
/// large spans are None.
pub fn parse_duration(s: &str) -> Option<i64> {
    let s = s.trim().to_lowercase();
    if let Ok(n) = s.parse::<i64>() {
        return n.checked_mul(60).filter(|t| *t > 0);
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
            let unit = match ch {
                's' => 1,
                'm' => 60,
                'h' => 3600,
                'd' => 86400,
                'w' => 604800,
                _ => return None,
            };
            total = total.checked_add(n.checked_mul(unit)?)?;
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
        if i > 0 && (s.len() - i).is_multiple_of(3) {
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
        assert_eq!(parse_duration("0"), None);
        assert_eq!(parse_duration("-5"), None);
        assert_eq!(parse_duration("99999999999999999d"), None);
    }

    #[test]
    fn ages_read_in_the_right_unit() {
        let day = 86_400_000;
        let now = crate::now_ms();
        assert_eq!(made_ago(now - 1000), "today");
        assert_eq!(made_ago(now - day - 1000), "yesterday");
        assert_eq!(made_ago(now - 12 * day), "12 days ago");
        assert_eq!(made_ago(now - 150 * day), "5 months ago");
        assert_eq!(made_ago(now - 1100 * day), "3 years ago");
        assert_eq!(made_ago(now + day), "today");
        // 2020-01-01T00:00:00Z as a Discord id.
        assert_eq!(crate::discord::snowflake_ms((1_577_836_800_000 - 1_420_070_400_000) << 22), 1_577_836_800_000);
    }

    #[test]
    fn markdown_is_shown_as_typed() {
        assert_eq!(escape_markdown("Gold stake"), "Gold stake");
        assert_eq!(escape_markdown("[Free nitro](https://evil.example)"), r"\[Free nitro\]\(https://evil.example\)");
        assert_eq!(escape_markdown("**big** _it_ ~~no~~ `code` ||spoiler||"), r"\*\*big\*\* \_it\_ \~\~no\~\~ \`code\` \|\|spoiler\|\|");
        assert_eq!(escape_markdown(r"a\b"), r"a\\b");
        assert_eq!(escape_markdown("# Big"), r"\# Big");
        assert_eq!(escape_markdown("-# small"), r"-\# small");
        assert_eq!(escape_markdown("> quoted"), r"\> quoted");
        // List markers only at the start of a line, and only when they start a list.
        assert_eq!(escape_markdown("- item"), r"\- item");
        assert_eq!(escape_markdown("  + item"), r"  \+ item");
        assert_eq!(escape_markdown("1. first\n2. second"), "1\\. first\n2\\. second");
        assert_eq!(escape_markdown("a - b + c"), "a - b + c");
        assert_eq!(escape_markdown("-5 hp, 2.5x"), "-5 hp, 2.5x");
        assert_eq!(escape_markdown("x_y\n- z"), "x\\_y\n\\- z");
        assert_eq!(escape_markdown(""), "");
    }

    #[test]
    fn numbers_group() {
        assert_eq!(num(12345), "12,345");
        assert_eq!(num(999), "999");
        assert_eq!(num(-1000), "-1,000");
    }
}
