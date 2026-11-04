//! A player clicking through the whole game, the way Discord would send the clicks, checking
//! that every screen is a message Discord accepts.

use std::collections::HashSet;

use bot_core::rusqlite::params;
use bot_core::twilight_model::channel::message::component::Component;
use bot_core::{Config, Ctx, Db};
use feat_rpg::data::{self, God};
use feat_rpg::engine::expedition::Rates;
use feat_rpg::game::{self, Route};
use feat_rpg::store;
use feat_rpg::ui::Screen;

const G: u64 = 10;
const U: u64 = 20;

fn ctx() -> Ctx {
    let db = Db::memory().unwrap();
    db.migrate("rpg", store::MIGRATIONS).unwrap();
    Ctx::new(Config::preview(3999), db).unwrap()
}

/// Discord's limits for a Components V2 message.
fn check(s: &Screen) {
    fn walk(c: &Component, n: &mut usize, text: &mut usize, ids: &mut Vec<String>) {
        *n += 1;
        match c {
            Component::Container(x) => x.components.iter().for_each(|c| walk(c, n, text, ids)),
            Component::ActionRow(x) => {
                assert!(
                    x.components.len() <= 5,
                    "a row has {} items",
                    x.components.len()
                );
                x.components.iter().for_each(|c| walk(c, n, text, ids));
            }
            Component::Section(x) => {
                assert!((1..=3).contains(&x.components.len()));
                x.components.iter().for_each(|c| walk(c, n, text, ids));
                walk(&x.accessory, n, text, ids);
            }
            Component::TextDisplay(t) => *text += t.content.chars().count(),
            Component::Button(b) => {
                let id = b.custom_id.clone().unwrap();
                assert!(id.len() <= 100, "custom id too long: {id}");
                assert!(b.label.as_ref().is_none_or(|l| l.chars().count() <= 80));
                ids.push(id);
            }
            Component::SelectMenu(m) => {
                assert!(
                    m.custom_id.len() <= 100,
                    "custom id too long: {}",
                    m.custom_id
                );
                let opts = m.options.as_ref().unwrap();
                assert!(!opts.is_empty() && opts.len() <= 25);
                let values: HashSet<&str> = opts.iter().map(|o| o.value.as_str()).collect();
                assert_eq!(
                    values.len(),
                    opts.len(),
                    "duplicate option values in {}",
                    m.custom_id
                );
                assert!(opts.iter().filter(|o| o.default).count() <= 1);
                ids.push(m.custom_id.clone());
            }
            _ => {}
        }
    }
    let (mut n, mut text, mut ids) = (0, 0, Vec::new());
    for c in s.components() {
        walk(&c, &mut n, &mut text, &mut ids);
    }
    assert!(n <= 40, "{n} components");
    assert!(text <= 4000, "{text} characters of text");
    let unique: HashSet<&String> = ids.iter().collect();
    assert_eq!(unique.len(), ids.len(), "duplicate custom ids: {ids:?}");
}

fn text_of(s: &Screen) -> String {
    format!("{:?}", s.blocks)
}

/// Click a button: build the route the way the button's custom id would carry it.
fn click(ctx: &Ctx, screen: &str, action: &str, arg: &str, values: &[&str]) -> Screen {
    let version = store::get(&ctx.db, G, U).unwrap().unwrap().version;
    click_at(ctx, screen, action, arg, values, version)
}

fn click_at(
    ctx: &Ctx,
    screen: &str,
    action: &str,
    arg: &str,
    values: &[&str],
    version: i64,
) -> Screen {
    let route = Route {
        owner: U,
        screen: screen.into(),
        action: action.into(),
        arg: arg.into(),
        version,
    };
    let values: Vec<String> = values.iter().map(|v| v.to_string()).collect();
    let out = store::run(&ctx.db, G, U, Some("Antho"), |g| {
        game::handle(ctx, g, &route, &values, Rates::default())
    })
    .unwrap()
    .unwrap();
    check(&out.screen);
    out.screen
}

/// Raise a player to a level (unlocks come with levels).
fn level_to(ctx: &Ctx, user: u64, level: u32) {
    store::run(&ctx.db, G, user, None, |g| {
        let need: i64 = (g.p.level..level).map(data::xp_to_next).sum::<i64>() - g.p.xp;
        g.xp(need.max(0)).map(|_| ())
    })
    .unwrap();
}

fn grant(ctx: &Ctx, items: &[(&str, i64)], gold: i64) {
    store::run(&ctx.db, G, U, None, |g| {
        for (id, n) in items {
            g.item(id, *n, "test")?;
        }
        g.xp(5000)?;
        g.gold(gold, "test")
    })
    .unwrap();
}

#[test]
fn a_new_player_plays_through_midgard() {
    let rt = bot_core::tokio::runtime::Runtime::new().unwrap();
    let _e = rt.enter();
    let ctx = ctx();
    check(&game::patron_choice(U, "Antho"));
    store::create(&ctx.db, G, U, "Antho", God::Thor).unwrap();

    // Every screen draws within Discord's limits from the start.
    for s in [
        "hub",
        "goals",
        "exp",
        "hunt",
        "boss",
        "forge",
        "pack",
        "temple",
        "givethor",
        "giveathena",
        "givehades",
        "patron",
        "help",
        "saga",
        "bounties",
        "lab",
        "hero",
        "skagi",
        "combat",
        "questlog",
        "guildfind",
        "guildmanage",
        "guild",
        "titan",
        "raid",
        "dungeons",
        "dungeon",
        "map",
        "region_shore",
        "region_barrow",
        "quests",
        "market",
    ] {
        click(&ctx, s, "", "", &[]);
    }

    // A new player has hunts, their character and the saga; the rest opens with levels.
    let s = click(&ctx, "lab", "enter", "", &[]);
    assert!(text_of(&s).contains("opens at level"), "{}", text_of(&s));
    assert!(store::get(&ctx.db, G, U).unwrap().unwrap().run.is_none());
    let s = click(&ctx, "combat", "", "", &[]);
    assert!(!text_of(&s).contains("Labyrinth"));
    assert!(text_of(&s).contains("Next: Expeditions at level 2"));
    level_to(&ctx, U, 2);
    // Said once on the overview, until you look.
    let s = click(&ctx, "tut", "skip", "", &[]);
    assert!(text_of(&s).contains("Expeditions"), "{}", text_of(&s));
    click(&ctx, "exp", "", "", &[]);
    let s = click(&ctx, "hub", "", "", &[]);
    assert!(!text_of(&s).contains("-# New"), "{}", text_of(&s));

    // Plan with the menus, then send out.
    let s = click(
        &ctx,
        "exp",
        "set",
        "region|shore.normal.4.rations",
        &["wolfwood"],
    );
    assert!(text_of(&s).contains("Forecast"));
    let before = store::get(&ctx.db, G, U).unwrap().unwrap();
    click(&ctx, "exp", "go", "shore.normal.1.rations", &[]);
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert!(p.expedition.is_some());
    assert_eq!(p.have("rations"), before.have("rations") - 1);

    // The same button again, from before the send: refused, nothing doubles.
    let s = click_at(
        &ctx,
        "exp",
        "go",
        "shore.normal.1.rations",
        &[],
        before.version,
    );
    assert!(text_of(&s).contains("before your last move"));

    // Not back yet: collect does nothing.
    click(&ctx, "exp", "collect", "", &[]);
    assert!(
        store::get(&ctx.db, G, U)
            .unwrap()
            .unwrap()
            .expedition
            .is_some()
    );

    // Back: collect pays out once.
    ctx.db
        .with(|c| c.execute("UPDATE rpg_expeditions SET ends_at = 0", []))
        .unwrap();
    let gold_before = store::get(&ctx.db, G, U).unwrap().unwrap().gold;
    let s = click(&ctx, "exp", "collect", "", &[]);
    assert!(text_of(&s).contains("Brought back"));
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert!(p.expedition.is_none());
    assert!(p.gold > gold_before);
    assert!(
        p.tribute[God::Thor as usize] > 0,
        "expedition tribute goes to the patron's altar"
    );
    let gold_after = p.gold;
    click(&ctx, "exp", "collect", "", &[]);
    assert_eq!(store::get(&ctx.db, G, U).unwrap().unwrap().gold, gold_after);

    // Hunt until the fight ends.
    click(&ctx, "hunt", "start", "shore", &[]);
    for _ in 0..80 {
        let p = store::get(&ctx.db, G, U).unwrap().unwrap();
        let Some(f) = p.fight else { break };
        let a = if matches!(f.intent(), data::Move::Heavy | data::Move::Roar) || f.stamina < 1 {
            "guard"
        } else {
            "strike"
        };
        click(&ctx, "hunt", "act", a, &[]);
    }
    assert!(
        store::get(&ctx.db, G, U).unwrap().unwrap().fight.is_none(),
        "the hunt never ended"
    );

    // Hurt: too hurt to hunt, a potion fixes it.
    store::run(&ctx.db, G, U, None, |g| g.set_hp(1)).unwrap();
    let s = click(&ctx, "hunt", "start", "shore", &[]);
    assert!(text_of(&s).contains("too hurt"));
    let potions = store::get(&ctx.db, G, U).unwrap().unwrap().have("potion");
    if potions > 0 {
        click(&ctx, "hunt", "drink", "", &[]);
        let p = store::get(&ctx.db, G, U).unwrap().unwrap();
        assert_eq!(p.have("potion"), potions - 1);
        assert!(p.hp() > 1);
    }

    // Forge supplies from materials.
    grant(
        &ctx,
        &[
            ("herbs", 40),
            ("fish", 30),
            ("iron", 50),
            ("yew", 30),
            ("amber", 4),
        ],
        2000,
    );
    let potions = store::get(&ctx.db, G, U).unwrap().unwrap().have("potion");
    click(&ctx, "forge", "make", "", &["potion"]);
    assert_eq!(
        store::get(&ctx.db, G, U).unwrap().unwrap().have("potion"),
        potions + 2
    );
    // A piece of gear and an upgrade.
    click(&ctx, "forge", "gear", "ambercharm", &[]);
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    let charm = p.equipped(data::Slot::Charm).expect("the charm is worn").id;
    click(&ctx, "forge", "upgrade", "", &[&charm.to_string()]);
    assert_eq!(
        store::get(&ctx.db, G, U)
            .unwrap()
            .unwrap()
            .equipped(data::Slot::Charm)
            .unwrap()
            .upgrade,
        1
    );

    // Give spare materials and gold at Athena's altar.
    click(&ctx, "temple", "give", "athena.spare", &[]);
    click(&ctx, "temple", "give", "athena.gold100", &[]);
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert!(p.tribute[God::Athena as usize] > 100);
    let o = ctx
        .db
        .with(|c| Ok(store::offerings(c, G, store::week_now()).unwrap()))
        .unwrap();
    assert_eq!(o[God::Athena as usize], p.tribute[God::Athena as usize]);
    assert!(
        p.items
            .iter()
            .filter(|(id, _)| data::item(id).is_some_and(|i| i.kind == data::Kind::Material))
            .all(|(_, n)| *n <= 20)
    );

    // Face the boss far too early: it's a real fight that can be lost or fled.
    store::run(&ctx.db, G, U, None, |g| g.set_hp(9999)).unwrap();
    click(&ctx, "boss", "start", "", &[]);
    assert!(store::get(&ctx.db, G, U).unwrap().unwrap().fight.is_some());
    click(&ctx, "boss", "act", "strike", &[]);
    click(&ctx, "boss", "leave", "", &[]);
    assert!(store::get(&ctx.db, G, U).unwrap().unwrap().fight.is_none());

    // The ledger saw every movement and nothing went below zero.
    let neg: i64 = ctx
        .db
        .with(|c| {
            c.query_row("SELECT COUNT(*) FROM rpg_items WHERE qty < 0", [], |r| {
                r.get(0)
            })
        })
        .unwrap();
    assert_eq!(neg, 0);
    let ledger: i64 = ctx
        .db
        .with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM rpg_ledger WHERE guild_id = ?1",
                params![G as i64],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert!(ledger > 20);
}

#[test]
fn beating_the_boss_opens_heroic_and_the_lesser_vow() {
    let rt = bot_core::tokio::runtime::Runtime::new().unwrap();
    let _e = rt.enter();
    let ctx = ctx();
    store::create(&ctx.db, G, U, "Antho", God::Athena).unwrap();
    store::run(&ctx.db, G, U, None, |g| {
        g.xp(1_000_000)?;
        for id in ["barrowblade", "mantle", "ravencharm"] {
            let row = g.add_gear(data::gear(id).unwrap(), true)?;
            for _ in 0..3 {
                g.upgrade(row)?;
            }
        }
        g.tribute(God::Athena, 2000)?;
        g.item("potion", 4, "test")?;
        for f in data::FACTS {
            g.learn(f)?;
        }
        Ok(())
    })
    .unwrap();
    // Play the boss with the simulator's sensible choices until it ends; retry a few times.
    for _ in 0..6 {
        store::run(&ctx.db, G, U, None, |g| g.set_hp(99_999)).unwrap();
        click(&ctx, "boss", "start", "", &[]);
        for _ in 0..200 {
            let p = store::get(&ctx.db, G, U).unwrap().unwrap();
            let Some(ref f) = p.fight else { break };
            let a = feat_rpg::engine::combat::autoplay(
                f,
                p.have("potion"),
                (p.have("fireoil"), p.have("frostoil")),
            );
            let arg = match a {
                feat_rpg::engine::combat::Action::Strike => "strike".to_string(),
                feat_rpg::engine::combat::Action::Guard => "guard".into(),
                feat_rpg::engine::combat::Action::Potion => "potion".into(),
                feat_rpg::engine::combat::Action::Oil(e) => e.oil().into(),
                feat_rpg::engine::combat::Action::Skill(s) => s.skill().id().into(),
            };
            click(&ctx, "boss", "act", &arg, &[]);
        }
        if store::get(&ctx.db, G, U).unwrap().unwrap().boss_won {
            break;
        }
        store::run(&ctx.db, G, U, None, |g| g.item("potion", 3, "test")).unwrap();
    }
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert!(
        p.boss_won,
        "a full-power Athena never beat the boss in six tries"
    );
    assert_eq!(p.have("shard"), 3);
    // Heroic is offered now, and a lesser vow can be taken.
    let s = click(
        &ctx,
        "exp",
        "set",
        "danger|barrow.normal.4.none",
        &["heroic"],
    );
    assert!(text_of(&s).contains("Heroic"));
    click(&ctx, "patron", "vow", "", &["thor"]);
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert_eq!(p.lesser, Some(God::Thor));
    let _ = data::GODS;
}

fn hunt_until_done(ctx: &Ctx, region: &str) {
    click(ctx, "hunt", "start", region, &[]);
    for _ in 0..80 {
        let p = store::get(&ctx.db, G, U).unwrap().unwrap();
        let Some(f) = p.fight else { break };
        let a = if matches!(f.intent(), data::Move::Heavy | data::Move::Roar) || f.stamina < 1 {
            "guard"
        } else {
            "strike"
        };
        click(ctx, "hunt", "act", a, &[]);
    }
    store::run(&ctx.db, G, U, None, |g| {
        let max = g.p.max_hp();
        g.set_hp(max)
    })
    .unwrap();
}

#[test]
fn the_saga_bounties_and_encounters() {
    let rt = bot_core::tokio::runtime::Runtime::new().unwrap();
    let _e = rt.enter();
    let ctx = ctx();
    store::create(&ctx.db, G, U, "Antho", God::Hades).unwrap();

    // The Loom has no goal: claim it straight away.
    let s = click(&ctx, "saga", "", "", &[]);
    assert!(text_of(&s).contains("The Loom"));
    click(&ctx, "saga", "claim", "", &[]);
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert_eq!(p.saga, 1);
    assert_eq!(p.have("potion"), 4);

    // Chapter 2 wants three wins at the Grey Shore; claiming early is refused.
    let s = click(&ctx, "saga", "claim", "", &[]);
    assert!(text_of(&s).contains("isn't finished"));
    for _ in 0..12 {
        if store::get(&ctx.db, G, U)
            .unwrap()
            .unwrap()
            .saga_progress
            .first()
            .copied()
            .unwrap_or(0)
            >= 3
        {
            break;
        }
        hunt_until_done(&ctx, "shore");
    }
    assert!(game::chapter_done(
        &store::get(&ctx.db, G, U).unwrap().unwrap()
    ));
    // Hunt wins counted toward today's bounties too, where one asks for wins.
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    let bs = game::todays_bounties(&p);
    for (i, b) in bs.iter().enumerate() {
        if let feat_rpg::quests::Goal::Wins {
            region: None,
            monster: None,
            ..
        } = b.goal
        {
            assert!(p.bounty[i].0 >= 3, "wins weren't counted for bounty {i}");
        }
    }
    let gold = p.gold;
    click(&ctx, "saga", "claim", "", &[]);
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert_eq!(p.saga, 2);
    assert!(p.gold >= gold + 120);

    // A choice chapter: jump there, finish its goal, and choose.
    store::run(&ctx.db, G, U, None, |g| {
        g.p.saga = 3;
        g.p.saga_progress = vec![2];
        g.save_saga()
    })
    .unwrap();
    let s = click(&ctx, "saga", "claim", "nonsense", &[]);
    assert!(text_of(&s).contains("Choose one"));
    let before = store::get(&ctx.db, G, U).unwrap().unwrap();
    let amber = before.have("amber");
    // A title is worn only when none is: the shore hunts above can roll an elite (6%) whose
    // first kill already gave one.
    let worn = before.title.clone();
    click(&ctx, "saga", "claim", "spare", &[]);
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert_eq!(p.saga, 4);
    assert_eq!(p.have("amber"), amber + 4);
    assert!(p.titles.iter().any(|t| t == "Friend of Nixies"));
    assert_eq!(
        p.title.as_deref(),
        worn.as_deref().or(Some("Friend of Nixies"))
    );

    // An encounter waiting after a hunt resolves once.
    store::run(&ctx.db, G, U, None, |g| g.set_event(Some("traveller"))).unwrap();
    let xp = store::get(&ctx.db, G, U).unwrap().unwrap().xp;
    let lvl = store::get(&ctx.db, G, U).unwrap().unwrap().level;
    click(&ctx, "hunt", "event", "walk", &[]);
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert!(p.event.is_none());
    assert!(p.xp > xp || p.level > lvl);
    click(&ctx, "hunt", "event", "walk", &[]);

    // The inn heals for gold.
    store::run(&ctx.db, G, U, None, |g| {
        g.set_hp(5)?;
        g.gold(500, "test")
    })
    .unwrap();
    click(&ctx, "hunt", "inn", "", &[]);
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert_eq!(p.hp(), p.max_hp());

    // Test speed: a 4-hour expedition at x60 is back in 4 minutes.
    store::set_speed(G, 60);
    click(&ctx, "exp", "go", "shore.normal.4.none", &[]);
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    let x = p.expedition.unwrap();
    assert_eq!(x.ends_at - x.started_at, 4 * 60_000);
    store::set_speed(G, 1);

    // The völva's ally joins the boss fight.
    store::run(&ctx.db, G, U, None, |g| {
        g.p.saga_choices
            .insert("volva".into(), "shieldmaiden".into());
        g.save_saga()?;
        let max = g.p.max_hp();
        g.set_hp(max)
    })
    .unwrap();
    click(&ctx, "boss", "start", "", &[]);
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert_eq!(p.fight.unwrap().ally.as_deref(), Some("shieldmaiden"));
}

#[test]
fn the_labyrinth_gear_and_the_tutorial() {
    let rt = bot_core::tokio::runtime::Runtime::new().unwrap();
    let _e = rt.enter();
    let ctx = ctx();
    store::create(&ctx.db, G, U, "Antho", God::Athena).unwrap();
    // A strong hero, so the run goes a few floors.
    store::run(&ctx.db, G, U, None, |g| {
        g.xp(1_000_000)?;
        for id in ["barrowblade", "mantle", "ravencharm"] {
            g.add_gear(data::gear(id).unwrap(), true)?;
        }
        g.item("potion", 5, "test")
    })
    .unwrap();

    // The welcome step, then the tutorial points somewhere.
    let s = click(&ctx, "hub", "", "", &[]);
    assert!(text_of(&s).contains("How this works"));
    click(&ctx, "tut", "ok", "", &[]);

    // Into the Labyrinth.
    click(&ctx, "lab", "enter", "", &[]);
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert!(p.run.is_some());
    assert!(p.tutorial_has(game::Tut::Labyrinth));
    // Open doors until floor 6 (past the first guardian) or the run ends.
    for _ in 0..200 {
        let p = store::get(&ctx.db, G, U).unwrap().unwrap();
        let Some(run) = p.run.clone() else { break };
        if run.floor > 6 {
            break;
        }
        if let Some(ref f) = p.fight {
            let a = feat_rpg::engine::combat::autoplay(f, p.have("potion"), (0, 0));
            let arg = match a {
                feat_rpg::engine::combat::Action::Strike => "strike".to_string(),
                feat_rpg::engine::combat::Action::Guard => "guard".into(),
                feat_rpg::engine::combat::Action::Potion => "potion".into(),
                feat_rpg::engine::combat::Action::Oil(e) => e.oil().into(),
                feat_rpg::engine::combat::Action::Skill(s) => s.skill().id().into(),
            };
            click(&ctx, "lab", "act", &arg, &[]);
        } else {
            // Prefer shrines when hurt, else the first door.
            let i = if p.hp() * 2 < p.max_hp() {
                run.doors
                    .iter()
                    .position(|d| format!("{d:?}") == "Shrine")
                    .unwrap_or(0)
            } else {
                0
            };
            let id = format!("{:?}", run.doors[i]).to_lowercase();
            click(&ctx, "lab", "door", &format!("{i}.{id}"), &[]);
        }
    }
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    if p.run.is_some() {
        let before = p.gold;
        let s = click(&ctx, "lab", "out", "", &[]);
        assert!(text_of(&s).contains("carry out"), "{}", text_of(&s));
        let p = store::get(&ctx.db, G, U).unwrap().unwrap();
        assert!(p.run.is_none());
        assert!(p.gold > before);
    }
    let best: i64 = ctx
        .db
        .with(|c| {
            c.query_row("SELECT COALESCE(MAX(best), 0) FROM rpg_lab", [], |r| {
                r.get(0)
            })
        })
        .unwrap();
    assert!(best >= 5, "a full-power hero only reached floor {best}");
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert!(
        p.have("thread") >= 3,
        "the guardian should have left threads"
    );

    // Refine a worn piece with threads, then reforge it.
    store::run(&ctx.db, G, U, None, |g| {
        g.item("thread", 20, "test")?;
        g.gold(5000, "test")
    })
    .unwrap();
    // The run may have dropped better gear; refine whichever worn piece can still go up.
    let piece = data::SLOTS
        .iter()
        .filter_map(|sl| p.equipped(*sl))
        .find(|g| g.rarity != data::Rarity::Epic)
        .unwrap()
        .clone();
    click(&ctx, "forge", "refine", "", &[&piece.id.to_string()]);
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    let after = p.gear.iter().find(|g| g.id == piece.id).unwrap();
    assert_eq!(after.rarity.index(), piece.rarity.index() + 1);
    assert_eq!(after.traits.len(), piece.traits.len() + 1);
    let threads = p.have("thread");
    click(&ctx, "forge", "reforge", "", &[&piece.id.to_string()]);
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert!(p.have("thread") < threads);
    assert_eq!(
        p.gear
            .iter()
            .find(|g| g.id == piece.id)
            .unwrap()
            .traits
            .len(),
        after.traits.len()
    );
    // Traits reach fights.
    assert!(
        p.hero().mods.damage > 0.0
            || p.hero().mods.crit > 0.0
            || p.hero().mods.steal > 0.0
            || p.hero().mods.fire > 0.0
            || p.hero().mods.frost > 0.0
    );
}

fn play_fight(ctx: &Ctx, screen: &str) {
    for _ in 0..200 {
        let p = store::get(&ctx.db, G, U).unwrap().unwrap();
        let Some(ref f) = p.fight else { break };
        let a = feat_rpg::engine::combat::autoplay(
            f,
            p.have("potion"),
            (p.have("fireoil"), p.have("frostoil")),
        );
        let arg = match a {
            feat_rpg::engine::combat::Action::Strike => "strike".to_string(),
            feat_rpg::engine::combat::Action::Guard => "guard".into(),
            feat_rpg::engine::combat::Action::Potion => "potion".into(),
            feat_rpg::engine::combat::Action::Oil(e) => e.oil().into(),
            feat_rpg::engine::combat::Action::Skill(s) => s.skill().id().into(),
        };
        click(ctx, screen, "act", &arg, &[]);
    }
}

fn strong(ctx: &Ctx) {
    store::run(&ctx.db, G, U, None, |g| {
        g.xp(1_000_000)?;
        for id in ["barrowblade", "mantle", "ravencharm"] {
            let row = g.add_gear(data::gear(id).unwrap(), true)?;
            for _ in 0..3 {
                g.upgrade(row)?;
            }
        }
        g.gold(20_000, "test")?;
        g.item("potion", 10, "test")
    })
    .unwrap();
}

fn heal(ctx: &Ctx) {
    store::run(&ctx.db, G, U, None, |g| {
        let max = g.p.max_hp();
        g.set_hp(max)
    })
    .unwrap();
}

#[test]
fn guilds_raids_and_titans() {
    let rt = bot_core::tokio::runtime::Runtime::new().unwrap();
    let _e = rt.enter();
    let ctx = ctx();
    store::create(&ctx.db, G, U, "Antho", God::Thor).unwrap();
    strong(&ctx);
    // Found a guild through the form's values.
    let s = click(&ctx, "guild", "create", "", &["Ravenhold", "rvn"]);
    assert!(text_of(&s).contains("Ravenhold"), "{}", text_of(&s));
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    let clan = p.clan.clone().expect("in a guild");
    assert!(clan.leader);
    assert_eq!(clan.tag, "RVN");
    // A second player joins.
    store::create(&ctx.db, G, 21, "Mira", God::Athena).unwrap();
    level_to(&ctx, 21, 12);
    store::run(&ctx.db, G, 21, None, |g| feat_rpg::guild::join(g, clan.id)).unwrap();
    assert_eq!(
        store::get(&ctx.db, G, 21)
            .unwrap()
            .unwrap()
            .clan
            .unwrap()
            .id,
        clan.id
    );
    // Hunting earns guild XP and counts toward the weekly challenge.
    hunt_until_done(&ctx, "shore");
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert!(p.clan.as_ref().unwrap().xp > 0);
    // Treasury, upgrade, raid.
    click(&ctx, "guild", "give", "1000", &[]);
    for _ in 0..10 {
        click(&ctx, "guild", "give", "1000", &[]);
    }
    click(&ctx, "guild", "buy", "", &["mead"]);
    assert!(
        store::get(&ctx.db, G, U)
            .unwrap()
            .unwrap()
            .clan
            .unwrap()
            .has("mead")
    );
    click(&ctx, "guild", "raid", "", &[]);
    let raid = ctx
        .db
        .with(|c| Ok(feat_rpg::shared::active(c, G, "raid", clan.id).unwrap()))
        .unwrap()
        .expect("raid started");
    heal(&ctx);
    click(&ctx, "raid", "fight", "", &[]);
    play_fight(&ctx, "raid");
    let after = ctx
        .db
        .with(|c| Ok(feat_rpg::shared::by_id(c, raid.id).unwrap().unwrap()))
        .unwrap();
    assert!(after.hp < raid.hp, "the attempt dealt no damage");
    // Fell it by hand and claim.
    ctx.db
        .with(|c| {
            c.execute(
                "UPDATE rpg_shared SET hp = 0, killed_at = 1 WHERE id = ?1",
                bot_core::rusqlite::params![raid.id],
            )
        })
        .unwrap();
    let shards_before = store::get(&ctx.db, G, U).unwrap().unwrap().gear.len();
    click(&ctx, "raid", "claim", "", &[]);
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert_eq!(p.gear.len(), shards_before + 1);
    assert!(p.titles.iter().any(|t| t == "Wyrmbane"));
    // Claiming twice gives nothing more.
    click(&ctx, "raid", "claim", "", &[]);
    assert_eq!(
        store::get(&ctx.db, G, U).unwrap().unwrap().gear.len(),
        shards_before + 1
    );

    // A Titan rises, takes damage from an attempt, and attempts are limited per day.
    let text = ctx
        .db
        .with(|c| Ok(feat_rpg::shared::rise_titan(c, G, 48).unwrap()))
        .unwrap();
    assert!(text.is_some());
    assert!(
        ctx.db
            .with(|c| Ok(feat_rpg::shared::rise_titan(c, G, 48).unwrap()))
            .unwrap()
            .is_none(),
        "two Titans at once"
    );
    for _ in 0..data::TITAN_ATTEMPTS {
        heal(&ctx);
        click(&ctx, "titan", "fight", "", &[]);
        play_fight(&ctx, "titan");
    }
    heal(&ctx);
    let s = click(&ctx, "titan", "fight", "", &[]);
    assert!(text_of(&s).contains("today's attempts"), "{}", text_of(&s));
    let t = ctx
        .db
        .with(|c| Ok(feat_rpg::shared::active(c, G, "titan", 0).unwrap().unwrap()))
        .unwrap();
    assert!(t.hp < t.max_hp);

    // Leaving the guild as leader hands it on.
    click(&ctx, "guild", "leave", "", &[]);
    let m = store::get(&ctx.db, G, 21).unwrap().unwrap().clan.unwrap();
    assert!(m.leader);
}

/// Click as someone other than the main player.
fn click_as(
    ctx: &Ctx,
    user: u64,
    screen: &str,
    action: &str,
    arg: &str,
    values: &[&str],
) -> Screen {
    let version = store::get(&ctx.db, G, user).unwrap().unwrap().version;
    let route = Route {
        owner: user,
        screen: screen.into(),
        action: action.into(),
        arg: arg.into(),
        version,
    };
    let values: Vec<String> = values.iter().map(|v| v.to_string()).collect();
    let out = store::run(&ctx.db, G, user, None, |g| {
        game::handle(ctx, g, &route, &values, Rates::default())
    })
    .unwrap()
    .unwrap();
    check(&out.screen);
    out.screen
}

fn clan_of(ctx: &Ctx, user: u64) -> Option<i64> {
    store::get(&ctx.db, G, user)
        .unwrap()
        .unwrap()
        .clan
        .map(|c| c.id)
}

#[test]
fn guild_controls() {
    let rt = bot_core::tokio::runtime::Runtime::new().unwrap();
    let _e = rt.enter();
    let ctx = ctx();
    store::create(&ctx.db, G, U, "Antho", God::Thor).unwrap();
    strong(&ctx);
    click(&ctx, "guild", "create", "", &["Ravenhold", "rvn"]);
    let clan = clan_of(&ctx, U).unwrap();
    for (u, name) in [(31, "Mira"), (32, "Oskar"), (33, "Teo")] {
        store::create(&ctx.db, G, u, name, God::Athena).unwrap();
        level_to(&ctx, u, 10);
    }

    // Invite only: joining straight in is refused, asking works, and the leader accepts.
    let s = click(&ctx, "guild", "policy", "", &["invite"]);
    assert!(text_of(&s).contains("invite only"), "{}", text_of(&s));
    let s = click_as(&ctx, 31, "guild", "join", &clan.to_string(), &[]);
    assert!(text_of(&s).contains("invite only"), "{}", text_of(&s));
    assert_eq!(clan_of(&ctx, 31), None);
    let s = click_as(&ctx, 31, "guildfind", "", "", &[]);
    assert!(text_of(&s).contains("Ask to join"), "{}", text_of(&s));
    click_as(&ctx, 31, "guild", "ask", &format!("{clan}.0.0."), &[]);
    let s = click_as(&ctx, 31, "guild", "", "", &[]);
    assert!(
        text_of(&s).contains("Waiting to hear from Ravenhold"),
        "{}",
        text_of(&s)
    );
    let hub = click(&ctx, "hub", "", "", &[]);
    assert!(
        text_of(&hub).contains("1 asking to join"),
        "{}",
        text_of(&hub)
    );
    let s = click(&ctx, "guildmanage", "", "", &[]);
    assert!(text_of(&s).contains("Mira"), "{}", text_of(&s));
    click(&ctx, "guild", "accept", "31", &[]);
    assert_eq!(clan_of(&ctx, 31), Some(clan));

    // An ask can be withdrawn, and a declined one is gone.
    click_as(&ctx, 32, "guild", "ask", &format!("{clan}.0.0."), &[]);
    click_as(&ctx, 32, "guild", "withdraw", &format!("{clan}.0.0."), &[]);
    let s = click(&ctx, "guild", "accept", "32", &[]);
    assert!(text_of(&s).contains("request is gone"), "{}", text_of(&s));
    click_as(&ctx, 32, "guild", "ask", &format!("{clan}.0.0."), &[]);
    click(&ctx, "guild", "decline", "", &["32"]);
    assert_eq!(clan_of(&ctx, 32), None);

    // The leader invites; the invite shows on the player's pages and lets them in.
    click(&ctx, "guild", "invite", "", &["33"]);
    let hub = click_as(&ctx, 33, "hub", "", "", &[]);
    assert!(text_of(&hub).contains("invited you"), "{}", text_of(&hub));
    let s = click_as(&ctx, 33, "guild", "", "", &[]);
    assert!(text_of(&s).contains("invited you"), "{}", text_of(&s));
    click_as(&ctx, 33, "guild", "join", &clan.to_string(), &[]);
    assert_eq!(clan_of(&ctx, 33), Some(clan));

    // A weekly minimum: members who were in all last week and fell short are marked.
    click(&ctx, "guild", "minimum", "", &["250"]);
    assert_eq!(
        store::get(&ctx.db, G, U)
            .unwrap()
            .unwrap()
            .clan
            .unwrap()
            .min_week,
        250
    );
    let week = store::get(&ctx.db, G, U).unwrap().unwrap().week();
    ctx.db
        .with(|c| {
            c.execute(
                "UPDATE rpg_clan_members SET joined = 0, week = ?2, week_xp = 40 WHERE user_id = ?1",
                params![31, week - 1],
            )?;
            Ok(())
        })
        .unwrap();
    let s = click(&ctx, "guildmanage", "", "", &[]);
    assert!(text_of(&s).contains("under the minimum"), "{}", text_of(&s));
    assert!(
        text_of(&s).contains("Under the minimum last week (40)"),
        "{}",
        text_of(&s)
    );

    // Kicking: out, and no straight rejoin or ask for a while, unless invited again.
    click(&ctx, "guild", "policy", "", &["open"]);
    click(&ctx, "guild", "kick", "", &["31"]);
    assert_eq!(clan_of(&ctx, 31), None);
    let s = click_as(&ctx, 31, "guild", "join", &clan.to_string(), &[]);
    assert!(
        text_of(&s).contains("removed you lately"),
        "{}",
        text_of(&s)
    );
    assert_eq!(clan_of(&ctx, 31), None);
    click(&ctx, "guild", "invite", "", &["31"]);
    click_as(&ctx, 31, "guild", "join", &clan.to_string(), &[]);
    assert_eq!(clan_of(&ctx, 31), Some(clan));

    // Search: by part of the name or the tag, and open-only.
    store::run(&ctx.db, G, 32, None, |g| {
        g.gold(2000, "test")?;
        feat_rpg::guild::found(g, "Wolfwood Watch", "WW")?;
        feat_rpg::guild::set_policy(g, "invite")?;
        Ok(())
    })
    .unwrap();
    let s = click_as(&ctx, 32, "guild", "find", "", &["raven"]);
    assert!(text_of(&s).contains("Ravenhold") && !text_of(&s).contains("Wolfwood Watch"));
    let s = click_as(&ctx, 32, "guild", "find", "", &["ww"]);
    assert!(text_of(&s).contains("Wolfwood Watch"), "{}", text_of(&s));
    let s = click_as(&ctx, 32, "guild", "page", "0.1.", &[]);
    assert!(text_of(&s).contains("Ravenhold") && !text_of(&s).contains("Wolfwood Watch"));
    let s = click_as(&ctx, 32, "guild", "find", "", &["nobody:here"]);
    assert!(text_of(&s).contains("No guild matches"), "{}", text_of(&s));
}

#[test]
fn dungeons_quests_market_and_switches() {
    let rt = bot_core::tokio::runtime::Runtime::new().unwrap();
    let _e = rt.enter();
    let ctx = ctx();
    store::create(&ctx.db, G, U, "Antho", God::Hades).unwrap();
    strong(&ctx);
    // A dungeon: four rooms and the boss, and the chest the first time.
    let gear_before = store::get(&ctx.db, G, U).unwrap().unwrap().gear.len();
    click(&ctx, "dungeon", "enter", "drowned", &[]);
    for _ in 0..6 {
        let p = store::get(&ctx.db, G, U).unwrap().unwrap();
        if p.delve.is_none() {
            break;
        }
        click(&ctx, "dungeon", "next", "", &[]);
        play_fight(&ctx, "dungeon");
    }
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert!(p.delve.is_none(), "the delve never ended");
    assert_eq!(p.gear.len(), gear_before + 1, "no chest gear");
    // The side quest from Eydis counted the clear once its earlier step is done.
    store::run(&ctx.db, G, U, None, |g| g.save_side("drills", 1, 0)).unwrap();
    click(&ctx, "dungeon", "enter", "drowned", &[]);
    for _ in 0..6 {
        if store::get(&ctx.db, G, U).unwrap().unwrap().delve.is_none() {
            break;
        }
        click(&ctx, "dungeon", "next", "", &[]);
        play_fight(&ctx, "dungeon");
    }
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert_eq!(
        p.side.iter().find(|x| x.0 == "drills").map(|x| x.2),
        Some(1)
    );
    click(&ctx, "quests", "claim", "drills", &[]);
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert_eq!(
        p.side.iter().find(|x| x.0 == "drills").map(|x| x.1),
        Some(2)
    );

    // The market sells a lot of materials once a day, and supplies up to a limit.
    let gold = p.gold;
    click(&ctx, "market", "gear", "0", &[]);
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert!(p.gold < gold);
    let again = click(&ctx, "market", "gear", "0", &[]);
    assert!(text_of(&again).contains("sold"));
    for _ in 0..8 {
        click(&ctx, "market", "supply", "", &["thread"]);
    }
    let bought: i64 = ctx
        .db
        .with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM rpg_bought WHERE slot LIKE 'thread%'",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(bought, 2);

    // Switching a system off hides it.
    store::set_modules(
        G,
        store::Modules {
            labyrinth: false,
            guilds: false,
            ..Default::default()
        },
    );
    let s = click(&ctx, "hub", "", "", &[]);
    let rows = format!("{:?}", s.rows);
    assert!(!rows.contains("Labyrinth") && !rows.contains("\"Guild\""));
    store::set_modules(G, store::Modules::default());
}

#[test]
fn the_forge_is_where_gear_comes_from() {
    let rt = bot_core::tokio::runtime::Runtime::new().unwrap();
    let _e = rt.enter();
    let ctx = ctx();
    store::create(&ctx.db, G, U, "Antho", God::Thor).unwrap();
    // Closed at level 1: the Character screen says when it opens.
    let s = click(&ctx, "hero", "", "", &[]);
    assert!(
        text_of(&s).contains("The forge at level 3"),
        "{}",
        text_of(&s)
    );
    let s = click(&ctx, "forge", "gear", "beardaxe", &[]);
    assert!(text_of(&s).contains("opens at level"));
    level_to(&ctx, U, 6);
    store::run(&ctx.db, G, U, None, |g| {
        for (id, n) in [
            ("iron", 60),
            ("yew", 30),
            ("pelt", 30),
            ("frost", 6),
            ("fang", 4),
            ("thread", 4),
        ] {
            g.item(id, n, "test")?;
        }
        g.gold(5000, "test")
    })
    .unwrap();
    let s = click(&ctx, "forge", "", "", &[]);
    let t = text_of(&s);
    eprintln!("{t}");
    assert!(t.contains("Smith rank 1"), "{t}");
    assert!(t.contains("Temper as you forge"), "{t}");

    // A plain forge: worn at once, and the smith learns from it.
    click(&ctx, "forge", "gear", "beardaxe", &[]);
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert_eq!(p.equipped(data::Slot::Weapon).unwrap().gear.id, "beardaxe");
    assert!(p.smith_xp > 0);

    // Tempered: the chosen trait, at least Fine, and the catalyst spent.
    let sturdy = data::TRAITS
        .iter()
        .position(|t| *t == data::Trait::Sturdy)
        .unwrap();
    let frost = p.have("frost");
    click(&ctx, "forge", "temper", "", &[&format!("jerkin.{sturdy}")]);
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    let vest = p.equipped(data::Slot::Armour).unwrap();
    assert_eq!(vest.gear.id, "jerkin");
    assert!(vest.rarity >= data::Rarity::Fine);
    assert_eq!(vest.traits[0].0, data::Trait::Sturdy);
    assert_eq!(p.have("frost"), frost - data::temper_cost(vest.gear));
    // Not the wrong slot's trait.
    let fierce = data::TRAITS
        .iter()
        .position(|t| *t == data::Trait::Fierce)
        .unwrap();
    let n = p.gear.len();
    click(&ctx, "forge", "temper", "", &[&format!("jerkin.{fierce}")]);
    assert_eq!(store::get(&ctx.db, G, U).unwrap().unwrap().gear.len(), n);

    // Upgrades stop where the smith rank says.
    store::run(&ctx.db, G, U, None, |g| g.smith(-g.p.smith_xp).map(|_| ())).unwrap();
    let axe = p.equipped(data::Slot::Weapon).unwrap().id.to_string();
    click(&ctx, "forge", "upgrade", "", &[&axe]);
    let s = click(&ctx, "forge", "upgrade", "", &[&axe]);
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert_eq!(
        p.equipped(data::Slot::Weapon).unwrap().upgrade,
        1,
        "rank 1 stops at +1"
    );
    assert!(text_of(&s).contains("smith rank"), "{}", text_of(&s));

    // Found gear stops at +2 whatever the rank.
    store::run(&ctx.db, G, U, None, |g| {
        g.smith(100_000)?;
        let id = g.add_piece(
            data::gear("handaxe").unwrap(),
            data::Rarity::Fine,
            Vec::new(),
            true,
            "hunt",
        )?;
        g.upgrade(id)?;
        g.upgrade(id)
    })
    .unwrap();
    let found = store::get(&ctx.db, G, U)
        .unwrap()
        .unwrap()
        .equipped(data::Slot::Weapon)
        .unwrap()
        .clone();
    assert!(!found.forged);
    let s = click(&ctx, "forge", "upgrade", "", &[&found.id.to_string()]);
    assert!(text_of(&s).contains("Found gear"), "{}", text_of(&s));

    // Sigrun's order stays the same all day, whatever you level or learn meanwhile.
    let ask = |ctx: &Ctx| {
        store::run(&ctx.db, G, U, None, |g| {
            Ok(format!("{:?}", feat_rpg::orders::today(g)?.ask))
        })
        .unwrap()
    };
    let first = ask(&ctx);
    store::run(&ctx.db, G, U, None, |g| g.smith(2000).map(|_| ())).unwrap();
    level_to(&ctx, U, 8);
    assert_eq!(ask(&ctx), first);

    // Sigrun's order: hand it over once a day for gold and smithing.
    store::run(&ctx.db, G, U, None, |g| {
        match feat_rpg::orders::today(g)?.ask {
            feat_rpg::orders::Ask::Supply(id, n) => g.item(id, n, "test"),
            feat_rpg::orders::Ask::Piece(gear) => g
                .add_piece(gear, data::Rarity::Common, Vec::new(), false, "forged")
                .map(|_| ()),
        }
    })
    .unwrap();
    let before = store::get(&ctx.db, G, U).unwrap().unwrap();
    let s = click(&ctx, "forge", "order", "", &[]);
    assert!(text_of(&s).contains("Sigrun weighs it"), "{}", text_of(&s));
    let after = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert!(after.gold > before.gold && after.smith_xp > before.smith_xp);
    let s = click(&ctx, "forge", "order", "", &[]);
    assert!(text_of(&s).contains("Come back tomorrow"));
    assert_eq!(store::get(&ctx.db, G, U).unwrap().unwrap().gold, after.gold);

    // The market sells materials, never finished gear.
    level_to(&ctx, U, 9);
    let before = store::get(&ctx.db, G, U).unwrap().unwrap();
    click(&ctx, "market", "gear", "0", &[]);
    let after = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert_eq!(after.gear.len(), before.gear.len());
    assert!(after.gold < before.gold);
    let more: i64 = after.items.values().sum::<i64>() - before.items.values().sum::<i64>();
    assert!(more > 0);
}

#[test]
fn the_saga_opens_expeditions_and_says_so() {
    let rt = bot_core::tokio::runtime::Runtime::new().unwrap();
    let _e = rt.enter();
    let ctx = ctx();
    store::create(&ctx.db, G, U, "Antho", God::Thor).unwrap();
    // "Salt and Song" done (three wins on the shore): claiming it moves to "Iron and Bread",
    // where the widow sends you out, so expeditions open before level 2.
    store::run(&ctx.db, G, U, None, |g| {
        g.p.saga = 1;
        g.p.saga_progress = vec![3];
        g.save_saga()
    })
    .unwrap();
    let s = click(&ctx, "saga", "claim", "", &[]);
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert_eq!(p.saga, 2);
    assert!(
        text_of(&s).contains("Now open:** Expeditions"),
        "{}",
        text_of(&s)
    );
    assert!(feat_rpg::unlocks::open(&p, data::Feature::Expedition));
    assert!(!feat_rpg::unlocks::open(&p, data::Feature::Forge));
}
