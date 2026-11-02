//! Reading what members write in their posts: an IdleonToolbox profile link, an answer that
//! means they're ready, chatter that doesn't help a review, and the dashboard's tags. All of it
//! is untrusted text, so nothing here slices a string at a byte index it didn't find itself.

const TOOLBOX_HOST: &str = "idleontoolbox.com";

/// The first IdleonToolbox link with a profile in `text`, rewritten as `https://` so it is safe
/// to put in a link: any path (`/account/misc/general`, `/`), any scheme or none, `www.` or
/// not, wrapped in `<>` or a masked link, followed by punctuation.
pub fn toolbox_link(text: &str) -> Option<String> {
    text.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '(' | ')' | '[' | ']' | '"' | '\'' | '|' | '`' | '*'))
        .find_map(toolbox_url)
}

fn toolbox_url(token: &str) -> Option<String> {
    let token = token.trim_end_matches(['.', ',', ';', ':', '!', '?']);
    let rest = strip_ci(token, "https://").or_else(|| strip_ci(token, "http://")).unwrap_or(token);
    let rest = strip_ci(rest, "www.").unwrap_or(rest);
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    if !rest[..host_end].eq_ignore_ascii_case(TOOLBOX_HOST) {
        return None;
    }
    let tail = rest[host_end..].split('#').next().unwrap_or_default();
    let (path, query) = tail.split_once('?').unwrap_or((tail, ""));
    profile_in(query)?;
    Some(format!("https://{TOOLBOX_HOST}{}?{query}", if path.is_empty() { "/" } else { path }))
}

/// The profile name in a toolbox link, decoded: `Antho987` from `...?profile=Antho987`.
pub fn toolbox_profile(url: &str) -> Option<String> {
    profile_in(url.split_once('?')?.1.split('#').next().unwrap_or_default())
}

fn profile_in(query: &str) -> Option<String> {
    let (_, value) = query.split('&').filter_map(|kv| kv.split_once('=')).find(|(k, _)| k.eq_ignore_ascii_case("profile"))?;
    let name: String = decode(value).trim().chars().take(50).collect();
    (!name.is_empty()).then_some(name)
}

/// `%20` and `+` as a space, `%C3%A9` as é; a broken escape stays as typed.
fn decode(v: &str) -> String {
    let b = v.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        match b[i] {
            b'%' if i + 2 < b.len() => match (hex(b[i + 1]), hex(b[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                    continue;
                }
                _ => out.push(b'%'),
            },
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `s` without `prefix`, ignoring ASCII case. `get`, not slicing: the byte at the prefix's
/// length may fall inside a character.
fn strip_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    s.get(..prefix.len()).filter(|p| p.eq_ignore_ascii_case(prefix)).and_then(|_| s.get(prefix.len()..))
}

/// Lowercase words, apostrophes dropped first so "I'm" is "im" and "let's" is "lets".
pub fn words(text: &str) -> Vec<String> {
    text.to_lowercase().replace(['\'', '’'], "").split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_owned).collect()
}

/// Words that answer "are you ready" with yes.
const YES: &[&str] = &["ready", "rdy", "redy", "here", "yes", "yea", "yeah", "ye", "yep", "yup", "ya", "sure", "go", "ok", "okay", "set"];
/// Words that may stand around a yes without changing it: "im ready now", "ready when you are",
/// "all set", "lets go".
const AROUND_YES: &[&str] = &["i", "im", "am", "now", "lets", "so", "all", "when", "you", "u", "are", "r", "and", "hey", "hi", "yo"];

/// Whether a member's reply to a ping means they're ready. Every word must be a yes ("ready",
/// "here", "yep", "ok") or one of the few that can stand around one ("im ready now", "ready when
/// you are"), with at least one yes, or the message is only a thumbs up. Anything else ("ready at
/// 8pm", "nearly ready", "ok hold on", "stuck here on W5"), a question, or a link is
/// conversation, not an answer: a missed "ready" costs staff a glance, a wrong one a ping.
pub fn says_ready(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() || t.ends_with('?') || has_link(t) {
        return false;
    }
    let w = words(t);
    if w.is_empty() {
        return ["👍", "✅", "👌"].iter().any(|e| t.contains(e));
    }
    w.len() <= 6 && w.iter().all(|w| YES.contains(&w.as_str()) || AROUND_YES.contains(&w.as_str())) && w.iter().any(|w| YES.contains(&w.as_str()))
}

/// Whether text has a web link in it, written out or bare.
fn has_link(text: &str) -> bool {
    text.contains("://") || text.to_lowercase().contains("www.") || toolbox_link(text).is_some()
}

/// Words that carry nothing for a review on their own: thanks, greetings, bumps.
const FILLER: &[&str] = &[
    "thanks", "thank", "you", "u", "ty", "tysm", "thx", "tyvm", "ok", "okay", "k", "kk", "lol", "lmao", "haha", "np", "hi", "hello", "hey", "yo", "bump", "up",
    "cool", "nice", "great", "awesome", "sure", "yes", "yep", "yeah", "no", "alright", "got", "it", "gotcha", "so", "much", "again", "please", "pls", "plz",
];

/// Whether a message is chatter: only filler words or emoji, or an answer to a ping. A message
/// with a link, a game stage or a world in it is never chatter, and neither is one with a
/// picture or a file (the caller checks those).
pub fn is_chatter(text: &str) -> bool {
    if has_link(text) || !tags_in(text).is_empty() {
        return false;
    }
    words(text).iter().all(|w| FILLER.contains(&w.as_str())) || says_ready(text)
}

/// The game stages, each with the words that put a request in it.
const STAGES: &[(&str, &[&str])] = &[
    ("Early game", &["earlygame", "early game", "beginner", "newbie", "new player", "just started"]),
    ("Mid game", &["midgame", "mid game"]),
    ("Late game", &["lategame", "late game", "endgame", "end game"]),
];

/// The dashboard's tags for a post's title or a member's message: the game stage it names
/// ("late game", "lategame", "end-game") and the worlds ("W5", "world 5"). Whole words only,
/// so "Gamestop" or "W50" tag nothing.
pub fn tags_in(text: &str) -> Vec<String> {
    let w = words(text);
    let line = format!(" {} ", w.join(" "));
    let mut tags: Vec<String> = STAGES.iter().filter(|(_, phrases)| phrases.iter().any(|p| line.contains(&format!(" {p} ")))).map(|(t, _)| (*t).to_owned()).collect();
    for (i, word) in w.iter().enumerate() {
        let n = if word == "world" {
            w.get(i + 1).map(String::as_str).unwrap_or_default()
        } else if let Some(n) = word.strip_prefix('w') {
            n
        } else {
            continue;
        };
        if n.len() == 1 && ('1'..='7').contains(&n.chars().next().unwrap_or('0')) {
            let tag = format!("W{n}");
            if !tags.contains(&tag) {
                tags.push(tag);
            }
        }
    }
    tags
}

/// Where a tag sorts: the stages in order, then the worlds.
pub fn tag_rank(tag: &str) -> usize {
    STAGES.iter().position(|(t, _)| *t == tag).unwrap_or_else(|| 10 + tag.get(1..).and_then(|n| n.parse::<usize>().ok()).unwrap_or(90))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolbox_links_on_any_path() {
        let p = |t: &str| toolbox_link(t);
        assert_eq!(p("https://idleontoolbox.com/account/misc/general?profile=Antho987").as_deref(), Some("https://idleontoolbox.com/account/misc/general?profile=Antho987"));
        assert_eq!(p("my toolbox: https://idleontoolbox.com/?profile=Kasia, thanks").as_deref(), Some("https://idleontoolbox.com/?profile=Kasia"));
        // No scheme, www, upper case, an embed-suppressed or masked link, a fragment.
        assert_eq!(p("idleontoolbox.com?profile=Kasia").as_deref(), Some("https://idleontoolbox.com/?profile=Kasia"));
        assert_eq!(p("<https://www.IdleonToolbox.com/dashboard?profile=Teo&x=1>").as_deref(), Some("https://idleontoolbox.com/dashboard?profile=Teo&x=1"));
        assert_eq!(p("[here](http://idleontoolbox.com/account?PROFILE=Noor#top)").as_deref(), Some("https://idleontoolbox.com/account?PROFILE=Noor"));
        // Not a profile, or not the toolbox.
        assert_eq!(p("https://idleontoolbox.com/account/misc/general"), None);
        assert_eq!(p("https://idleontoolbox.com/?profile="), None);
        assert_eq!(p("https://idleontoolbox.com.evil.example/?profile=Kasia"), None);
        assert_eq!(p("https://evil.example/idleontoolbox.com?profile=Kasia"), None);
        assert_eq!(p("javascript:idleontoolbox.com?profile=x"), None);
        assert_eq!(p(""), None);
        // Characters straddling a prefix's length don't panic.
        assert_eq!(p("ミラ ht€ é https€://idleontoolbox.com"), None);
    }

    #[test]
    fn profile_names_are_decoded() {
        assert_eq!(toolbox_profile("https://idleontoolbox.com/?profile=Antho987").as_deref(), Some("Antho987"));
        assert_eq!(toolbox_profile("https://idleontoolbox.com/?a=1&profile=Big+Tree%21").as_deref(), Some("Big Tree!"));
        assert_eq!(toolbox_profile("https://idleontoolbox.com/?profile=Caf%C3%A9").as_deref(), Some("Café"));
        assert_eq!(toolbox_profile("https://idleontoolbox.com/?profile=50%").as_deref(), Some("50%"));
        assert_eq!(toolbox_profile("https://idleontoolbox.com/?profile=%E"), Some("%E".into()));
        assert_eq!(toolbox_profile("https://idleontoolbox.com/"), None);
    }

    #[test]
    fn ready_answers() {
        for yes in ["ready", "Ready!", "i'm ready", "Im ready", "rdy", "ok ready", "here", "I’m here", "yes", "yep", "sure", "go", "let's go", "READY NOW", "👍", "ok", "ready when you are", "im ready now", "all set", "ready 👍"] {
            assert!(says_ready(yes), "{yes}");
        }
        for no in [
            "not ready",
            "not yet",
            "ready in 5",
            "ready in 10 min",
            "brb",
            "no",
            "are you ready?",
            "I'll be ready soon",
            "one sec",
            "",
            "thanks",
            "hello there",
            "wait",
            // Reported misfires of the first matcher.
            "ok hold on",
            "gotta go to work",
            "im getting ready",
            "ready at 8pm",
            "yes in a bit",
            "nearly ready",
            "stuck here on W5",
            "yes I have W6 unlocked",
            "is my alchemy ok",
            "here you go https://idleontoolbox.com/?profile=Kasia",
            "here www.example.com",
        ] {
            assert!(!says_ready(no), "{no}");
        }
        // A long message that happens to say "ready" is conversation.
        assert!(!says_ready("I already did the ready check for alchemy and sailing on my main"));
    }

    #[test]
    fn chatter_is_filler() {
        assert!(is_chatter("thanks!"));
        assert!(is_chatter("Thank you so much"));
        assert!(is_chatter("bump"));
        assert!(is_chatter("🙏"));
        assert!(is_chatter("ready"));
        assert!(!is_chatter("Stuck on W5 sailing"));
        assert!(!is_chatter("https://idleontoolbox.com/?profile=Kasia"));
        // A link, a world or a stage is never hidden, however short.
        assert!(!is_chatter("here you go https://idleontoolbox.com/?profile=Kasia"));
        assert!(!is_chatter("ok https://imgur.com/a/x"));
        assert!(!is_chatter("W6"));
        assert!(!is_chatter("yes late game"));
        assert!(!is_chatter("stuck here on W5"));
        assert!(!is_chatter("yes I have W6 unlocked"));
        assert!(!is_chatter("is my alchemy ok"));
        assert!(!is_chatter("ready at 8pm"));
    }

    #[test]
    fn tags_from_words() {
        assert_eq!(tags_in("Lategame account, stuck in W5"), ["Late game", "W5"]);
        assert_eq!(tags_in("early-game review please (world 2)"), ["Early game", "W2"]);
        assert_eq!(tags_in("Mid game, W3 and w4, w3 again"), ["Mid game", "W3", "W4"]);
        assert_eq!(tags_in("end game"), ["Late game"]);
        assert!(tags_in("Gamestop W50 world 9 w0 late").is_empty());
        assert!(tags_in("").is_empty());
        let mut t = vec!["W5".to_owned(), "Late game".to_owned(), "W2".to_owned(), "Early game".to_owned()];
        t.sort_by_key(|t| tag_rank(t));
        assert_eq!(t, ["Early game", "Late game", "W2", "W5"]);
    }
}
