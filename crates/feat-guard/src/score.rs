//! Scoring a message for scams. Everything here is pure, so it's tested without Discord.
//!
//! Each signal adds a weight. A message is caught when the total reaches the server's
//! threshold (low 8, normal 5, high 4). Lookalike links and the compromised-account repeat
//! weigh 8, so they catch on their own at every strictness. Wording alone needs two signals
//! at normal strictness.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::links;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Strictness {
    Low,
    #[default]
    Normal,
    High,
}

impl Strictness {
    pub const ALL: [Strictness; 3] = [Strictness::Low, Strictness::Normal, Strictness::High];

    pub fn threshold(self) -> i32 {
        match self {
            Strictness::Low => 8,
            Strictness::Normal => 5,
            Strictness::High => 4,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Strictness::Low => "low",
            Strictness::Normal => "normal",
            Strictness::High => "high",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Strictness::Low => "Low: only near-certain scams",
            Strictness::Normal => "Normal",
            Strictness::High => "High: catches more, expect some mistakes",
        }
    }

    pub fn parse(s: &str) -> Self {
        Self::ALL.into_iter().find(|x| x.key() == s).unwrap_or_default()
    }
}

/// What's known about the message besides its text.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub account_age_days: Option<f64>,
    /// Accounts younger than this count as new.
    pub new_account_days: i64,
    /// Distinct channels the same message went to in the last 30 seconds, this one included.
    pub repeat_channels: usize,
    pub has_attachments: bool,
    /// Also score commission, art and service ads. Off by default: ads aren't scams.
    pub catch_ads: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Signal {
    pub weight: i32,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct Verdict {
    pub score: i32,
    /// Heaviest first.
    pub signals: Vec<Signal>,
}

impl Verdict {
    fn add(&mut self, weight: i32, text: impl Into<String>) {
        self.score += weight;
        self.signals.push(Signal { weight, text: text.into() });
    }

    pub fn caught(&self, s: Strictness) -> bool {
        self.score >= s.threshold()
    }

    /// The heaviest signal, as the one-line reason.
    pub fn reason(&self) -> String {
        self.signals.first().map(|s| s.text.clone()).unwrap_or_else(|| "Nothing suspicious".into())
    }
}

pub fn score(text: &str, facts: &Facts) -> Verdict {
    let mut v = Verdict::default();
    let found = links::find(text);
    let mut seen = Vec::new();
    for link in &found {
        if seen.contains(&link.host) {
            continue;
        }
        seen.push(link.host.clone());
        for (w, t) in [links::judge(link), links::judge_masked(link), links::judge_userinfo(link)].into_iter().flatten() {
            v.add(w, t);
        }
    }

    let words = words(text);
    let mut wording = 0;
    for tag in TAGS.iter().filter(|t| facts.catch_ads || !t.ad) {
        let mut sum: i32 = RULES.iter().filter(|r| r.tag == tag.key && r.matches(&words)).map(|r| r.weight).sum();
        if tag.key == "portfolio" && portfolio_link(text) {
            sum += 3;
        }
        if sum > 0 {
            let w = sum.min(tag.cap);
            wording += w;
            v.add(w, tag.label);
        }
    }
    let unknown_link = found.iter().any(|l| !links::is_trusted(&l.host));
    if wording > 0 && unknown_link {
        v.add(2, "Scam wording next to an unknown link");
    }
    let lower = text.to_lowercase();
    if (lower.contains("@everyone") || lower.contains("@here")) && !found.is_empty() {
        v.add(4, "@everyone or @here with a link");
    }
    if facts.repeat_channels >= 3 {
        let w = if !found.is_empty() || facts.has_attachments { 8 } else { 5 };
        v.add(w, format!("Same message in {} channels within 30 seconds", facts.repeat_channels));
    }
    if v.score > 0
        && let Some(age) = facts.account_age_days
        && age < facts.new_account_days as f64
    {
        let text = match age.floor() as i64 {
            0 => "Account made today".to_owned(),
            1 => "Account is 1 day old".to_owned(),
            d => format!("Account is {d} days old"),
        };
        v.add(2, text);
    }
    v.signals.sort_by_key(|s| std::cmp::Reverse(s.weight));
    v
}

// ---- wording ----

/// The words of a message for phrase matching: lowercased, lookalike letters folded, apostrophes
/// dropped ("you've" is "youve"), and leet undone in words that are mostly letters ("n1tr0").
pub fn words(text: &str) -> Vec<String> {
    let cleaned: String = text
        .chars()
        .filter(|c| !matches!(c, '\'' | '’' | '`' | '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{2060}' | '\u{feff}'))
        .flat_map(char::to_lowercase)
        .map(links::fold_char)
        .collect();
    cleaned
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let letters = w.chars().filter(|c| c.is_alphabetic()).count();
            let starts_with_digit = w.starts_with(|c: char| c.is_ascii_digit());
            if letters >= 2 && !(starts_with_digit && letters < 3) && w.chars().any(|c| c.is_ascii_digit()) {
                w.chars().map(links::leet).collect()
            } else {
                w.to_owned()
            }
        })
        .collect()
}

struct Tag {
    key: &'static str,
    label: &'static str,
    /// Most this kind of wording can add, however many of its phrases match.
    cap: i32,
    /// Commission, art and service ads: only scored when the server opts in.
    ad: bool,
}

const TAGS: &[Tag] = &[
    Tag { key: "nitro", label: "Free Nitro offer", cap: 5, ad: false },
    Tag { key: "claim", label: "Asks you to claim a prize", cap: 3, ad: false },
    Tag { key: "steam", label: "Steam gift offer", cap: 4, ad: false },
    Tag { key: "inventory", label: "Giving away a game inventory", cap: 4, ad: false },
    Tag { key: "quitting", label: "Says they're quitting a game", cap: 2, ad: false },
    Tag { key: "trade", label: "Asks for a trade offer", cap: 2, ad: false },
    Tag { key: "crypto", label: "Crypto giveaway or airdrop", cap: 5, ad: false },
    Tag { key: "money", label: "Easy money promise", cap: 5, ad: false },
    Tag { key: "won", label: "Says you won something", cap: 3, ad: false },
    Tag { key: "prize", label: "Fake reward notice", cap: 5, ad: false },
    Tag { key: "gamble", label: "Casino or deposit bonus", cap: 5, ad: false },
    Tag { key: "bio", label: "Points to their bio or profile", cap: 3, ad: false },
    Tag { key: "adult", label: "Adult content spam", cap: 5, ad: false },
    Tag { key: "solicit", label: "Asks you to DM them", cap: 4, ad: true },
    Tag { key: "ad", label: "Selling services", cap: 3, ad: true },
    Tag { key: "portfolio", label: "Portfolio self-promo", cap: 5, ad: true },
    Tag { key: "price", label: "Price talk", cap: 1, ad: true },
    Tag { key: "urgency", label: "Urgency", cap: 1, ad: false },
];

/// `(tag, weight, pattern)`. Patterns are words separated by spaces:
/// - `a|b|c` matches one of the words; `pre*` matches a word starting with `pre`;
///   `sign_up` matches two words in a row.
/// - a trailing `?` makes the slot optional.
/// - `..3` skips up to three words.
/// - `#` matches a number (`500`, `50k`).
const PHRASES: &[(&str, i32, &str)] = &[
    // Nitro
    ("nitro", 4, "free discord? nitro*"),
    ("nitro", 4, "nitro* for|4 free"),
    ("nitro", 3, "nitro* giveaway|drop|airdrop|generator|gen|gift*"),
    ("nitro", 3, "discord|they is|are? giving|gives|gave away? ..3 nitro*"),
    ("nitro", 3, "gift*|giving|give away? ..2 nitro* to|for everyone|all|you|u"),
    // Claim
    ("claim", 3, "claim|redeem|grab|collect yours|your|ur|it ..2 now|here|free|gift*|reward*|prize*|nitro|bonus|before"),
    ("claim", 3, "claim|redeem now|here|free|gift*|reward*|prize*|bonus"),
    ("claim", 3, "claim|get your|ur free|gift"),
    // Steam
    ("steam", 3, "steam gift*|giftcard*|card*|wallet"),
    ("steam", 3, "free steam gift*|game*|skin*|key*|card*|wallet"),
    ("steam", 2, "# steam gift*|card*"),
    // Inventory giveaways ("I'm leaving CS2, giving my inventory away")
    ("inventory", 4, "giv*|gift*|giveaway away? ..3 my|all|whole|entire ..2 inventory|inv|skins|items|knife|knives"),
    ("inventory", 4, "inventory|skins|items giveaway|for free|away"),
    ("quitting", 2, "leaving|quitting|quit|left|done_with ..2 cs|cs2|csgo|counter|steam|rust|dota|dota2|tf2|valorant|fortnite|roblox|gaming"),
    ("trade", 2, "send|sending|throw me? a|your? trade|tradeoffer|trade_offer|offer"),
    ("trade", 2, "add me ..3 trade"),
    ("urgency", 1, "first|1st|ist come first|1st|ist serve*"),
    // Crypto
    ("crypto", 3, "crypto|bitcoin|btc|eth|ethereum|nft|nfts|usdt|solana|token|tokens|coin|coins giveaway|airdrop*|drop|invest*|doubl*|free"),
    ("crypto", 3, "airdrop* ..3 claim|wallet|connect|eligible|free|live"),
    ("crypto", 3, "connect your|ur wallet"),
    ("crypto", 3, "elon|musk|mrbeast ..2 giv*|giveaway|gift*|doubl*"),
    // Money
    ("money", 5, "earn|earning|make|making # ..3 daily|weekly|monthly|everyday"),
    ("money", 5, "earn|earning|make|making # ..3 per|every|each|a day|week|month"),
    ("money", 2, "invest* ..4 profit*|returns|roi"),
    ("money", 3, "send|deposit # ..4 receive|back|double*"),
    // Prizes
    ("won", 3, "congratulations|congrats ..3 selected|chosen|won|winner"),
    ("won", 3, "you|youve|u have? won ..3 prize|reward|gift|nitro|#"),
    ("won", 3, "you|youve|u have? been selected|chosen"),
    ("prize", 5, "reward|prize received"),
    ("prize", 5, "activate|enter|use your|the|a? bonus|promo|reward|gift code"),
    ("prize", 5, "activate|enter|use code for? bonus|promo|reward|gift"),
    ("prize", 5, "your reward|bonus|winnings|prize is|has|have been? successfully? ..2 #|added|included|credited|ready"),
    // Gambling
    ("gamble", 4, "deposit|withdraw|wager now|today|here|bonus|instantly"),
    ("gamble", 3, "welcome|deposit|signup|sign_up bonus"),
    ("gamble", 3, "bonus|rakeback|cashback|promo_code|promocode ..6 deposit|sign_up|signup|register"),
    ("gamble", 3, "sign_up|signup|register ..6 bonus|free_money|reward"),
    ("gamble", 3, "online? casino|betting|gambling|slots|slot|poker site|bonus|free"),
    ("gamble", 1, "rakeback|cashback|wagering|rollover"),
    // Bio and adult spam
    ("bio", 3, "check|see|visit|open out? my bio|profile|pfp"),
    ("bio", 3, "look at my bio|profile"),
    ("bio", 3, "link|links in|on my bio|profile"),
    ("adult", 3, "onlyfans|0nlyfans|fansly|nudes|sexting"),
    ("adult", 3, "hot|sexy|naughty|horny ..3 pics|photos|videos|content|girls|girl|teen|teens|single|singles"),
    ("adult", 3, "dm|message|text|add me ..2 for|4 ..2 nudes|pics|fun|content|sexting|nsfw|videos"),
    // Ads and solicitation
    ("solicit", 4, "message|dm|pm|whisper|msg me for rates|rate|details|pricing|prices|price|info"),
    ("solicit", 4, "dm|message|contact me|us for|to order|commission|book|buy"),
    ("solicit", 3, "dm|message me to? learn|know|find_out how"),
    ("solicit", 3, "available ..6 hit_me_up|hmu|dm_me|message_me|contact_me"),
    ("solicit", 3, "hmu|hit_me_up ..6 available|open|offering|selling|commission*|prices|price"),
    ("ad", 3, "commission|commissions are|is? open|opened"),
    ("ad", 3, "open for? commission|commissions"),
    ("ad", 3, "taking|accepting|doing commission*|orders|requests now|rn|atm"),
    ("ad", 3, "boosting|accounts_for_sale|account_for_sale"),
    ("ad", 3, "followers|likes|views|subs for sale|cheap"),
    ("ad", 3, "custom stream? overlay*|panel*|emote*|badge*|banner*|vtuber ..3 available|for_sale"),
    ("portfolio", 3, "show some|your love|support"),
    ("portfolio", 3, "take a look and show|give|drop|leave"),
    ("portfolio", 1, "i|we just? made|created|designed|finished|completed an|a? ..3 logo|pfp|banner|emote|mascot|overlay|design|artwork|model"),
    ("portfolio", 1, "for a|my ..1 client|customer"),
    ("price", 1, "very? discounted|cheap|low price|prices"),
    ("price", 1, "cheap|best prices|rates"),
    ("urgency", 1, "limited_time|act_now|hurry|dont_miss|before_its_gone"),
];

/// Portfolio links: an art site with a design word in the path.
fn portfolio_link(text: &str) -> bool {
    let lower = text.to_lowercase();
    ["behance.net/", "artstation.com/", "dribbble.com/", "fiverr.com/"].iter().any(|site| {
        lower.split_whitespace().any(|w| {
            w.find(site).is_some_and(|i| {
                let path = &w[i + site.len()..];
                ["logo", "pfp", "mascot", "emote", "banner", "design", "twitch"].iter().any(|k| path.contains(k))
            })
        })
    })
}

enum Alt {
    Num,
    Exact(Vec<String>),
    Prefix(String),
}

enum Slot {
    Words(Vec<Alt>, bool),
    Gap(usize),
    Num(bool),
}

struct Rule {
    tag: &'static str,
    weight: i32,
    slots: Vec<Slot>,
}

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    PHRASES.iter().map(|(tag, weight, p)| Rule { tag, weight: *weight, slots: parse(p) }).collect()
});

fn parse(pattern: &str) -> Vec<Slot> {
    pattern
        .split_whitespace()
        .map(|tok| {
            if let Some(n) = tok.strip_prefix("..") {
                return Slot::Gap(n.parse().unwrap_or(1));
            }
            let (tok, optional) = match tok.strip_suffix('?') {
                Some(t) => (t, true),
                None => (tok, false),
            };
            if tok == "#" {
                return Slot::Num(optional);
            }
            let alts = tok
                .split('|')
                .map(|a| match a.strip_suffix('*') {
                    _ if a == "#" => Alt::Num,
                    Some(p) => Alt::Prefix(p.to_owned()),
                    None => Alt::Exact(a.split('_').map(str::to_owned).collect()),
                })
                .collect();
            Slot::Words(alts, optional)
        })
        .collect()
}

impl Rule {
    fn matches(&self, words: &[String]) -> bool {
        (0..words.len()).any(|i| match_at(&self.slots, words, i))
    }
}

fn is_number(w: &str) -> bool {
    w.starts_with(|c: char| c.is_ascii_digit())
}

fn match_at(slots: &[Slot], words: &[String], i: usize) -> bool {
    let Some((first, rest)) = slots.split_first() else { return true };
    match first {
        Slot::Gap(n) => (0..=*n).any(|k| i + k <= words.len() && match_at(rest, words, i + k)),
        Slot::Num(optional) => {
            let hit = words.get(i).is_some_and(|w| is_number(w));
            (hit && match_at(rest, words, i + 1)) || (*optional && match_at(rest, words, i))
        }
        Slot::Words(alts, optional) => {
            let hit = alts.iter().any(|alt| match alt {
                Alt::Num => words.get(i).is_some_and(|w| is_number(w)) && match_at(rest, words, i + 1),
                Alt::Prefix(p) => words.get(i).is_some_and(|w| w.starts_with(p.as_str())) && match_at(rest, words, i + 1),
                Alt::Exact(seq) => {
                    seq.iter().enumerate().all(|(k, s)| words.get(i + k) == Some(s)) && match_at(rest, words, i + seq.len())
                }
            });
            hit || (*optional && match_at(rest, words, i))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> Verdict {
        score(text, &Facts { new_account_days: 7, ..Default::default() })
    }

    fn explain(v: &Verdict) -> String {
        v.signals.iter().map(|s| format!("{}({})", s.text, s.weight)).collect::<Vec<_>>().join(" + ")
    }

    /// Scams and spam the old bot caught, plus the ones the owner named.
    const SCAMS: &[&str] = &[
        "free discord nitro! claim yours now",
        "Hey everyone! Free Nitro giveaway at disc0rd.gift/free",
        "I make $500 daily with crypto investing, DM me to learn how",
        "Earn $200 per day with this simple trick!",
        "Check this out: https://discörd.gift/freestuff",
        "Login here: steamcommunlty.com/id/trade",
        "NFT airdrop! Claim your free gift now!",
        "Reward Received: $2500 YOUR WINNINGS HAS BEEN SUCCESSFULLY INCLUDED",
        "Activate Code for Bonus - Enter the promo code to receive a welcome bonus",
        "Deposit now and get rakeback bonus! Sign up today",
        "Your reward is $2500! Withdraw now",
        "Congratulations you've been selected to win! Claim your free gift",
        "You have won a special prize! Deposit bonus available",
        "Wager bonus: sign up and get free $50 reward code",
        // New ones
        "I'm leaving CS2, giving away my whole inventory, first come first serve. Send me a trade offer",
        "quitting csgo so im giving my skins away to whoever adds me first https://steamcommunity.com/tradeoffer/new/?partner=1",
        "@everyone free nitro for everyone https://discord-nitro.shop/claim",
        "gift for you $50 steam gift card https://steamcommunity.com.ru/gift/claim",
        "Elon is giving away 5000 BTC, send 0.1 and receive 0.2 back https://elon-x2.xyz",
        "Huge crypto airdrop is live, connect your wallet to claim https://claim-drop.site",
        "hey cuties check my bio for hot pics 😘",
        "dm me for nudes 18+ onlyfans in bio",
        "[steamcommunity.com/gift](https://steamcomrnunity.ru/gift)",
        "Fr33 n1tr0 for everyone, claim here https://dlscord.com/nitro",
        "https://xn--dscord-pvf.com/gift free nitro",
        "free nitro https://bit.ly/3abc",
        "discord is giving away nitro to everyone: https://discordgift.site/abc",
        "https://discord.com@nitro-claim.ru/gift",
    ];

    /// Commission, art and service ads the old bot caught. Only scored with `catch_ads`.
    const ADS: &[&str] = &[
        "CUSTOM VTUBER MODEL COMMISSION ARE OPEN NOW IN VERY DISCOUNTED PRICES\nMessage me for rates and details",
        "commissions open!! DM me for details and pricing 💜",
        "Taking commissions now! Message me for rates",
        "Open for commissions, contact me to order!",
        "Selling boosting services! Cheap prices, dm me for rates",
        "Followers for sale! Best rates guaranteed, message me for info",
        "heyya everyonee, i made an awesome logo PFP design for a special client. I'm thrilled to share it with you all—take a look and show some love! 💖💫https://www.behance.net/gallery/227078169/MASCOT-LOGO-PFP-FOR-TWITCH",
        "heyya everyonee, i made an awesome logo PFP design for a special client. I'm thrilled to share it with you all—take a look and show some love! 💖💫",
        "Just finished an amazing mascot design for a client! Show some love 💖 https://artstation.com/artwork/mascot-logo",
        "I created a new emote design for a customer, take a look and show your support! https://dribbble.com/shots/emote-design",
        "Custom stream overlays & panels available, hit me up 👊",
        "Custom overlays available, hmu for prices",
    ];

    /// Ordinary messages that must never be caught at normal strictness.
    const NORMAL: &[&str] = &[
        "This game is at a discounted price on Steam right now!",
        "The new DLC is cheap, only $5",
        "Have you seen the low prices on the Steam sale?",
        "Best prices I have seen for a AAA game",
        "Hey how are you doing today?",
        "Anyone wanna play Valorant?",
        "I love this server!",
        "The commission the artist did for me looks amazing!",
        "My commission from last week finally arrived",
        "Check out this new game release",
        "I got the game at a cheap price during the sale",
        "Does anyone know if that artist has commissions open? I want one",
        "The prices for art commissions are kinda high these days",
        "I made a logo for my friend, turned out great!",
        "Check out this cool design on behance.net/gallery/123456",
        "The artist made an awesome mascot for a client of ours",
        "Show some love to the new members joining today!",
        "Amazon has discounted prices today for Prime Day",
        "Limited time offer on PlayStation Store",
        "Bitcoin went up today, interesting",
        "I think NFTs are overrated lol",
        "The game went free to play, nice!",
        "You can get a free skin if you link your account",
        "I got a nice reward in the game after completing the quest",
        "The deposit for the apartment was expensive",
        "The casino level in the game is really hard",
        "I withdrew money from the ATM",
        "The bonus stage in Mario is fun",
        // Real links and people talking about Nitro or Steam
        "join us here https://discord.gg/abcdef",
        "https://discord.com/channels/1100000000000000001/1100000000000000041/1",
        "my profile: https://steamcommunity.com/id/antho",
        "it's on sale https://store.steampowered.com/app/730/CounterStrike_2/",
        "is nitro worth it? i only want it for the emojis",
        "my friend gifted me nitro for my birthday, so nice",
        "lol remember when free nitro scams were everywhere",
        "discord nitro got more expensive again",
        "I got a $20 steam gift card for christmas",
        "I'm leaving early today, see you all tomorrow",
        "congrats you won the tournament!",
        "we are live https://twitch.tv/antho",
        "nitro boost perks are nice, thanks for the boost!",
        "is free nitro from epic still a thing? https://store.epicgames.com/en-US/p/discord--discord-nitro",
        "dm me if you want to play later",
        "you can see my setup on my profile",
        "I'm quitting valorant, too toxic",
        "anyone want to trade? add me on steam",
        "the airdrop landed right next to us lmao",
        "I make 20 dollars an hour at my job",
        "the steak was great",
        "check the pinned message for the rules",
    ];

    #[test]
    fn scams_are_caught_at_normal() {
        for msg in SCAMS {
            let v = plain(msg);
            assert!(v.caught(Strictness::Normal), "missed (score {}): {msg}\n  {}", v.score, explain(&v));
        }
    }

    #[test]
    fn ads_only_catch_when_opted_in() {
        let with = Facts { new_account_days: 7, catch_ads: true, ..Default::default() };
        for msg in ADS {
            let on = score(msg, &with);
            assert!(on.caught(Strictness::Normal), "missed with ads on (score {}): {msg}\n  {}", on.score, explain(&on));
            let off = plain(msg);
            assert!(!off.caught(Strictness::Normal), "caught with ads off (score {}): {msg}\n  {}", off.score, explain(&off));
        }
    }

    #[test]
    fn normal_messages_pass_at_normal() {
        let with_ads = Facts { new_account_days: 7, catch_ads: true, ..Default::default() };
        for msg in NORMAL {
            let v = score(msg, &with_ads);
            assert!(!v.caught(Strictness::Normal), "false positive (score {}): {msg}\n  {}", v.score, explain(&v));
        }
    }

    #[test]
    fn real_links_alone_score_nothing() {
        for msg in [
            "https://discord.gg/abcdef",
            "https://steamcommunity.com/id/antho",
            "https://store.steampowered.com/app/730",
            "https://www.twitch.tv/antho",
            "https://discord.gift/AbCdEf123",
        ] {
            assert_eq!(plain(msg).score, 0, "{msg}");
        }
    }

    #[test]
    fn lookalike_links_catch_even_at_low() {
        for msg in ["https://dlscord.com/gift", "steamcommunlty.com/id/trade", "https://discord-nitro.shop"] {
            assert!(plain(msg).caught(Strictness::Low), "{msg}");
        }
    }

    #[test]
    fn compromised_account_repeat() {
        let facts = |n| Facts { repeat_channels: n, new_account_days: 7, ..Default::default() };
        let msg = "hey guys look at this https://example-site.com/x";
        assert!(!score(msg, &facts(1)).caught(Strictness::Normal));
        assert!(!score(msg, &facts(2)).caught(Strictness::Normal));
        let v = score(msg, &facts(3));
        assert!(v.caught(Strictness::Low), "{}", explain(&v));
        assert_eq!(v.reason(), "Same message in 3 channels within 30 seconds");
        // Images only, no text: still the compromised pattern.
        let img = Facts { repeat_channels: 4, has_attachments: true, new_account_days: 7, ..Default::default() };
        assert!(score("", &img).caught(Strictness::Low));
        // Plain text repeated: caught at normal, not at low.
        let text = score("anyone want to play tonight?", &facts(3));
        assert!(text.caught(Strictness::Normal) && !text.caught(Strictness::Low));
    }

    #[test]
    fn new_account_only_adds_to_other_signals() {
        let young = Facts { account_age_days: Some(0.5), new_account_days: 7, ..Default::default() };
        assert_eq!(score("hello everyone", &young).score, 0);
        let v = score("free nitro", &young);
        assert!(v.signals.iter().any(|s| s.text == "Account made today"), "{}", explain(&v));
        assert!(v.caught(Strictness::Normal));
        let old = Facts { account_age_days: Some(400.0), new_account_days: 7, ..Default::default() };
        assert!(!score("free nitro", &old).caught(Strictness::Normal));
    }

    #[test]
    fn everyone_with_link() {
        let v = plain("@everyone look https://some-site.xyz/page");
        assert!(v.signals.iter().any(|s| s.text.starts_with("@everyone")));
        assert!(!plain("@everyone stream starts in 10").signals.iter().any(|s| s.text.starts_with("@everyone")));
    }

    #[test]
    fn strictness_moves_the_line() {
        let v = plain("free nitro");
        assert!(!v.caught(Strictness::Normal));
        assert!(v.caught(Strictness::High));
        // One weak phrase never catches, even at high.
        assert!(!plain("congrats you won the tournament!").caught(Strictness::High));
        assert!(!plain("check out my profile for the schedule").caught(Strictness::High));
        assert_eq!(Strictness::parse("high"), Strictness::High);
        assert_eq!(Strictness::parse("nonsense"), Strictness::Normal);
    }

    #[test]
    fn words_undo_leet_but_keep_numbers() {
        assert_eq!(words("Fr33 N1TR0 for $50k, I'm 2500"), vec!["free", "nitro", "for", "50k", "im", "2500"]);
        assert_eq!(words("ｆｒｅｅ nіtro"), vec!["free", "nitro"]);
        assert_eq!(words("cs2 inventory"), vec!["cs2", "inventory"]);
    }

    #[test]
    fn pattern_language() {
        let r = |p: &str, t: &str| Rule { tag: "x", weight: 1, slots: parse(p) }.matches(&words(t));
        assert!(r("free discord? nitro*", "get free nitros"));
        assert!(r("free discord? nitro*", "FREE DISCORD NITRO"));
        assert!(!r("free discord? nitro*", "nitro free"));
        assert!(r("earn # ..3 daily", "earn $500 easily daily"));
        assert!(!r("earn # ..1 daily", "earn $500 very easily daily"));
        assert!(r("sign_up bonus", "sign up bonus"));
        assert!(!r("sign_up bonus", "sign bonus"));
    }
}
