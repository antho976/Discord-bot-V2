//! The Idleon account check as a desktop app: the web page's check, read on your own computer.
//! The reading is `idleon-core`'s, the same as the page's; this crate is the window around it
//! and what it syncs with the bot's server (`sync`).

pub mod app;
pub mod pics;
pub mod style;
pub mod sync;
mod ui;

pub use app::App;

/// Opens the window.
pub fn run() -> iced::Result {
    let mut a = iced::application(App::boot, App::update, App::view)
        .title(App::title)
        .subscription(App::subscription)
        .theme(|_: &App| style::theme())
        .default_font(style::SANS)
        .window(iced::window::Settings {
            size: iced::Size::new(1400.0, 920.0),
            min_size: Some(iced::Size::new(480.0, 400.0)),
            ..Default::default()
        })
        .antialiasing(true);
    for f in style::FONTS {
        a = a.font(f);
    }
    a.run()
}
