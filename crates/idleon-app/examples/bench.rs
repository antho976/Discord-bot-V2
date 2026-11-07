//! Times what a person waits on in the app, without a window:
//! `cargo run --release -p idleon-app --example bench -- save.json [runs]`.
//!
//! - Reading a save: parse, checks, review, tiers, Should have by now (all off the window's
//!   thread in the app, but they decide when the review shows).
//! - Putting the server's tiers in place at start (on the window's thread, before the first frame).
//! - Each click: building the view and laying it out (on the window's thread, every message),
//!   with the layout cache kept between them as the app keeps it. Drawing isn't counted: the
//!   app draws on the GPU, this has none.

use std::time::Instant;

use iced_test::core::renderer::Headless;
use iced_test::runtime::UserInterface;
use iced_test::runtime::user_interface::Cache;
use idleon_app::app::{App, Msg, ready_now};
use idleon_core::review::{Packs, Review};
use idleon_core::save::Save;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// p50 and p95 of a run's samples.
fn spread(mut v: Vec<f64>) -> String {
    v.sort_by(f64::total_cmp);
    let at = |q: f64| v[((v.len() as f64 - 1.0) * q).round() as usize];
    format!(
        "p50 {:>7.2} ms  p95 {:>7.2} ms  max {:>7.2} ms  (n={})",
        at(0.5),
        at(0.95),
        v[v.len() - 1],
        v.len()
    )
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let text = std::fs::read_to_string(&args[1]).expect("the save reads");
    let runs: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(30);

    // The first read also loads the game's data (once per run of the app).
    let t = Instant::now();
    let _ = &*idleon_core::game::GAME;
    println!("game data      {:.2} ms (once, at the first read)", ms(t));
    let t = Instant::now();
    let _ = ready_now(Some(&text), Packs::Buy, None);
    println!("first read     {:.2} ms (cold)", ms(t));
    let (mut parse, mut checks, mut review, mut tiers, mut owing, mut whole) =
        (vec![], vec![], vec![], vec![], vec![], vec![]);
    for _ in 0..runs {
        let t = Instant::now();
        let s = Save::parse(&text).expect("parses");
        parse.push(ms(t));
        let t = Instant::now();
        let ms_ = idleon_core::run_checks(&s);
        checks.push(ms(t));
        let t = Instant::now();
        let r = Review::with_packs(&s, ms_, idleon_core::world_reached(&s), Packs::Buy);
        review.push(ms(t));
        drop(s);
        let t = Instant::now();
        let _ts: Vec<_> = r.mechanics.iter().map(|m| m.tiers()).collect();
        tiers.push(ms(t));
        let t = Instant::now();
        let _o = idleon_core::view::owing(&r);
        owing.push(ms(t));
        let t = Instant::now();
        let _ = ready_now(Some(&text), Packs::Buy, None);
        whole.push(ms(t));
    }
    println!("save: {} KB", text.len() / 1024);
    println!("parse          {}", spread(parse));
    println!("checks         {}", spread(checks));
    println!("review         {}", spread(review));
    println!("tiers          {}", spread(tiers));
    println!("should have    {}", spread(owing));
    println!("whole read     {}", spread(whole));

    let crowd = idleon_core::crowd::source().into_owned();
    let expected = idleon_core::expected::source().into_owned();
    let mut set = vec![];
    for _ in 0..runs {
        let t = Instant::now();
        idleon_core::crowd::set(Some(&crowd));
        idleon_core::expected::set(Some(&expected));
        set.push(ms(t));
    }
    idleon_core::crowd::set(None);
    idleon_core::expected::set(None);
    println!("tiers at start {}", spread(set));

    for f in idleon_app::style::FONTS {
        iced_test::renderer::graphics::text::font_system()
            .write()
            .expect("fonts")
            .load_font(std::borrow::Cow::Borrowed(f));
    }
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <iced::Renderer as Headless>::new(idleon_app::style::SANS, 16.into(), Some("tiny-skia")),
    )
    .expect("a renderer");
    let size = iced::Size::new(1400.0, 920.0);
    let ready = ready_now(Some(&text), Packs::Buy, None).expect("reads");
    let mut app = App::showing(ready, size.width);
    let stamps = app
        .done_mut()
        .expect("a review")
        .ready
        .review
        .mechanics
        .iter()
        .position(|m| m.key == "stamps")
        .expect("stamps");
    let mut cache = Cache::default();
    let mut frame = |app: &App, cache: Cache| -> (f64, f64, Cache) {
        let t = Instant::now();
        let v = app.view();
        let view = ms(t);
        let t = Instant::now();
        let ui = UserInterface::build(v, size, cache, &mut renderer);
        let layout = ms(t);
        (view, layout, ui.into_cache())
    };
    let (v, l, c) = frame(&app, cache);
    cache = c;
    println!("first frame    view {v:.2} ms  layout {l:.2} ms");
    // Each step a person takes, done `runs` times: (view, layout) per step.
    type Step = Box<dyn Fn(&mut App)>;
    let steps: Vec<(&str, Step)> = vec![
        (
            "open a card",
            Box::new(move |a: &mut App| {
                let _ = a.update(Msg::Toggle(stamps));
            }),
        ),
        (
            "switch world",
            Box::new(|a: &mut App| {
                let d = a.done_mut().expect("a review");
                let to = if d.tab == 1 { 7 } else { 1 };
                let _ = a.update(Msg::Tab(to));
            }),
        ),
        (
            "go to a card",
            Box::new(move |a: &mut App| {
                let _ = a.update(Msg::Go(stamps));
            }),
        ),
        (
            "show all",
            Box::new(move |a: &mut App| {
                let _ = a.update(Msg::All(stamps));
            }),
        ),
    ];
    for (name, step) in steps {
        let (mut vs, mut ls) = (vec![], vec![]);
        for _ in 0..runs {
            step(&mut app);
            let (v, l, c) = frame(&app, cache);
            cache = c;
            vs.push(v);
            ls.push(l);
        }
        println!("{name:<14} view   {}", spread(vs));
        println!("{:<14} layout {}", "", spread(ls));
    }
    // Peak memory of this run (Linux): the reads, the game data, a software renderer; the app
    // draws on the GPU instead.
    if let Ok(st) = std::fs::read_to_string("/proc/self/status") {
        for l in st.lines().filter(|l| l.starts_with("VmHWM") || l.starts_with("VmRSS")) {
            println!("{l}");
        }
    }
}
