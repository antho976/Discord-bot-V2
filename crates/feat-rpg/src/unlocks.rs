//! What's open to a player. A new player starts with hunts, their character and the saga;
//! everything else opens with levels (or sooner, when the saga gets there), one at a time, and
//! the overview says so once when it does.

use crate::data::{self, Feature, Unlock};
use crate::emoji::get as e;
use crate::quests;
use crate::store::{Modules, Player};

/// Whether the server has the system switched on at all.
pub fn enabled(m: &Modules, f: Feature) -> bool {
    match f {
        Feature::Expedition | Feature::Forge | Feature::Temple => true,
        Feature::Encounters => m.encounters,
        Feature::Bounties => m.bounties,
        Feature::Elites => m.elites,
        Feature::Dungeons => m.dungeons,
        Feature::Townsfolk => m.quests,
        Feature::Labyrinth => m.labyrinth,
        Feature::Market => m.market,
        Feature::Guild => m.guilds,
        Feature::Titans => m.titans,
    }
}

fn chapter_reached(p: &Player, id: &str) -> bool {
    quests::CHAPTERS
        .iter()
        .position(|c| c.id == id)
        .is_some_and(|i| p.saga >= i)
}

/// Whether the player has reached it (by level, or by the saga).
pub fn open(p: &Player, f: Feature) -> bool {
    let u = data::unlock(f);
    p.level >= u.level || u.chapter.is_some_and(|c| chapter_reached(p, c))
}

/// Open, or already in the middle of it (a run, an expedition out, a guild joined before
/// unlocks existed): never lock someone out of something they started.
pub fn usable(p: &Player, f: Feature) -> bool {
    open(p, f)
        || match f {
            Feature::Expedition => p.expedition.is_some(),
            Feature::Labyrinth => p.run.is_some(),
            Feature::Dungeons => p.delve.is_some(),
            Feature::Guild => p.clan.is_some(),
            Feature::Townsfolk => !p.side.is_empty(),
            Feature::Bounties => p.bounty.iter().any(|b| b.0 > 0 || b.1),
            Feature::Titans => p
                .fight
                .as_ref()
                .is_some_and(|f| f.kind == crate::engine::combat::Kind::Shared),
            _ => false,
        }
}

/// The system a screen belongs to, if it isn't open from the start.
pub fn for_screen(screen: &str) -> Option<Feature> {
    Some(match screen {
        "exp" | "map" => Feature::Expedition,
        s if s.starts_with("region_") => Feature::Expedition,
        "forge" => Feature::Forge,
        "bounties" => Feature::Bounties,
        "temple" | "givethor" | "giveathena" | "givehades" => Feature::Temple,
        "dungeons" | "dungeon" => Feature::Dungeons,
        "quests" => Feature::Townsfolk,
        "lab" => Feature::Labyrinth,
        "market" => Feature::Market,
        "guild" | "guildfind" | "guildmanage" | "raid" => Feature::Guild,
        "titan" => Feature::Titans,
        _ => return None,
    })
}

/// Whether a tutorial step is about to introduce this system itself (so the news would say it
/// twice). Once the step is done, or the tutorial skipped, the news is the one to say it.
fn tutorial_covers(p: &Player, f: Feature) -> bool {
    use crate::game::Tut;
    let step = match f {
        Feature::Expedition => Tut::Expedition,
        Feature::Forge => Tut::Forge,
        Feature::Labyrinth => Tut::Labyrinth,
        _ => return false,
    };
    p.tutorial < Tut::ALL && !p.tutorial_has(step)
}

/// The newest opened system the player hasn't looked at yet, to say so on the overview.
pub fn news<'a>(p: &Player, m: &Modules) -> Option<&'a Unlock> {
    data::UNLOCKS.iter().find(|u| {
        !u.why.is_empty()
            && enabled(m, u.feature)
            && open(p, u.feature)
            && p.unlocks & u.bit == 0
            && !tutorial_covers(p, u.feature)
    })
}

/// The next system still to open, for a quiet line on the menus.
pub fn next<'a>(p: &Player, m: &Modules, among: &[Feature]) -> Option<&'a Unlock> {
    data::UNLOCKS
        .iter()
        .filter(|u| among.contains(&u.feature) && !u.why.is_empty())
        .find(|u| enabled(m, u.feature) && !open(p, u.feature))
}

/// "-# 🔒 Next: The Labyrinth at level 8", or nothing.
pub fn next_line(p: &Player, m: &Modules, among: &[Feature]) -> Option<String> {
    next(p, m, among).map(|u| format!("-# {} Next: {} at level {}", e("lock"), u.name, u.level))
}

/// What opened between two levels, for the level-up line ("Now open: Dungeons").
pub fn opened_line(p: &Player, m: &Modules, from: u32) -> String {
    opened(p, m, from, p.saga)
}

/// What's open now that wasn't at `from_level` and saga chapter `from_saga`.
pub fn opened(p: &Player, m: &Modules, from_level: u32, from_saga: usize) -> String {
    let was = |u: &Unlock| {
        from_level >= u.level
            || u.chapter.is_some_and(|c| {
                quests::CHAPTERS
                    .iter()
                    .position(|x| x.id == c)
                    .is_some_and(|i| from_saga >= i)
            })
    };
    let names: Vec<&str> = data::UNLOCKS
        .iter()
        .filter(|u| !u.why.is_empty() && enabled(m, u.feature) && open(p, u.feature) && !was(u))
        .map(|u| u.name)
        .collect();
    if names.is_empty() {
        String::new()
    } else {
        format!("\n{} **Now open:** {}", e("check"), names.join(", "))
    }
}
