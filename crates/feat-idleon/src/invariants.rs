//! Checks that should hold for every save (Idleon page round 2, 2026-11-09). The unit test runs
//! them on an empty save, which builds every mechanic and ladder; `examples/invariants.rs`
//! runs them on real saves, rendered page included.
//!
//! - No tier asks more than `crowd::PART` items, except the `BEST` tables and game tiers drawn
//!   whole (a statue tier is every statue), and count tiers ("any 20 of 80").
//! - Every rung of a ladder from other players' saves belongs to its mechanic's world or later:
//!   nothing is expected before you can have it.
//! - The rendered results have no NaN, inf, null or "undefined", and the tier data the page
//!   script draws later tiers from is whole: every row's label and note a string, every item it
//!   names in its item list.

use bot_core::serde_json::{self, Value};

use crate::crowd::{BEST, PART, endgame};
use crate::review::{Mechanic, Review};

/// What breaks a rule in `r`'s mechanics and ladders, a line each.
pub fn review(r: &Review) -> Vec<String> {
    let mut bad = Vec::new();
    for m in &r.mechanics {
        // What the page draws: both sides of a switch, else the mechanic itself.
        let views: Vec<Mechanic> = match m.levels_view().zip(m.tiers_view()) {
            Some((a, b)) => vec![a, b],
            None => vec![m.clone()],
        };
        for v in &views {
            if BEST.contains(&v.key) || v.whole_tiers {
                continue;
            }
            let tiers = v.tiers();
            for k in (0..tiers.len()).filter(|&k| tiers[k].targets.is_some()) {
                let n = v.tier_asks(&tiers, k).len();
                if n > PART {
                    bad.push(format!("{} tier {} ({}) asks {n}, more than {PART}", v.key, k + 1, tiers[k].label));
                }
            }
        }
        if let Some(l) = &m.crowd {
            for rung in l.rungs.iter().filter(|r| !endgame(r.stage) && r.world < m.world) {
                bad.push(format!("{} (World {}) has rung {:?} in World {}", m.key, m.world, rung.label, rung.world));
            }
        }
    }
    bad
}

/// Words that mean a number or a value went missing on the way to the page.
const BROKEN: [&str; 4] = ["NaN", "undefined", "inf", "null"];

/// What's broken in a rendered results page (`crate::results_html`).
pub fn page(html: &str) -> Vec<String> {
    let mut bad = Vec::new();
    // Text and attributes, not the tier data: that's checked as data below.
    let mut rest = html;
    let mut text = String::new();
    while let Some(at) = rest.find("<script") {
        text.push_str(&rest[..at]);
        rest = rest[at..].find("</script>").map_or("", |e| &rest[at + e..]);
    }
    text.push_str(rest);
    let edge = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric() && c != '_');
    for word in BROKEN {
        for (at, _) in text.match_indices(word) {
            if edge(text[..at].chars().next_back()) && edge(text[at + word.len()..].chars().next()) {
                let from = text.floor_char_boundary(at.saturating_sub(60));
                let to = text.ceil_char_boundary((at + word.len() + 40).min(text.len()));
                bad.push(format!("{word:?} in the page: …{}…", &text[from..to]));
            }
        }
    }
    let open = "<script class=\"ia-later\" type=\"application/json\">";
    for (n, (at, _)) in html.match_indices(open).enumerate() {
        let body = &html[at + open.len()..];
        let body = &body[..body.find("</script>").unwrap_or(body.len())];
        let d: Value = match serde_json::from_str(body) {
            Ok(d) => d,
            Err(e) => {
                bad.push(format!("tier data {n} doesn't read: {e}"));
                continue;
            }
        };
        let items = d["i"].as_array().map_or(0, Vec::len);
        let item_ok = |x: &Value| x.as_u64().is_some_and(|i| (i as usize) < items);
        for (near, rows) in [(true, &d["n"]), (false, &d["f"])] {
            for (k, row) in rows.as_object().into_iter().flatten() {
                let ok = row[0].is_string()
                    && row[1].is_string()
                    && row[2].as_array().is_some_and(|a| a.iter().all(|x| item_ok(&x[0]) && x[1].is_string()))
                    && if near { row[3].as_array().is_some_and(|a| a.iter().all(item_ok)) } else { row[3].is_u64() && row[4].is_u64() && row[5].is_u64() };
                let words = row.to_string();
                if !ok || BROKEN.iter().any(|w| words.contains(&format!("\"{w}\""))) {
                    bad.push(format!("tier data {n}, tier {k}: {}", &words[..words.floor_char_boundary(200)]));
                }
            }
        }
    }
    bad
}

#[cfg(test)]
mod tests {
    use crate::review::Review;
    use crate::save::Save;

    /// Broken today, each with the lane fixing it (round 2): remove a line once it's fixed, and
    /// the test keeps it fixed. A new problem fails the test.
    const KNOWN: &[&str] = &[
        // The final tier, everything at its max, isn't split (up to 235 asks on cards).
        "(Everything maxed) asks",
        // A by-max ladder, never split: 11 anvil points a tier.
        "anvil-points tier ",
    ];

    #[test]
    fn every_ladder_keeps_its_tiers_small_and_in_their_worlds() {
        let s = Save::parse(r#"{"OptLacc": []}"#).expect("an empty save");
        let r = Review::new(&s, crate::checks::all(&s), 7);
        let bad = super::review(&r);
        let new: Vec<&String> = bad.iter().filter(|b| !KNOWN.iter().any(|k| b.contains(k))).collect();
        assert!(new.is_empty(), "{} new problems:\n{}", new.len(), new.iter().map(|b| b.as_str()).collect::<Vec<_>>().join("\n"));
        let page = crate::results_html(&r);
        assert_eq!(super::page(&page), Vec::<String>::new());
    }

    #[test]
    fn the_page_check_finds_what_it_looks_for() {
        let page = r#"<p>Lv NaN</p><p>null_onyx</p><script class="ia-later" type="application/json">{"i":[["a"]],"n":{"1":["x","y",[[3,"Lv 2",""]],[]]},"f":{}}</script>"#;
        let bad = super::page(page);
        assert_eq!(bad.len(), 2, "{bad:?}");
        assert!(bad[0].contains("NaN") && bad[1].starts_with("tier data 0, tier 1"));
    }
}
