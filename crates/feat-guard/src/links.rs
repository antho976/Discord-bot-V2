//! Finding links in a message and judging their domains: lookalikes of Discord, Steam and
//! Twitch (typos, homoglyphs, punycode), fake gift domains, masked links and IP loggers.

/// Domains owned by the brands scammers imitate. Never flagged; their subdomains neither.
const OFFICIAL: &[&str] = &[
    "discord.com", "discord.gg", "discordapp.com", "discordapp.net", "discord.media", "discord.gift",
    "discord.new", "discord.dev", "discord.co", "discord.design", "discord.store", "discordstatus.com",
    "discordmerch.com", "dis.gd", "steampowered.com", "steamcommunity.com", "steamstatic.com",
    "steamgames.com", "steamdeck.com", "steamusercontent.com", "steamserver.net", "steamchina.com",
    "s.team", "steam.tv", "valvesoftware.com", "twitch.tv", "twitch.com", "twitchcdn.net",
    "twitchsvc.net", "twitchapps.com", "twitchstatus.com", "ttvnw.net", "jtvnw.net",
];

/// Well-known community sites that carry a brand name in their domain.
const COMMUNITY: &[&str] = &[
    "discords.com", "discord.me", "discordbotlist.com", "discordservers.com", "bots.gg", "discordlist.gg",
    "steamgifts.com", "steamdb.info", "steamrep.com", "steamcharts.com", "steamladder.com", "steamid.io",
    "steamid.uk", "steamtrades.com", "steamspy.com", "twitchtracker.com", "twitchmetrics.net",
    "twitchemotes.com", "twitchinsights.net", "nitrotype.com", "gonitro.com", "nitropack.io",
];

/// Big sites whose links don't make scam wording more suspicious.
const TRUSTED: &[&str] = &[
    "youtube.com", "youtu.be", "twitter.com", "x.com", "reddit.com", "google.com", "github.com",
    "wikipedia.org", "tenor.com", "giphy.com", "imgur.com", "epicgames.com", "instagram.com",
    "tiktok.com", "spotify.com", "kick.com", "playstation.com", "xbox.com", "nintendo.com",
    "bsky.app", "twimg.com",
];

/// Gambling and skin-betting sites, matched on the main name.
const GAMBLING: &[&str] = &[
    "stake", "duelbits", "roobet", "rollbit", "gamdom", "csgoroll", "packdraw", "1xbet", "22bet", "melbet",
    "betway", "betwinner", "mostbet", "linebet", "1win", "skinhub", "csgofast", "csgoempire", "csgo500",
    "csgopoly", "hellcase", "keydrop", "farmskins",
];
const GAMBLING_HOSTS: &[&str] = &["bc.game", "skin.club", "clash.gg", "key-drop.com"];

const IP_LOGGERS: &[&str] = &[
    "grabify.link", "iplogger.org", "iplogger.com", "iplogger.ru", "2no.co", "yip.su", "blasze.com",
    "ps3cfw.com", "lovebird.guru", "trulove.guru", "dateing.club", "shrekis.life", "headshot.monster",
];

const SHORTENERS: &[&str] = &[
    "bit.ly", "tinyurl.com", "cutt.ly", "rb.gy", "is.gd", "t.ly", "shorturl.at", "goo.su", "clck.ru",
    "tiny.cc", "ow.ly", "rebrand.ly", "t.co",
];

/// What a brand's real domain is, for messages: `(skeleton, shown as, distance allowed)`.
/// Distance 0 means only homoglyph or leet lookalikes count, no typos.
const BRANDS: &[(&str, &str, usize)] = &[
    ("discord", "discord.com", 1),
    ("discordapp", "discord.com", 1),
    ("steamcommunity", "steamcommunity.com", 2),
    ("steampowered", "steampowered.com", 2),
    ("steam", "steampowered.com", 0),
    ("twitch", "twitch.tv", 0),
];

/// Brand names looked for inside a domain name, as skeletons, longest first.
const BRAND_WORDS: &[&str] = &["discordapp", "steamcommunity", "steampowered", "discord", "twitch", "steam", "nitro"];

/// Words that turn a brand name in a domain into a fake gift or login page.
const BAIT: &[&str] = &["gift", "nitro", "free", "claim", "promo", "giveaway", "reward", "bonus", "verify", "login", "auth", "airdrop"];

/// Words real community sites also put next to a brand (steamtradematcher.com). Bait only when
/// the name holds nothing else, as in steam-tradeoffer.ru.
const WEAK_BAIT: &[&str] = &["trade", "offer", "drop", "account", "secure", "support", "event", "skin", "case", "boost"];

/// Second-level names under a country's domain, so `steam.co.uk` is read as the site `steam`.
const SECOND_LEVEL: &[&str] = &["co", "com", "net", "org", "gov", "edu", "ac"];

/// Top-level domains accepted for links written without `https://`.
const BARE_TLDS: &[&str] = &[
    "com", "net", "org", "gg", "gift", "gifts", "io", "co", "ru", "xyz", "app", "me", "tv", "info", "biz",
    "site", "online", "store", "shop", "click", "link", "top", "club", "live", "pro", "fun", "icu", "cc",
    "ws", "su", "ly", "to", "vip", "win", "space", "today", "money", "cash", "community", "promo", "help",
    "dev", "uk", "de", "fr", "pl", "us", "cn", "in", "ua", "by", "kz", "tk", "ml", "ga", "cf", "gq",
];

#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    /// Lowercased, `www.` and punycode decoded.
    pub host: String,
    /// The host as written, lowercased, when punycode was decoded.
    pub raw_host: Option<String>,
    /// For `[text](url)`: a domain shown in the text.
    pub shown_host: Option<String>,
    /// `https://discord.com@evil.ru`: the part before the `@`.
    pub userinfo: Option<String>,
}

/// Every link in a message, with or without `https://`.
pub fn find(text: &str) -> Vec<Link> {
    let mut out = Vec::new();
    let lower = text.to_lowercase();
    // Masked links first, so their text isn't read as a second link.
    let mut masked_spans: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while let Some(open) = lower[i..].find('[').map(|o| o + i) {
        let Some(close) = lower[open..].find("](").map(|c| c + open) else { break };
        let Some(end) = lower[close..].find(')').map(|e| e + close) else { break };
        let label = &lower[open + 1..close];
        let target = lower[close + 2..end].trim_matches(|c| c == '<' || c == '>');
        if !label.contains('[')
            && let Some(mut link) = parse_url(target)
        {
            link.shown_host = label.split_whitespace().find_map(|w| host_of_token(w).map(|(h, _)| h));
            out.push(link);
            masked_spans.push((open, end + 1));
        }
        i = close + 2;
    }
    for token in lower.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '(' | ')' | '[' | ']' | '|' | '`')) {
        let start = token.as_ptr() as usize - lower.as_ptr() as usize;
        if token.is_empty() || masked_spans.iter().any(|(a, b)| start >= *a && start < *b) {
            continue;
        }
        if let Some(link) = parse_url(token) {
            out.push(link);
        }
    }
    out
}

/// A token that is a link: `https://…` anywhere in it, or a bare domain with a known TLD.
fn parse_url(token: &str) -> Option<Link> {
    if let Some(at) = token.find("://") {
        let scheme = &token[..at];
        if !scheme.ends_with("http") && !scheme.ends_with("https") {
            return None;
        }
        let rest = &token[at + 3..];
        let authority = rest.split(['/', '?', '#', '\\']).next().unwrap_or_default();
        let (userinfo, hostport) = match authority.rfind('@') {
            Some(a) => (Some(authority[..a].to_owned()), &authority[a + 1..]),
            None => (None, authority),
        };
        let host = hostport.split(':').next().unwrap_or_default();
        let (host, raw_host) = clean_host(host)?;
        return Some(Link { host, raw_host, shown_host: None, userinfo });
    }
    let (host, raw_host) = host_of_token(token)?;
    Some(Link { host, raw_host, shown_host: None, userinfo: None })
}

/// A bare `domain.tld/path` token, or a full URL, reduced to its host.
fn host_of_token(token: &str) -> Option<(String, Option<String>)> {
    if token.contains("://") {
        return parse_url(token).map(|l| (l.host, l.raw_host));
    }
    if token.contains('@') {
        return None;
    }
    let token = token.trim_end_matches(['.', ',', '!', '?', ':', ';', '*', '_', '~']);
    let host = token.split(['/', '?', '#']).next().unwrap_or_default();
    let host = host.trim_start_matches(['*', '_', '~']);
    let tld = host.rsplit('.').next().unwrap_or_default();
    if !host.contains('.') || !BARE_TLDS.contains(&tld) {
        return None;
    }
    clean_host(host)
}

fn clean_host(host: &str) -> Option<(String, Option<String>)> {
    let host = host.trim_end_matches('.').trim_start_matches("www.");
    let ok = host.contains('.')
        && host.split('.').all(|l| !l.is_empty() && l.chars().all(|c| c.is_alphanumeric() || c == '-'));
    if !ok {
        return None;
    }
    let decoded: Vec<String> = host
        .split('.')
        .map(|l| l.strip_prefix("xn--").and_then(punycode_decode).unwrap_or_else(|| l.to_owned()))
        .collect();
    let decoded = decoded.join(".");
    let raw = (decoded != host).then(|| host.to_owned());
    Some((decoded, raw))
}

fn on_list(host: &str, list: &[&str]) -> bool {
    list.iter().any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

pub fn is_official(host: &str) -> bool {
    on_list(host, OFFICIAL)
}

/// Official, community or big well-known sites.
pub fn is_trusted(host: &str) -> bool {
    on_list(host, OFFICIAL) || on_list(host, COMMUNITY) || on_list(host, TRUSTED)
}

/// Why a domain is suspicious, as `(weight, reason)`. None for ordinary domains.
pub fn judge(link: &Link) -> Option<(i32, String)> {
    let host = link.host.as_str();
    let shown = link.raw_host.as_deref().unwrap_or(host);
    if is_official(host) || on_list(host, COMMUNITY) {
        return None;
    }
    if on_list(host, IP_LOGGERS) {
        return Some((6, format!("IP-logger link ({shown})")));
    }
    // A real domain written in front of another one: discord.com.evil.ru,
    // steamcommunity.com-login.xyz, discord.gift-nitro.ru.
    if let Some(real) = official_in_front(host) {
        return Some((8, format!("Link imitates {real} ({shown})")));
    }
    let labels: Vec<&str> = host.split('.').collect();
    // The site someone registered is `main` plus the suffix after it: in discord.fandom.com the
    // site is fandom.com and `discord` is only a page on it.
    let suffix = suffix_len(&labels);
    let names = &labels[..labels.len() - suffix];
    let main = names.last().copied().unwrap_or_default();
    for label in names {
        for part in std::iter::once(*label).chain(label.split('-')) {
            if let Some((_, real, _)) = BRANDS.iter().find(|(b, _, _)| part == *b) {
                // The brand itself as the site's name: discord.xyz, steam.ru. As a subdomain
                // (discord.fandom.com) or next to other words (discord-nitro) it is judged below.
                if part == *label && *label == main {
                    return Some((8, format!("Link imitates {real} ({shown})")));
                }
                continue;
            }
            // A brand in disguise: dlscord, dіscord (Cyrillic і), steamcommunlty.
            let sk = skeleton(part);
            for (brand, real, dist) in BRANDS {
                let close = sk == *brand || (*dist > 0 && sk.len() >= 5 && distance(&sk, brand) <= *dist);
                if close {
                    return Some((8, format!("Link imitates {real} ({shown})")));
                }
            }
        }
    }
    // A brand next to bait in one name (discord-nitro.shop, steam-tradeoffer.ru), or a brand
    // on a site whose name is bait (discord.gifts-claim.ru, steamcommunity.login-page.ru,
    // discord-app.gift).
    let skeletons: Vec<String> = labels.iter().map(|l| skeleton(l)).collect();
    let mut glued_brand = None;
    for (i, sk) in skeletons[..names.len()].iter().enumerate() {
        let Some(brand) = brand_in(sk) else { continue };
        let fake = has_word(sk, BAIT, brand)
            || (has_word(sk, WEAK_BAIT, brand) && only_bait(sk))
            || skeletons.iter().enumerate().any(|(j, other)| j != i && (has_word(other, BAIT, brand) || only_bait(other)));
        if fake {
            return Some((8, format!("Fake {} gift or login link ({shown})", brand_label(brand))));
        }
        if names[i] != brand && brand != "nitro" {
            glued_brand = glued_brand.or(Some(brand));
        }
    }
    if let Some(brand) = glued_brand {
        return Some((2, format!("Unofficial domain using the {} name ({shown})", brand_label(brand))));
    }
    if GAMBLING.contains(&main) || on_list(host, GAMBLING_HOSTS) {
        return Some((4, format!("Gambling or skin-betting site ({shown})")));
    }
    if link.raw_host.is_some() && mixes_scripts(host) {
        return Some((3, format!("Domain mixes alphabets ({shown})")));
    }
    if on_list(host, SHORTENERS) {
        return Some((1, format!("Shortened link ({shown})")));
    }
    None
}

/// An official domain written at the start of a label run and followed by more host:
/// `discord.com.evil.ru`, `steamcommunity.com-login.xyz`.
fn official_in_front(host: &str) -> Option<&'static str> {
    let b = host.as_bytes();
    OFFICIAL.iter().copied().find(|d| {
        host.match_indices(d).any(|(i, _)| (i == 0 || b[i - 1] == b'.') && matches!(b.get(i + d.len()), Some(b'.' | b'-')))
    })
}

/// How many labels at the end of a host are the public suffix: 1 for `.com`, 2 for a
/// country's second level like `.co.uk` or `.com.ru`.
fn suffix_len(labels: &[&str]) -> usize {
    let n = labels.len();
    if n >= 3 && labels[n - 1].len() == 2 && SECOND_LEVEL.contains(&labels[n - 2]) { 2 } else { 1 }
}

/// The first brand inside a name's skeleton.
fn brand_in(sk: &str) -> Option<&'static str> {
    BRAND_WORDS.iter().copied().find(|b| sk.contains(b))
}

fn brand_label(brand: &str) -> &'static str {
    match brand {
        "steam" | "steamcommunity" | "steampowered" => "Steam",
        "twitch" => "Twitch",
        "nitro" => "Nitro",
        _ => "Discord",
    }
}

/// Whether a name's skeleton holds one of `words`, other than the brand found in it.
fn has_word(sk: &str, words: &[&str], brand: &str) -> bool {
    words.iter().any(|w| *w != brand && sk.contains(&skeleton(w)))
}

/// A name made of bait words with nothing else but digits, brands and up to two letters:
/// `gifts-claim`, `steam-tradeoffers`. A name with another word in it (`steamtradematcher`) is
/// a site of its own.
fn only_bait(sk: &str) -> bool {
    let mut rest = sk.to_owned();
    let mut bait = false;
    for w in BAIT.iter().chain(WEAK_BAIT) {
        let w = skeleton(w);
        if rest.contains(&w) {
            bait = true;
            rest = rest.replace(&w, "");
        }
    }
    for b in BRAND_WORDS {
        rest = rest.replace(b, "");
    }
    bait && rest.chars().filter(|c| !c.is_ascii_digit()).count() <= 2
}

/// `[discord.com](https://evil.ru)`: the text shows one site and the link opens another.
pub fn judge_masked(link: &Link) -> Option<(i32, String)> {
    let shown = link.shown_host.as_deref()?;
    let same = shown == link.host || link.host.ends_with(&format!(".{shown}")) || shown.ends_with(&format!(".{}", link.host));
    if same {
        return None;
    }
    let weight = if is_trusted(shown) { 6 } else { 4 };
    Some((weight, format!("Link text shows {shown} but opens {}", link.host)))
}

/// `https://discord.com@evil.ru`: a real-looking domain before an `@` hides the real one.
pub fn judge_userinfo(link: &Link) -> Option<(i32, String)> {
    let info = link.userinfo.as_deref()?;
    info.contains('.').then(|| (6, format!("Link hides its real address behind {info}@")))
}

fn mixes_scripts(host: &str) -> bool {
    let latin = host.chars().any(|c| c.is_ascii_alphabetic());
    let other = host.chars().any(|c| c.is_alphabetic() && !c.is_ascii());
    latin && other
}

/// A letter folded to the ASCII letter it imitates: accents, Cyrillic and Greek lookalikes,
/// fullwidth forms.
pub fn fold_char(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'а' | 'α' => 'a',
        'ß' | 'в' | 'β' => 'b',
        'ç' | 'ć' | 'č' | 'с' | 'ϲ' => 'c',
        'ď' | 'ԁ' | 'ɗ' => 'd',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'е' | 'ё' | 'ε' => 'e',
        'ɡ' | 'ğ' => 'g',
        'һ' | 'н' => 'h',
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'ı' | 'і' | 'ї' | 'ι' | 'ɩ' => 'i',
        'ј' => 'j',
        'к' | 'κ' => 'k',
        'ł' | 'ℓ' => 'l',
        'м' => 'm',
        'ñ' | 'ń' | 'η' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'о' | 'ο' | 'σ' => 'o',
        'р' | 'ρ' => 'p',
        'ԛ' => 'q',
        'ś' | 'š' | 'ѕ' => 's',
        'т' | 'τ' => 't',
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'υ' => 'u',
        'ѵ' | 'ν' => 'v',
        'ԝ' | 'ω' => 'w',
        'х' | 'χ' => 'x',
        'ý' | 'ÿ' | 'у' | 'ү' => 'y',
        'ž' | 'ź' | 'ż' => 'z',
        '\u{ff41}'..='\u{ff5a}' => char::from_u32(c as u32 - 0xff41 + 'a' as u32).unwrap_or(c),
        '\u{ff21}'..='\u{ff3a}' => char::from_u32(c as u32 - 0xff21 + 'a' as u32).unwrap_or(c),
        '\u{ff10}'..='\u{ff19}' => char::from_u32(c as u32 - 0xff10 + '0' as u32).unwrap_or(c),
        _ => c,
    }
}

/// Digits and symbols used as letters.
pub fn leet(c: char) -> char {
    match c {
        '0' => 'o',
        '1' | '!' | '|' => 'i',
        '3' => 'e',
        '4' | '@' => 'a',
        '5' | '$' => 's',
        '7' => 't',
        '8' => 'b',
        '9' => 'g',
        _ => c,
    }
}

/// What a domain label looks like to a reader: lowercased, homoglyphs and leet folded, `l` read
/// as `i`, `rn` as `m`, `vv` as `w`, separators dropped. Two strings with the same skeleton look
/// alike.
pub fn skeleton(s: &str) -> String {
    let folded: String = s
        .chars()
        .flat_map(char::to_lowercase)
        .map(fold_char)
        .map(leet)
        .filter(|c| c.is_alphanumeric())
        .collect();
    folded.replace("rn", "m").replace("vv", "w").replace('l', "i")
}

/// Edit distance counting a swap of two neighbours as one edit.
pub fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    if n.abs_diff(m) > 2 {
        return 3;
    }
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[n][m]
}

/// Decode one punycode label (without its `xn--` prefix), RFC 3492.
pub fn punycode_decode(input: &str) -> Option<String> {
    const BASE: u32 = 36;
    const TMIN: u32 = 1;
    const TMAX: u32 = 26;
    fn adapt(delta: u32, points: u32, first: bool) -> u32 {
        let mut delta = if first { delta / 700 } else { delta / 2 };
        delta += delta / points;
        let mut k = 0;
        while delta > ((BASE - TMIN) * TMAX) / 2 {
            delta /= BASE - TMIN;
            k += BASE;
        }
        k + ((BASE - TMIN + 1) * delta) / (delta + 38)
    }
    let (basic, extended) = match input.rfind('-') {
        Some(i) => (&input[..i], &input[i + 1..]),
        None => ("", input),
    };
    if !basic.is_ascii() {
        return None;
    }
    let mut out: Vec<char> = basic.chars().collect();
    let (mut n, mut i, mut bias) = (128u32, 0u32, 72u32);
    let mut digits = extended.bytes();
    let mut peek = digits.clone();
    while peek.next().is_some() {
        let old_i = i;
        let mut w = 1u32;
        let mut k = BASE;
        loop {
            let b = digits.next()?;
            let digit = match b {
                b'a'..=b'z' => b - b'a',
                b'A'..=b'Z' => b - b'A',
                b'0'..=b'9' => b - b'0' + 26,
                _ => return None,
            } as u32;
            i = i.checked_add(digit.checked_mul(w)?)?;
            let t = if k <= bias { TMIN } else if k >= bias + TMAX { TMAX } else { k - bias };
            if digit < t {
                break;
            }
            w = w.checked_mul(BASE - t)?;
            k += BASE;
        }
        let len = out.len() as u32 + 1;
        bias = adapt(i - old_i, len, old_i == 0);
        n = n.checked_add(i / len)?;
        i %= len;
        out.insert(i as usize, char::from_u32(n)?);
        i += 1;
        peek = digits.clone();
    }
    Some(out.into_iter().collect())
}

/// Make links in quoted text unclickable for a staff alert: `https://` becomes `hxxps://` and
/// dots in domains become `[.]`.
pub fn defang(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for (i, word) in text.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let looks_like_link = word.contains("://") || host_of_token(&word.to_lowercase()).is_some();
        if looks_like_link {
            let mut w = word.replace('.', "[.]");
            // Found on the original bytes: lowercasing can change a character's length (the
            // Kelvin sign is 3 bytes, its "k" 1), so an index into a lowercased copy may land
            // inside a character here and panic.
            if let Some(at) = w.find("://")
                && let Some(t) = w.as_bytes()[..at].windows(3).position(|b| b.eq_ignore_ascii_case(b"ttp"))
            {
                w.replace_range(t..t + 3, "xxp");
            }
            out.push_str(&w);
        } else {
            out.push_str(word);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts(text: &str) -> Vec<String> {
        find(text).into_iter().map(|l| l.host).collect()
    }

    fn verdict(text: &str) -> Option<i32> {
        find(text).iter().filter_map(judge).map(|(w, _)| w).max()
    }

    #[test]
    fn finds_links_with_and_without_scheme() {
        assert_eq!(hosts("go to https://www.Example.com/a?b and steamcommunlty.com/id/trade."), vec!["example.com", "steamcommunlty.com"]);
        assert_eq!(hosts("<https://discord.gg/abc>"), vec!["discord.gg"]);
        assert_eq!(hosts("see file.txt or e.g. this, mail me@site.com"), Vec::<String>::new());
        assert_eq!(hosts("Check this out: https://discörd.gift/freestuff"), vec!["discörd.gift"]);
    }

    #[test]
    fn decodes_punycode() {
        assert_eq!(punycode_decode("mnchen-3ya").as_deref(), Some("münchen"));
        assert_eq!(punycode_decode("dscord-pvf").as_deref(), Some("dіscord"));
        assert_eq!(punycode_decode("dsrd-65dr9l").as_deref(), Some("dіsсоrd"));
        assert_eq!(hosts("https://xn--dscord-pvf.com/gift"), vec!["dіscord.com"]);
    }

    #[test]
    fn real_domains_pass() {
        for t in [
            "https://discord.com/channels/1/2",
            "https://discord.gg/abc",
            "https://discord.gift/AbCdEf",
            "https://steamcommunity.com/id/me",
            "https://store.steampowered.com/app/730",
            "https://www.twitch.tv/antho",
            "https://cdn.discordapp.com/attachments/1/2/a.png",
            "https://www.steamgifts.com/",
            "https://youtube.com/watch?v=1",
            "https://www.switch.com/",
            "https://steak.com/menu",
            "https://discords.com/servers",
        ] {
            assert_eq!(verdict(t), None, "{t}");
        }
    }

    #[test]
    fn lookalikes_are_caught() {
        for t in [
            "https://dlscord.com/gift",
            "https://disc0rd.gift/free",
            "https://discörd.gift/freestuff",
            "steamcommunlty.com/id/trade",
            "https://steamcommunity.com.ru/tradeoffer",
            "https://stearncommunity.com/login",
            "https://steamcommnuity.com/",
            "https://dicsord.com/nitro",
            "https://xn--dscord-pvf.com/gift",
            "https://twltch.tv/login",
            "https://discord-nitro.shop/claim",
            "https://dlscord-nitro.shop/claim",
            "https://free-nitro.xyz",
            "https://steam-tradeoffer.ru/x",
            "discordgift.site/abc",
        ] {
            assert_eq!(verdict(t), Some(8), "{t}");
        }
    }

    #[test]
    fn brand_pages_on_other_sites_pass() {
        // A brand as a page on an unrelated site, or next to a real word, is not an imitation.
        for t in [
            "https://discord.js.org/docs/packages/discord.js/main",
            "https://discord.fandom.com/wiki/Nitro",
            "https://www.steamtradematcher.com/",
            "https://steam.gamebanana.com/",
            "https://discordjs.guide/",
        ] {
            let v = verdict(t);
            assert!(v.is_none_or(|w| w < 4), "{t}: {v:?}");
        }
        assert_eq!(verdict("https://discord.fandom.com/wiki/Nitro"), None);
        assert_eq!(verdict("https://discord.js.org/"), None);
        assert_eq!(verdict("https://www.steamtradematcher.com/"), Some(2));
    }

    #[test]
    fn scam_shapes_around_a_brand_are_caught() {
        for t in [
            "https://discord.gift-nitro.ru/claim",
            "https://steamcommunity.com-login.xyz/id/me",
            "https://discord.com.verify.ru/",
            "https://cdn.discordapp.com.files.ru/a.png",
            "https://dlscord.com/",
            "https://steam-tradeoffer.ru/x",
            "https://steamtradeoffers.com/x",
            "https://steam.co.uk/",
            "https://discord.gifts-claim.ru/",
            "https://steamcommunity.login-page.ru/",
            "https://discord.nitro.xyz/",
            "https://discord-app.gift/",
            "https://discord-nitro.pages.dev/",
            "https://dlscord.pages.dev/",
            "https://free-discord-nitro-generator.com/",
        ] {
            assert_eq!(verdict(t), Some(8), "{t}");
        }
    }

    #[test]
    fn masked_and_userinfo_links() {
        let l = &find("[steamcommunity.com/gift](https://steamcomrnunity.ru/gift)")[0];
        assert_eq!(l.host, "steamcomrnunity.ru");
        assert_eq!(judge_masked(l).map(|x| x.0), Some(6));
        let ok = &find("[my clip](https://youtube.com/watch?v=1)")[0];
        assert_eq!(judge_masked(ok), None);
        let same = &find("[discord.com](https://discord.com/invite)")[0];
        assert_eq!(judge_masked(same), None);
        let trick = &find("https://discord.com@evil.ru/gift")[0];
        assert_eq!(trick.host, "evil.ru");
        assert!(judge_userinfo(trick).is_some());
    }

    #[test]
    fn other_bad_domains() {
        assert_eq!(verdict("https://grabify.link/ABC"), Some(6));
        assert_eq!(verdict("https://csgoroll.com/r/x"), Some(4));
        assert_eq!(verdict("https://bit.ly/abc"), Some(1));
        assert_eq!(verdict("https://mistakes.com"), None);
    }

    #[test]
    fn defangs() {
        assert_eq!(defang("free https://dlscord.com/x now"), "free hxxps://dlscord[.]com/x now");
        assert_eq!(defang("go to steamcommunlty.com/id"), "go to steamcommunlty[.]com/id");
        assert_eq!(defang("hello there."), "hello there.");
    }

    #[test]
    fn skeletons_and_distance() {
        assert_eq!(skeleton("D1SC0RD"), "discord");
        assert_eq!(skeleton("dlscord"), "discord");
        assert_eq!(skeleton("stearncommunity"), "steamcommunity");
        assert_eq!(distance("dicsord", "discord"), 1);
        assert_eq!(distance("discord", "discord"), 0);
        assert_eq!(distance("steak", "steam"), 1);
    }

    #[test]
    fn defang_survives_characters_that_change_length_when_lowercased() {
        // U+212A (Kelvin sign) lowercases to a 1-byte "k"; this used to panic.
        assert_eq!(defang("\u{212A}ttp://x.com"), "\u{212A}xxp://x[.]com");
        assert_eq!(defang("free nitro HTTPS://x.com"), "free nitro HxxPS://x[.]com".replace("HxxPS", "HxxpS"));
        assert_eq!(defang("İttp://a.b"), "İxxp://a[.]b");
    }
}
