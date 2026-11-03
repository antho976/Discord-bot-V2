//! The saga and the daily bounties.
//!
//! **The Loom.** The Norns and the Fates weave every saga at one loom, and someone has cut a
//! thread. You relive the sagas woven into it to find out who. Each realm is one saga; the
//! frame chapters between them carry the mystery from realm to realm.
//!
//! Most chapters are a scene and a goal. A few end in a choice; every option of a choice is
//! worth about the same, so no choice can ruin a build. They change the story's words, a
//! title, and in one case who stands with you against the boss.

use std::collections::BTreeMap;

use crate::data::{self, God};

#[derive(Debug, Clone, Copy)]
pub enum Goal {
    /// Win hunts (in a region, against a monster).
    Wins { region: Option<&'static str>, monster: Option<&'static str>, n: u32 },
    /// Collect expeditions (from a region).
    Expeditions { region: Option<&'static str>, n: u32 },
    /// Forge gear or make supplies.
    Forge { n: u32 },
    /// Know this many facts about the realm boss.
    Facts(u32),
    Level(u32),
    Boss,
    /// Give this much tribute at any altar.
    Give(i64),
}

impl Goal {
    /// Goals counted from what happens (the rest are read from the player).
    pub fn counted(&self) -> bool {
        matches!(self, Goal::Wins { .. } | Goal::Expeditions { .. } | Goal::Forge { .. } | Goal::Give(_))
    }
    pub fn target(&self) -> i64 {
        match self {
            Goal::Wins { n, .. } | Goal::Expeditions { n, .. } | Goal::Forge { n } => *n as i64,
            Goal::Facts(n) | Goal::Level(n) => *n as i64,
            Goal::Boss => 1,
            Goal::Give(n) => *n,
        }
    }
    pub fn describe(&self) -> String {
        let region = |r: &Option<&str>| r.and_then(data::region).map(|r| format!(" in {}", r.name)).unwrap_or_default();
        match self {
            Goal::Wins { monster: Some(m), n, .. } => format!("Defeat {n} {}", data::monster(m).map(|m| plural(m.name)).unwrap_or_default()),
            Goal::Wins { region: r, n, .. } => format!("Win {n} hunt{}{}", if *n == 1 { "" } else { "s" }, region(r)),
            Goal::Expeditions { region: Some(r), n } => format!("Come back from {} expedition{} to {}", if *n == 1 { "an".to_string() } else { n.to_string() }, if *n == 1 { "" } else { "s" }, data::region(r).map(|x| x.name).unwrap_or("")),
            Goal::Expeditions { n, .. } => format!("Come back from {} expedition{}", if *n == 1 { "an".to_string() } else { n.to_string() }, if *n == 1 { "" } else { "s" }),
            Goal::Forge { n } => format!("Make {} thing{} at the forge", if *n == 1 { "one".to_string() } else { n.to_string() }, if *n == 1 { "" } else { "s" }),
            Goal::Facts(n) => format!("Learn {n} things about the Barrow King"),
            Goal::Level(n) => format!("Reach level {n}"),
            Goal::Boss => "Defeat the Barrow King".into(),
            Goal::Give(n) => format!("Give {n} tribute at the temple"),
        }
    }
}

fn plural(name: &str) -> String {
    if name.ends_with('y') && !name.ends_with("ey") { format!("{}ies", &name[..name.len() - 1]) } else if name.ends_with('s') { name.into() } else { format!("{name}s") }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Reward {
    pub gold: i64,
    pub xp: i64,
    pub items: &'static [(&'static str, i64)],
    /// Tribute to your patron's altar (counts for the server too).
    pub tribute: i64,
    pub title: Option<&'static str>,
}

#[derive(Debug, Clone, Copy)]
pub struct Choice {
    pub id: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
    /// What happens, shown once chosen.
    pub after: &'static str,
    pub reward: Reward,
}

#[derive(Debug, Clone, Copy)]
pub struct Chapter {
    pub id: &'static str,
    pub title: &'static str,
    pub place: &'static str,
    pub text: &'static str,
    /// A line from your patron, in [`data::GODS`] order.
    pub patron: Option<[&'static str; 3]>,
    pub goals: &'static [Goal],
    pub choices: &'static [Choice],
    pub reward: Reward,
    /// The claim button.
    pub claim: &'static str,
}

pub const NO: Reward = Reward { gold: 0, xp: 0, items: &[], tribute: 0, title: None };

/// The Loom's opening, Midgard's saga, and the Loom again. Later realms append theirs.
pub const CHAPTERS: &[Chapter] = &[
    Chapter {
        id: "loom",
        title: "The Loom",
        place: "Between the worlds",
        text: "Your patron leads you into a hall with no walls. Three Norns and three Fates sit at one loom, weaving every saga that ever was, side by side. One thread hangs cut.\n\n\"Someone is unweaving the old stories,\" says Urd. \"Live them again, and find out who.\"\n\nYour fingers touch the cloth. Then cold, and salt in your mouth.",
        patron: Some([
            "Thor cracks his knuckles. \"Find them. Then I hit them.\"",
            "Athena studies the cut end. \"Clean. Whoever did this knew exactly which thread to take.\"",
            "Hades says nothing. He knows what it is to have something taken.",
        ]),
        goals: &[],
        choices: &[],
        reward: Reward { items: &[("potion", 2)], ..NO },
        claim: "Touch the Loom",
    },
    Chapter {
        id: "salt",
        title: "Salt and Song",
        place: "Grey Shore, Midgard",
        text: "You wake on the Grey Shore, centuries ago, in a life that isn't yours. A fisherman's widow drags you out of the surf.\n\n\"The nixies sing louder every night since the King under the mound woke,\" she says. \"Make yourself useful.\"",
        patron: None,
        goals: &[Goal::Wins { region: Some("shore"), monster: None, n: 3 }],
        choices: &[],
        reward: Reward { gold: 120, xp: 150, ..NO },
        claim: "Walk to the village",
    },
    Chapter {
        id: "bread",
        title: "Iron and Bread",
        place: "Skagi village",
        text: "Skagi is half empty. The smoke from the longhouse is thin.\n\n\"Go out and bring something back,\" the widow says, pressing bread into your hands. \"Iron, herbs, anything. People here have stopped going out.\"",
        patron: None,
        goals: &[Goal::Expeditions { region: None, n: 1 }],
        choices: &[],
        reward: Reward { xp: 120, items: &[("rations", 3)], ..NO },
        claim: "Bring it to the longhouse",
    },
    Chapter {
        id: "singer",
        title: "The Drowned Singer",
        place: "Grey Shore",
        text: "One nixie sings a name over and over: *Hrolf. Hrolf. Hrolf.* The others go quiet when she does.\n\nShe's the one leading them.",
        patron: None,
        goals: &[Goal::Wins { region: None, monster: Some("nixie"), n: 2 }],
        choices: &[
            Choice {
                id: "spare",
                label: "Spare her",
                hint: "Sea amber, and she'll remember you",
                after: "You lower your weapon. She sinks back into the grey water, and when the tide goes out there's amber on the rocks where she sat.",
                reward: Reward { items: &[("amber", 4)], title: Some("Friend of Nixies"), ..NO },
            },
            Choice {
                id: "silence",
                label: "Silence her",
                hint: "Her hoard of drowned men's gold",
                after: "The shore goes quiet. In her nest under the rocks: a drowned man's purse, and the rings of a dozen more.",
                reward: Reward { gold: 220, title: Some("Siren-silencer"), ..NO },
            },
        ],
        reward: Reward { xp: 200, ..NO },
        claim: "Decide",
    },
    Chapter {
        id: "jarl",
        title: "The Jarl of Skagi",
        place: "Skagi longhouse",
        text: "Jarl Sigrun has a forge, a cold one, and no smith left to work it.\n\n\"Hrolf was our jarl once,\" she says. \"They buried him with his crown in the Barrow Fields. Now he wants his village back.\" She points at the anvil. \"Make something worth carrying and you can stay.\"",
        patron: None,
        goals: &[Goal::Forge { n: 1 }],
        choices: &[],
        reward: Reward { gold: 150, xp: 150, ..NO },
        claim: "Show the jarl",
    },
    Chapter {
        id: "pines",
        title: "Into Wolfwood",
        place: "Wolfwood",
        text: "Raiders took Skagi's last cart into the pines. The wolves have been strange since, Sigrun says. Bolder, and quieter.",
        patron: Some([
            "\"Wolves,\" Thor says. \"I like wolves. Try not to kill the good ones.\"",
            "\"Watch where they lead you,\" Athena says. \"Animals run from what's wrong.\"",
            "\"They're running from the dead,\" Hades says. \"Follow them.\"",
        ]),
        goals: &[Goal::Expeditions { region: Some("wolfwood"), n: 1 }],
        choices: &[],
        reward: Reward { xp: 250, items: &[("pelt", 3)], ..NO },
        claim: "Follow the tracks",
    },
    Chapter {
        id: "wolfmother",
        title: "The Wolf-Mother",
        place: "Deep Wolfwood",
        text: "Something huge is killing the jarl's hunting wolves. When you find her, she's standing over a draugr she has torn apart, and she's bleeding.\n\nShe's been fighting the dead on her own.",
        patron: None,
        goals: &[Goal::Wins { region: Some("wolfwood"), monster: None, n: 4 }],
        choices: &[
            Choice {
                id: "hunt",
                label: "Take her fangs",
                hint: "Wolf fangs for the forge",
                after: "It's a hard fight and a sad one. Her fangs are long as daggers.",
                reward: Reward { items: &[("fang", 3)], title: Some("Wolf-bane"), ..NO },
            },
            Choice {
                id: "offer",
                label: "Leave her an offering",
                hint: "Your patron notices",
                after: "You leave her meat and back away. She eats, and watches you go. Somewhere above, your patron smiles.",
                reward: Reward { tribute: 300, title: Some("Wolf-friend"), ..NO },
            },
        ],
        reward: Reward { xp: 300, ..NO },
        claim: "Decide",
    },
    Chapter {
        id: "tracks",
        title: "Cold Tracks",
        place: "The edge of the Barrow Fields",
        text: "The raiders' tracks end at the grave mounds. Frost in the footprints, in summer.\n\nBefore you face whatever is under the stone, learn what it is.",
        patron: None,
        goals: &[Goal::Facts(2)],
        choices: &[],
        reward: Reward { xp: 300, items: &[("potion", 2)], ..NO },
        claim: "Tell Sigrun",
    },
    Chapter {
        id: "draugr",
        title: "Draugr Night",
        place: "Barrow Fields",
        text: "At night the mounds open and Hrolf's warriors walk out, still in their mail. They're carrying something back inside each time.\n\nThinning their numbers now will make the King's hall emptier later.",
        patron: None,
        goals: &[Goal::Wins { region: None, monster: Some("draugr"), n: 3 }],
        choices: &[],
        reward: Reward { gold: 200, xp: 350, items: &[("frost", 1)], ..NO },
        claim: "Back to Skagi",
    },
    Chapter {
        id: "volva",
        title: "The Völva's Price",
        place: "Skagi",
        text: "Skagi's völva hasn't spoken in a year. She speaks now.\n\n\"Hrolf did not wake by himself. Someone opened his mound from inside the Loom, and left the door open.\" She looks at you as if she knows where you came from. \"Choose who goes into the dark with you.\"",
        patron: Some([
            "\"Take the shieldmaiden,\" Thor says. \"Then again, I'd take everyone.\"",
            "\"Each is worth the same,\" Athena says. \"Pick the one your fighting needs.\"",
            "\"Someone opened a door between the dead and the living,\" Hades says quietly. \"That's my business.\"",
        ]),
        goals: &[Goal::Level(12)],
        choices: &[
            Choice {
                id: "shieldmaiden",
                label: "Eydis the shieldmaiden",
                hint: "In the fight: catches the King's first slam",
                after: "Eydis shoulders her shield. \"I've buried enough of Skagi,\" she says. \"Not one more.\"",
                reward: NO,
            },
            Choice {
                id: "volva",
                label: "The völva",
                hint: "In the fight: reveals one thing you haven't scouted",
                after: "The völva wraps herself in her cloak. \"I'll tell you what he'll do,\" she says, \"a moment before he does it.\"",
                reward: NO,
            },
            Choice {
                id: "jarlsmen",
                label: "The jarl's men",
                hint: "In the fight: +8% damage",
                after: "Sigrun's last six warriors bang their shields. It's not many. It's loud.",
                reward: NO,
            },
        ],
        reward: Reward { xp: 300, ..NO },
        claim: "Decide",
    },
    Chapter {
        id: "mound",
        title: "Under the Mound",
        place: "The King's barrow",
        text: "The stone rolls aside by itself. Inside, on a throne of grave-goods, Hrolf the Barrow King opens his eyes. Around his neck, on a cord, hangs a single golden thread.",
        patron: None,
        goals: &[Goal::Boss],
        choices: &[],
        reward: Reward { gold: 300, title: Some("Mound-breaker"), ..NO },
        claim: "Take the thread",
    },
    Chapter {
        id: "apple",
        title: "A Golden Apple",
        place: "The Loom",
        text: "You wake at the Loom with the golden thread in your fist. The Norns tie it back. Skagi's saga is whole again.\n\nBut at the cut, the Fates find two things that don't belong to any saga: a golden apple, and a sprig of mistletoe.",
        patron: Some([
            "\"Mistletoe,\" Thor says, and the hall goes cold. \"I know whose that is.\"",
            "\"An apple of discord,\" Athena says. \"Eris. Or someone who wants us to think so.\"",
            "\"Two tricksters,\" Hades says. \"Or one, wearing both faces.\"",
        ]),
        goals: &[],
        choices: &[],
        reward: Reward { xp: 500, title: Some("Thread-finder"), ..NO },
        claim: "Wait for the next thread",
    },
];

pub fn chapter(i: usize) -> Option<&'static Chapter> {
    CHAPTERS.get(i)
}

/// How far a goal is, given counters and what the player is.
pub fn goal_progress(goal: &Goal, counter: i64, level: u32, facts: usize, boss: bool) -> i64 {
    match goal {
        Goal::Facts(_) => facts as i64,
        Goal::Level(_) => level as i64,
        Goal::Boss => i64::from(boss),
        _ => counter,
    }
}

/// Something that happened, for the saga and the bounties to count.
#[derive(Debug, Clone, Copy)]
pub enum Event<'a> {
    Win { region: &'a str, monster: &'a str },
    Expedition { region: &'a str },
    Forged,
    Gave(i64),
}

impl Goal {
    /// How much this event moves the goal.
    pub fn counts(&self, ev: Event) -> i64 {
        match (self, ev) {
            (Goal::Wins { region, monster, .. }, Event::Win { region: r, monster: m }) => {
                i64::from(region.is_none_or(|x| x == r) && monster.is_none_or(|x| x == m))
            }
            (Goal::Expeditions { region, .. }, Event::Expedition { region: r }) => i64::from(region.is_none_or(|x| x == r)),
            (Goal::Forge { .. }, Event::Forged) => 1,
            (Goal::Give(_), Event::Gave(n)) => n,
            _ => 0,
        }
    }
}

// ---- bounties ----

/// One of today's three bounties.
#[derive(Debug, Clone)]
pub struct Bounty {
    pub goal: Goal,
    pub gold: i64,
    pub xp: i64,
}

fn mix(a: u64, b: u64) -> u64 {
    let mut r = crate::engine::Rng::new(a ^ b.rotate_left(29));
    r.next_u64()
}

/// Today's bounties for a player: picked from the day and the player, so they're the same all
/// day and differ between players. `region` is where the player hunts comfortably.
pub fn bounties(user: u64, day: i64, level: u32, region: &'static data::Region) -> Vec<Bounty> {
    let mut r = crate::engine::Rng::new(mix(user, day as u64));
    let monster = *r.pick(region.monsters);
    let pool = [
        Goal::Wins { region: None, monster: None, n: 5 },
        Goal::Wins { region: Some(region.id), monster: None, n: 4 },
        Goal::Wins { region: None, monster: Some(monster), n: 2 },
        Goal::Expeditions { region: None, n: 1 },
        Goal::Expeditions { region: Some(region.id), n: 1 },
        Goal::Forge { n: 1 },
        Goal::Give(200 + 20 * level as i64),
    ];
    let mut picked: Vec<usize> = Vec::new();
    while picked.len() < 3 {
        let i = (r.next_u64() % pool.len() as u64) as usize;
        // Never two of the same kind.
        let kind = |g: &Goal| std::mem::discriminant(g);
        if !picked.iter().any(|j| kind(&pool[*j]) == kind(&pool[i])) {
            picked.push(i);
        }
    }
    picked
        .into_iter()
        .map(|i| Bounty { goal: pool[i], gold: 30 + 8 * level as i64, xp: 60 + 25 * level as i64 })
        .collect()
}

/// Days since the epoch, the bounties' key.
pub fn day(now_ms: i64) -> i64 {
    now_ms.div_euclid(86_400_000)
}

/// The ally chosen at the völva, if any.
pub fn ally(choices: &BTreeMap<String, String>) -> Option<&str> {
    choices.get("volva").map(String::as_str)
}

pub fn patron_line(ch: &Chapter, g: God) -> Option<&'static str> {
    ch.patron.map(|l| l[g as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No choice is a trap: options of one choice are worth about the same in tribute value
    /// (gold 1:1, items at their altar value, tribute as itself).
    #[test]
    fn choices_are_sidegrades() {
        let worth = |r: &Reward| r.gold + r.tribute + r.items.iter().map(|(id, n)| data::item(id).map(|i| i.tribute.max(10)).unwrap_or(10) * n).sum::<i64>() * 4;
        for ch in CHAPTERS {
            if ch.choices.len() < 2 || ch.id == "volva" {
                continue;
            }
            let w: Vec<i64> = ch.choices.iter().map(|c| worth(&c.reward)).collect();
            let (lo, hi) = (*w.iter().min().unwrap(), *w.iter().max().unwrap());
            assert!(hi <= lo * 2, "{}: options worth {w:?}", ch.id);
        }
    }

    /// Only some chapters end in a choice, and choices aren't back to back.
    #[test]
    fn choices_are_spaced_out() {
        let with: Vec<usize> = CHAPTERS.iter().enumerate().filter(|(_, c)| !c.choices.is_empty()).map(|(i, _)| i).collect();
        assert!(with.len() * 3 <= CHAPTERS.len(), "too many choices: {with:?}");
        for w in with.windows(2) {
            assert!(w[1] - w[0] >= 2, "choices back to back at {w:?}");
        }
    }

    #[test]
    fn bounties_are_stable_and_different() {
        let r = data::region("shore").unwrap();
        let a = bounties(1, 100, 5, r);
        let b = bounties(1, 100, 5, r);
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
        assert_eq!(a.len(), 3);
        let kinds: std::collections::HashSet<_> = a.iter().map(|x| std::mem::discriminant(&x.goal)).collect();
        assert_eq!(kinds.len(), 3);
    }

    #[test]
    fn goals_reference_real_things() {
        for ch in CHAPTERS {
            for g in ch.goals {
                if let Goal::Wins { region, monster, .. } = g {
                    assert!(region.is_none_or(|r| data::region(r).is_some()), "{}", ch.id);
                    assert!(monster.is_none_or(|m| data::monster(m).is_some()), "{}", ch.id);
                }
                assert!(!g.describe().is_empty());
            }
            for c in ch.choices {
                for (id, _) in c.reward.items {
                    assert!(data::item(id).is_some());
                }
            }
        }
    }
}
