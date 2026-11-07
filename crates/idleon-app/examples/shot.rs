//! Draws the app without a window and saves it as a PNG, and says how long each step took:
//! `cargo run --release -p idleon-app --example shot -- <save.json | test:7> <out.png> [width] [height] [what]`.
//!
//! `what`: `top` (the default), `open:<mechanic key>` (that card open, its tab chosen),
//! `all:<key>` (open with Show all), `ask` (the paste box), `packs:free`.

use std::time::Instant;

use iced_test::simulator::Simulator;
use idleon_app::app::{App, Msg, ready_now};
use idleon_core::review::Packs;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (src, out) = (&args[1], &args[2]);
    let width: f32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1400.0);
    let height: f32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(920.0);
    let what = args.get(5).map(String::as_str).unwrap_or("top");
    let packs = what
        .strip_prefix("packs:")
        .map(Packs::from_code)
        .unwrap_or(Packs::Buy);
    let t0 = Instant::now();
    let text =
        (!src.starts_with("test:")).then(|| std::fs::read_to_string(src).expect("the save reads"));
    let test = src.strip_prefix("test:").and_then(|w| w.parse().ok());
    let ready = ready_now(text.as_deref(), packs, test).expect("the save reads");
    eprintln!(
        "read and reviewed: {:.1} ms",
        t0.elapsed().as_secs_f64() * 1000.0
    );
    let mut app = if what == "ask" {
        App::boot_quiet().0
    } else {
        App::showing(ready, width)
    };
    app.width = width;
    if let Some(key) = what
        .strip_prefix("open:")
        .or_else(|| what.strip_prefix("all:"))
    {
        let i = app
            .done_mut()
            .expect("a review")
            .ready
            .review
            .mechanics
            .iter()
            .position(|m| m.key == key)
            .expect("that mechanic");
        let _ = app.update(Msg::Go(i));
        if what.starts_with("all:") {
            let _ = app.update(Msg::All(i));
        }
    }
    let settings = iced::Settings {
        fonts: idleon_app::style::FONTS
            .iter()
            .map(|f| std::borrow::Cow::Borrowed(*f))
            .collect(),
        default_font: idleon_app::style::SANS,
        ..Default::default()
    };
    // Building the view: the work every message pays for.
    let mut best = f64::MAX;
    for _ in 0..5 {
        let t = Instant::now();
        let v = app.view();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        best = best.min(ms);
        drop(v);
    }
    eprintln!("view built: {best:.2} ms (best of 5)");
    let t = Instant::now();
    let mut ui = Simulator::with_size(settings, (width, height), app.view());
    eprintln!("first layout: {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
    // Scrolled to the card, as a click on its line would.
    if let Some(key) = what.strip_prefix("open:").or_else(|| what.strip_prefix("all:"))
        && let (Ok(card), Ok(page)) = (ui.find(iced::widget::selector::id(format!("m-{key}"))), ui.find(iced::widget::selector::id(idleon_app::app::PAGE)))
    {
        eprintln!("card at y {:.0} in the page", card.bounds().y - page.bounds().y);
    }
    if std::env::var("SHOT_DEBUG").is_ok() {
        for key in ["m-gallery", "m-stamps", "m-spelunking"] {
            eprintln!(
                "{key}: {:?}",
                ui.find(iced::widget::selector::id(key)).map(|t| t.bounds())
            );
        }
        for txt in ["Late World 6", "Should have by now", "Report a problem"] {
            eprintln!("{txt}: {:?}", ui.find(txt).map(|t| t.bounds()));
        }
    }
    let t = Instant::now();
    let shot = ui
        .snapshot(&idleon_app::style::theme())
        .expect("a snapshot");
    eprintln!("drawn: {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
    // The snapshot names its file `<out>-<renderer>.png` and only writes one that isn't there.
    let stem = out.trim_end_matches(".png");
    for r in ["tiny-skia", "wgpu"] {
        let _ = std::fs::remove_file(format!("{stem}-{r}.png"));
    }
    shot.matches_image(stem).expect("the PNG writes");
}
