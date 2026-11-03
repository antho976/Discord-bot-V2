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
                assert!(x.components.len() <= 5, "a row has {} items", x.components.len());
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
                assert!(m.custom_id.len() <= 100, "custom id too long: {}", m.custom_id);
                let opts = m.options.as_ref().unwrap();
                assert!(!opts.is_empty() && opts.len() <= 25);
                let values: HashSet<&str> = opts.iter().map(|o| o.value.as_str()).collect();
                assert_eq!(values.len(), opts.len(), "duplicate option values in {}", m.custom_id);
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

fn click_at(ctx: &Ctx, screen: &str, action: &str, arg: &str, values: &[&str], version: i64) -> Screen {
    let route = Route { owner: U, screen: screen.into(), action: action.into(), arg: arg.into(), version };
    let values: Vec<String> = values.iter().map(|v| v.to_string()).collect();
    let out = store::run(&ctx.db, G, U, Some("Antho"), |g| game::handle(ctx, g, &route, &values, Rates::default())).unwrap().unwrap();
    check(&out.screen);
    out.screen
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
    for s in ["hub", "goals", "exp", "hunt", "boss", "forge", "pack", "temple", "givethor", "giveathena", "givehades", "patron", "help"] {
        click(&ctx, s, "", "", &[]);
    }

    // Plan with the menus, then send out.
    let s = click(&ctx, "exp", "set", "region|shore.normal.4.rations", &["wolfwood"]);
    assert!(text_of(&s).contains("Forecast"));
    let before = store::get(&ctx.db, G, U).unwrap().unwrap();
    click(&ctx, "exp", "go", "shore.normal.1.rations", &[]);
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert!(p.expedition.is_some());
    assert_eq!(p.have("rations"), before.have("rations") - 1);

    // The same button again, from before the send: refused, nothing doubles.
    let s = click_at(&ctx, "exp", "go", "shore.normal.1.rations", &[], before.version);
    assert!(text_of(&s).contains("before your last move"));

    // Not back yet: collect does nothing.
    click(&ctx, "exp", "collect", "", &[]);
    assert!(store::get(&ctx.db, G, U).unwrap().unwrap().expedition.is_some());

    // Back: collect pays out once.
    ctx.db.with(|c| c.execute("UPDATE rpg_expeditions SET ends_at = 0", [])).unwrap();
    let gold_before = store::get(&ctx.db, G, U).unwrap().unwrap().gold;
    let s = click(&ctx, "exp", "collect", "", &[]);
    assert!(text_of(&s).contains("Brought back"));
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert!(p.expedition.is_none());
    assert!(p.gold > gold_before);
    assert!(p.tribute[God::Thor as usize] > 0, "expedition tribute goes to the patron's altar");
    let gold_after = p.gold;
    click(&ctx, "exp", "collect", "", &[]);
    assert_eq!(store::get(&ctx.db, G, U).unwrap().unwrap().gold, gold_after);

    // Hunt until the fight ends.
    click(&ctx, "hunt", "start", "shore", &[]);
    for _ in 0..80 {
        let p = store::get(&ctx.db, G, U).unwrap().unwrap();
        let Some(f) = p.fight else { break };
        let a = if matches!(f.intent(), data::Move::Heavy | data::Move::Roar) || f.stamina < 1 { "guard" } else { "strike" };
        click(&ctx, "hunt", "act", a, &[]);
    }
    assert!(store::get(&ctx.db, G, U).unwrap().unwrap().fight.is_none(), "the hunt never ended");

    // Forge supplies from materials.
    grant(&ctx, &[("herbs", 40), ("fish", 30), ("iron", 50), ("yew", 30), ("amber", 4)], 2000);
    let potions = store::get(&ctx.db, G, U).unwrap().unwrap().have("potion");
    click(&ctx, "forge", "make", "", &["potion"]);
    assert_eq!(store::get(&ctx.db, G, U).unwrap().unwrap().have("potion"), potions + 2);
    // A piece of gear and an upgrade.
    click(&ctx, "forge", "gear", "ambercharm", &[]);
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    let charm = p.equipped(data::Slot::Charm).expect("the charm is worn").id;
    click(&ctx, "forge", "upgrade", "", &[&charm.to_string()]);
    assert_eq!(store::get(&ctx.db, G, U).unwrap().unwrap().equipped(data::Slot::Charm).unwrap().upgrade, 1);

    // Give spare materials and gold at Athena's altar.
    click(&ctx, "temple", "give", "athena.spare", &[]);
    click(&ctx, "temple", "give", "athena.gold100", &[]);
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert!(p.tribute[God::Athena as usize] > 100);
    let o = ctx.db.with(|c| Ok(store::offerings(c, G, store::week_now()).unwrap())).unwrap();
    assert_eq!(o[God::Athena as usize], p.tribute[God::Athena as usize]);
    assert!(p.items.iter().filter(|(id, _)| data::item(id).is_some_and(|i| i.kind == data::Kind::Material)).all(|(_, n)| *n <= 20));

    // Face the boss far too early: it's a real fight that can be lost or fled.
    store::run(&ctx.db, G, U, None, |g| g.set_hp(9999)).unwrap();
    click(&ctx, "boss", "start", "", &[]);
    assert!(store::get(&ctx.db, G, U).unwrap().unwrap().fight.is_some());
    click(&ctx, "boss", "act", "strike", &[]);
    click(&ctx, "boss", "leave", "", &[]);
    assert!(store::get(&ctx.db, G, U).unwrap().unwrap().fight.is_none());

    // The ledger saw every movement and nothing went below zero.
    let neg: i64 = ctx.db.with(|c| c.query_row("SELECT COUNT(*) FROM rpg_items WHERE qty < 0", [], |r| r.get(0))).unwrap();
    assert_eq!(neg, 0);
    let ledger: i64 = ctx.db.with(|c| c.query_row("SELECT COUNT(*) FROM rpg_ledger WHERE guild_id = ?1", params![G as i64], |r| r.get(0))).unwrap();
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
            let a = feat_rpg::engine::combat::autoplay(f, p.have("potion"), (p.have("fireoil"), p.have("frostoil")));
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
    assert!(p.boss_won, "a full-power Athena never beat the boss in six tries");
    assert_eq!(p.have("shard"), 3);
    // Heroic is offered now, and a lesser vow can be taken.
    let s = click(&ctx, "exp", "set", "danger|barrow.normal.4.none", &["heroic"]);
    assert!(text_of(&s).contains("Heroic"));
    click(&ctx, "patron", "vow", "", &["thor"]);
    let p = store::get(&ctx.db, G, U).unwrap().unwrap();
    assert_eq!(p.lesser, Some(God::Thor));
    let _ = data::GODS;
}
