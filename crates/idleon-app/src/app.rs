//! The app's state and what each message does. Reading a save, the network and the disk all run
//! off the window's thread (`background`); the window only ever draws what's ready.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use iced::widget::{image, operation, scrollable, selector, text_editor};
use iced::{Size, Task, keyboard, window};

use idleon_core::review::{Packs, Review, Tier};
use idleon_core::save::Save;
use idleon_core::view::{self, Owing};

use crate::sync;

/// The page's scrollable, so a Should have line can scroll to its card.
pub const PAGE: &str = "page";

/// A value handed from a background thread through a message (messages must be `Clone`; this
/// one is taken once).
pub struct Slot<T>(Arc<Mutex<Option<T>>>);

impl<T> Clone for Slot<T> {
    fn clone(&self) -> Self {
        Slot(self.0.clone())
    }
}

impl<T> Slot<T> {
    fn new(v: T) -> Self {
        Slot(Arc::new(Mutex::new(Some(v))))
    }
    fn take(&self) -> Option<T> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}

impl<T> std::fmt::Debug for Slot<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Slot")
    }
}

/// Runs `f` on its own thread and answers when it's done.
fn background<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> impl std::future::Future<Output = T> + Send {
    let (tx, rx) = iced::futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    async move { rx.await.expect("the background thread answers") }
}

/// A review and everything the screen draws from it that's worth working out once: each
/// mechanic's tiers, and Should have by now.
pub struct Ready {
    pub review: Review,
    pub tiers: Vec<Vec<Tier>>,
    pub owing: Option<Owing>,
    /// The save it came from, to check again with other packs or new tiers (`None` for a test
    /// account).
    pub text: Option<Arc<str>>,
    /// How long reading it took, for the trace.
    pub took_ms: f64,
}

fn prepare(text: Option<Arc<str>>, packs: Packs, test_world: Option<u8>) -> Result<Ready, String> {
    let t0 = Instant::now();
    let save = match &text {
        Some(t) => Save::parse(t).map_err(|e| e.message().to_string())?,
        None => Save::parse(r#"{"OptLacc": []}"#).expect("an empty save reads"),
    };
    let world = test_world.unwrap_or_else(|| idleon_core::world_reached(&save));
    let mut review = Review::with_packs(&save, idleon_core::run_checks(&save), world, packs);
    drop(save);
    review.test = test_world.is_some();
    let tiers = review.mechanics.iter().map(|m| m.tiers()).collect();
    let owing = view::owing(&review);
    Ok(Ready {
        review,
        tiers,
        owing,
        text,
        took_ms: t0.elapsed().as_secs_f64() * 1000.0,
    })
}

pub enum Screen {
    Ask,
    Done(Box<Done>),
}

pub struct Done {
    pub ready: Ready,
    /// The world tab showing.
    pub tab: u8,
    /// Cards open, by mechanic index.
    pub open: BTreeSet<usize>,
    /// Cards whose Show all is open.
    pub all: BTreeSet<usize>,
    pub fresh_more: bool,
}

pub struct Report {
    pub card: Option<String>,
    pub text: text_editor::Content,
    pub said: Option<String>,
    pub sending: bool,
}

pub struct App {
    pub screen: Screen,
    pub settings: sync::Settings,
    pub packs: Packs,
    /// What's being read, while it is.
    pub busy: Option<&'static str>,
    pub error: Option<String>,
    /// The last load asked for: an older one that answers late is dropped.
    load: u64,
    pub width: f32,
    pub server: String,
    pub info: Option<sync::Info>,
    pub icon: Option<image::Handle>,
    /// Where the tiers come from, for the footer.
    pub tiers_from: &'static str,
    pub report: Report,
    /// The sign-in code the browser should show, while the app waits for it.
    pub linking: Option<String>,
    /// Test accounts on the paste box (a debug build, or `IDLEON_CHECK_TEST=1`).
    pub test_links: bool,
    pub trace: Option<Trace>,
}

/// `IDLEON_CHECK_TRACE=1`: how long each step a person waits on takes, on stderr.
pub struct Trace {
    pub start: Instant,
    pub view_ms: std::cell::Cell<f64>,
    pub views: std::cell::Cell<u32>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    OpenFile,
    Picked(Option<PathBuf>),
    Paste,
    Pasted(Option<String>),
    Dropped(PathBuf),
    Read(Result<String, String>),
    Loaded(u64, Slot<Result<Ready, String>>),
    Packs(Packs),
    Test(u8),
    Again,
    Tab(u8),
    Toggle(usize),
    All(usize),
    FreshMore,
    /// A Should have line or a Not started chip: its card, opened and scrolled to.
    Go(usize),
    ScrollTo(Option<f32>),
    Resized(Size),
    Synced(Slot<SyncDone>),
    Icon(Option<Slot<Vec<u8>>>),
    ReportCard(String),
    ReportEdit(text_editor::Action),
    ReportSend,
    ReportSent(Result<String, (String, bool)>),
    ReportCopy,
    SignIn,
    LinkCode(Result<sync::Link, String>),
    Linked(Result<sync::Session, String>),
    SignOut,
    Guild(String),
    Key(keyboard::Key, keyboard::Modifiers),
    Url(String),
    Frame,
}

/// What a sync brought back.
pub struct SyncDone {
    pub info: Option<sync::Info>,
    /// New tiers were put in place (a review on screen should be read again).
    pub new_tiers: bool,
    pub from_server: bool,
}

pub const CARD_TEXT: &str = "Anything";

impl App {
    pub fn boot() -> (App, Task<Msg>) {
        let trace = std::env::var("IDLEON_CHECK_TRACE")
            .is_ok_and(|v| v == "1")
            .then(|| Trace {
                start: Instant::now(),
                view_ms: 0.0.into(),
                views: 0.into(),
            });
        let settings = sync::settings();
        let packs = Packs::from_code(&settings.packs);
        // The server's tiers as last fetched go in before anything is read with them.
        let kept = sync::kept_tiers();
        let tiers_from = if kept.as_ref().is_some_and(sync::use_tiers) {
            "the server, as last synced"
        } else {
            "the app itself"
        };
        let mut app = App {
            screen: Screen::Ask,
            settings,
            packs,
            busy: None,
            error: None,
            load: 0,
            width: 1400.0,
            server: sync::server(),
            info: None,
            icon: None,
            tiers_from,
            report: Report {
                card: None,
                text: text_editor::Content::new(),
                said: None,
                sending: false,
            },
            linking: None,
            test_links: cfg!(debug_assertions)
                || std::env::var("IDLEON_CHECK_TEST").is_ok_and(|v| v == "1"),
            trace,
        };
        let mut tasks = vec![app.sync(kept.map(|t| t.version))];
        // The save last checked, checked again: the app opens on your account.
        if let Some(text) = sync::last_save() {
            tasks.push(app.read(Some(text.into()), None, "Reading your last save…"));
        }
        (app, Task::batch(tasks))
    }

    fn trace(&self, what: &str) {
        if let Some(t) = &self.trace {
            eprintln!(
                "[trace] {:>8.1} ms  {what}",
                t.start.elapsed().as_secs_f64() * 1000.0
            );
        }
    }

    /// Asks the server what's new: its servers list, and its tiers when they changed.
    fn sync(&self, have: Option<String>) -> Task<Msg> {
        if sync::offline() {
            return Task::none();
        }
        let server = self.server.clone();
        Task::perform(
            background(move || {
                let info = sync::info(&server).ok();
                let mut new_tiers = false;
                if info
                    .as_ref()
                    .is_none_or(|i| have.as_deref() != Some(i.tiers.as_str()))
                    && let Ok(Some(t)) = sync::tiers(&server, have.as_deref())
                    && sync::use_tiers(&t)
                {
                    sync::keep_tiers(&t);
                    new_tiers = true;
                }
                let from_server = info.is_some();
                SyncDone {
                    info,
                    new_tiers,
                    from_server,
                }
            }),
            |d| Msg::Synced(Slot::new(d)),
        )
    }

    /// Reads a save (or a test account, `text` None) off the window's thread.
    fn read(&mut self, text: Option<Arc<str>>, test: Option<u8>, busy: &'static str) -> Task<Msg> {
        self.load += 1;
        self.busy = Some(busy);
        self.error = None;
        let (n, packs) = (self.load, self.packs);
        Task::perform(background(move || prepare(text, packs, test)), move |r| {
            Msg::Loaded(n, Slot::new(r))
        })
    }

    fn done(&mut self) -> Option<&mut Done> {
        match &mut self.screen {
            Screen::Done(d) => Some(d),
            Screen::Ask => None,
        }
    }

    pub fn guild(&self) -> Option<&sync::Guild> {
        let gs = &self.info.as_ref()?.guilds;
        self.settings
            .guild
            .as_ref()
            .and_then(|g| gs.iter().find(|x| &x.id == g))
            .or_else(|| gs.first())
    }

    pub fn update(&mut self, msg: Msg) -> Task<Msg> {
        match msg {
            Msg::OpenFile => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .set_title("Open your Idleon save")
                        .add_filter("Idleon save", &["json", "txt"])
                        .pick_file()
                        .await
                        .map(|f| f.path().to_path_buf())
                },
                Msg::Picked,
            ),
            Msg::Picked(None) => Task::none(),
            Msg::Picked(Some(p)) | Msg::Dropped(p) => {
                self.busy = Some("Opening the file…");
                Task::perform(
                    background(move || {
                        let meta = std::fs::metadata(&p)
                            .map_err(|_| "Couldn't open that file.".to_string())?;
                        if meta.len() as usize > idleon_core::save::MAX_BYTES {
                            return Err("That file is bigger than any Idleon save. Pick the JSON you saved from IdleonToolbox or Idleon Efficiency.".into());
                        }
                        std::fs::read_to_string(&p).map_err(|_| "That file isn't text. Pick the JSON you saved from IdleonToolbox or Idleon Efficiency.".into())
                    }),
                    Msg::Read,
                )
            }
            Msg::Paste => iced::clipboard::read().map(Msg::Pasted),
            Msg::Pasted(s) => match s {
                Some(s) if s.trim_start().starts_with('{') => self.update(Msg::Read(Ok(s))),
                _ => {
                    self.error = Some("Your clipboard doesn't hold a save. Copy your account's JSON first, then try again.".into());
                    Task::none()
                }
            },
            Msg::Read(Ok(s)) => {
                if s.len() > idleon_core::save::MAX_BYTES {
                    self.busy = None;
                    self.error = Some(idleon_core::save::SaveError::TooBig.message().into());
                    return Task::none();
                }
                self.read(Some(s.into()), None, "Reading your save…")
            }
            Msg::Read(Err(e)) => {
                self.busy = None;
                self.error = Some(e);
                Task::none()
            }
            Msg::Loaded(n, slot) => {
                if n != self.load {
                    return Task::none();
                }
                self.busy = None;
                match slot.take() {
                    Some(Ok(ready)) => {
                        self.trace(&format!("review ready (read in {:.1} ms)", ready.took_ms));
                        if let Some(t) = &ready.text {
                            let t = t.clone();
                            std::thread::spawn(move || sync::keep_save(&t));
                        }
                        // The same account again (new packs, new tiers): stay where you were.
                        let (tab, open, all, fresh_more) = match self.done() {
                            Some(d)
                                if d.ready.review.name == ready.review.name
                                    && d.ready.review.test == ready.review.test
                                    && d.ready.review.world == ready.review.world =>
                            {
                                (
                                    d.tab,
                                    std::mem::take(&mut d.open),
                                    std::mem::take(&mut d.all),
                                    d.fresh_more,
                                )
                            }
                            _ => (
                                view::worlds(&ready.review).1,
                                BTreeSet::new(),
                                BTreeSet::new(),
                                false,
                            ),
                        };
                        self.screen = Screen::Done(Box::new(Done {
                            ready,
                            tab,
                            open,
                            all,
                            fresh_more,
                        }));
                        Task::none()
                    }
                    Some(Err(e)) => {
                        self.error = Some(e);
                        self.screen = Screen::Ask;
                        sync::forget_save();
                        Task::none()
                    }
                    None => Task::none(),
                }
            }
            Msg::Packs(p) => {
                self.packs = p;
                self.settings.packs = p.code().to_string();
                let s = self.settings.clone();
                std::thread::spawn(move || sync::save_settings(&s));
                self.reread("Checking it again…")
            }
            Msg::Test(w) => self.read(None, Some(w), "Making a test account…"),
            Msg::Again => {
                self.screen = Screen::Ask;
                self.error = None;
                std::thread::spawn(sync::forget_save);
                operation::snap_to(PAGE, scrollable::RelativeOffset::START)
            }
            Msg::Tab(w) => {
                if let Some(d) = self.done() {
                    d.tab = w;
                }
                Task::none()
            }
            Msg::Toggle(i) => {
                if let Some(d) = self.done()
                    && !d.open.remove(&i)
                {
                    d.open.insert(i);
                }
                Task::none()
            }
            Msg::All(i) => {
                if let Some(d) = self.done()
                    && !d.all.remove(&i)
                {
                    d.all.insert(i);
                }
                Task::none()
            }
            Msg::FreshMore => {
                if let Some(d) = self.done() {
                    d.fresh_more = true;
                }
                Task::none()
            }
            Msg::Go(i) => {
                let Some(d) = self.done() else {
                    return Task::none();
                };
                d.tab = d.ready.review.mechanics[i].world;
                d.open.insert(i);
                let card = card_id(d.ready.review.mechanics[i].key);
                // Once drawn: the card's place in the page, less the page's own top.
                selector::find(selector::id(card))
                    .then(|card| {
                        selector::find(selector::id(PAGE)).map(move |page| match (&card, page) {
                            (Some(c), Some(p)) => Some(c.bounds().y - p.bounds().y - 20.0),
                            _ => None,
                        })
                    })
                    .map(Msg::ScrollTo)
            }
            Msg::ScrollTo(Some(y)) => operation::scroll_to(
                PAGE,
                scrollable::AbsoluteOffset {
                    x: 0.0,
                    y: y.max(0.0),
                },
            ),
            Msg::ScrollTo(None) => Task::none(),
            Msg::Resized(s) => {
                self.width = s.width;
                Task::none()
            }
            Msg::Synced(slot) => {
                let Some(d) = slot.take() else {
                    return Task::none();
                };
                self.trace("sync answered");
                // New tiers, or the server saying the kept ones are still its own.
                if d.new_tiers || (d.from_server && self.tiers_from == "the server, as last synced") {
                    self.tiers_from = "the server";
                }
                let icon = d.info.as_ref().and_then(|i| {
                    let g = self
                        .settings
                        .guild
                        .as_ref()
                        .and_then(|g| i.guilds.iter().find(|x| &x.id == g))
                        .or_else(|| i.guilds.first())?;
                    g.icon.clone()
                });
                self.info = d.info.or(self.info.take());
                let mut tasks = Vec::new();
                if let Some(url) = icon {
                    // The preview names its icon by path; Discord's are whole addresses.
                    let url = if url.starts_with('/') {
                        format!("{}{url}", self.server)
                    } else {
                        url
                    };
                    tasks.push(Task::perform(background(move || sync::icon(&url)), |b| {
                        Msg::Icon(b.map(Slot::new))
                    }));
                }
                if d.new_tiers {
                    tasks.push(self.reread("New tiers from the server, checking again…"));
                }
                Task::batch(tasks)
            }
            Msg::Icon(b) => {
                self.icon = b.and_then(|s| s.take()).map(crate::pics::bytes);
                Task::none()
            }
            Msg::Guild(id) => {
                self.settings.guild = Some(id);
                let s = self.settings.clone();
                std::thread::spawn(move || sync::save_settings(&s));
                Task::none()
            }
            Msg::ReportCard(c) => {
                self.report.card = (c != CARD_TEXT).then_some(c);
                Task::none()
            }
            Msg::ReportEdit(a) => {
                self.report.text.perform(a);
                // The suggestion box's limit, as the page's box has it.
                let t = self.report.text.text();
                if t.chars().count() > view::REPORT_TEXT {
                    let cut: String = t.chars().take(view::REPORT_TEXT).collect();
                    self.report.text = text_editor::Content::with_text(&cut);
                }
                Task::none()
            }
            Msg::ReportCopy => {
                let Some(d) = self.done() else {
                    return Task::none();
                };
                let name = d.ready.review.name.clone();
                let text = view::report_text(
                    &name,
                    self.report.card.as_deref().unwrap_or(""),
                    &self.report.text.text(),
                );
                self.report.said =
                    Some("Copied. Now run /suggest in the server and paste it.".into());
                iced::clipboard::write(text)
            }
            Msg::ReportSend => {
                let (Some(s), Some(g)) = (
                    self.settings.session.clone(),
                    self.guild().map(|g| g.id.clone()),
                ) else {
                    return Task::none();
                };
                let text = self.report.text.text();
                if text.trim().is_empty() {
                    self.report.said = Some("Write what looks wrong first.".into());
                    return Task::none();
                }
                let Some(d) = self.done() else {
                    return Task::none();
                };
                let account = d.ready.review.name.clone();
                let card = self.report.card.clone().unwrap_or_default();
                self.report.sending = true;
                self.report.said = Some("Sending…".into());
                let server = self.server.clone();
                Task::perform(
                    background(move || {
                        sync::report(&server, &g, &s.token, &account, &card, text.trim())
                    }),
                    Msg::ReportSent,
                )
            }
            Msg::ReportSent(r) => {
                self.report.sending = false;
                match r {
                    Ok(said) => {
                        self.report.said = Some(said);
                        self.report.text = text_editor::Content::new();
                    }
                    Err((said, gone)) => {
                        self.report.said = Some(if said.is_empty() {
                            "Couldn't send it. Try again in a minute.".into()
                        } else {
                            said
                        });
                        if gone {
                            self.settings.session = None;
                            let s = self.settings.clone();
                            std::thread::spawn(move || sync::save_settings(&s));
                        }
                    }
                }
                Task::none()
            }
            Msg::SignIn => {
                let server = self.server.clone();
                self.report.said = None;
                Task::perform(background(move || sync::link_start(&server)), Msg::LinkCode)
            }
            Msg::LinkCode(Ok(link)) => {
                let Some(g) = self.guild().map(|g| g.id.clone()) else {
                    return Task::none();
                };
                let url = format!("{}/p/{g}/idleon/link?code={}", self.server, link.code);
                self.linking = Some(format!("{}-{}", &link.code[..4], &link.code[4..]));
                let _ = open::that_detached(&url);
                let server = self.server.clone();
                Task::perform(
                    background(move || sync::link_wait(&server, &link)),
                    Msg::Linked,
                )
            }
            Msg::LinkCode(Err(e)) | Msg::Linked(Err(e)) => {
                self.linking = None;
                self.report.said = Some(e);
                Task::none()
            }
            Msg::Linked(Ok(s)) => {
                self.linking = None;
                self.settings.session = Some(s);
                let st = self.settings.clone();
                std::thread::spawn(move || sync::save_settings(&st));
                Task::none()
            }
            Msg::SignOut => {
                if let (Some(s), Some(g)) = (
                    self.settings.session.take(),
                    self.guild().map(|g| g.id.clone()),
                ) {
                    let server = self.server.clone();
                    std::thread::spawn(move || sync::sign_out(&server, &g, &s.token));
                }
                let st = self.settings.clone();
                std::thread::spawn(move || sync::save_settings(&st));
                Task::none()
            }
            Msg::Key(key, m) => {
                use keyboard::key::Named;
                match key.as_ref() {
                    keyboard::Key::Character("v")
                        if m.command() && matches!(self.screen, Screen::Ask) =>
                    {
                        self.update(Msg::Paste)
                    }
                    keyboard::Key::Character("o") if m.command() => self.update(Msg::OpenFile),
                    keyboard::Key::Named(Named::Escape) => {
                        if let Some(d) = self.done() {
                            d.open.clear();
                        }
                        Task::none()
                    }
                    _ => Task::none(),
                }
            }
            Msg::Url(u) => {
                let _ = open::that_detached(u);
                Task::none()
            }
            Msg::Frame => {
                self.trace("frame");
                Task::none()
            }
        }
    }

    /// The account on screen read again (new packs, new tiers), staying where you were.
    fn reread(&mut self, busy: &'static str) -> Task<Msg> {
        let again = match &self.screen {
            Screen::Done(d) => Some((
                d.ready.text.clone(),
                d.ready.review.test.then_some(d.ready.review.world),
            )),
            Screen::Ask => None,
        };
        match again {
            Some((text, test)) => self.read(text, test, busy),
            None => Task::none(),
        }
    }

    pub fn subscription(&self) -> iced::Subscription<Msg> {
        let events = iced::event::listen_with(|e, status, _| match e {
            iced::Event::Window(window::Event::FileDropped(p)) => Some(Msg::Dropped(p)),
            iced::Event::Window(window::Event::Resized(s)) => Some(Msg::Resized(s)),
            iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                if status == iced::event::Status::Ignored =>
            {
                Some(Msg::Key(key, modifiers))
            }
            _ => None,
        });
        // The trace counts frames only until the first review is drawn.
        let frames = match (&self.trace, &self.screen) {
            (Some(_), _)
                if self.busy.is_some()
                    || self.trace.as_ref().is_some_and(|t| t.views.get() < 3) =>
            {
                window::frames().map(|_| Msg::Frame)
            }
            _ => iced::Subscription::none(),
        };
        iced::Subscription::batch([events, frames])
    }

    pub fn title(&self) -> String {
        match &self.screen {
            Screen::Done(d) => format!("{} · Idleon account check", view::title(&d.ready.review)),
            Screen::Ask => "Idleon account check".into(),
        }
    }
}

/// A card's widget ID, as the page's anchor names it (`m-stamps`).
pub fn card_id(key: &str) -> String {
    format!("m-{key}")
}

/// For tools that drive the app without a window (screenshots, timing): a review read the
/// way the app reads one.
pub fn ready_now(text: Option<&str>, packs: Packs, test: Option<u8>) -> Result<Ready, String> {
    prepare(text.map(Arc::from), packs, test)
}

impl App {
    /// An app showing `ready`, for tools without a window.
    pub fn showing(ready: Ready, width: f32) -> App {
        let (app, _) = App::boot_quiet();
        let tab = view::worlds(&ready.review).1;
        App {
            screen: Screen::Done(Box::new(Done {
                ready,
                tab,
                open: BTreeSet::new(),
                all: BTreeSet::new(),
                fresh_more: false,
            })),
            width,
            ..app
        }
    }

    pub fn boot_quiet() -> (App, Task<Msg>) {
        let app = App {
            screen: Screen::Ask,
            settings: sync::Settings::default(),
            packs: Packs::Buy,
            busy: None,
            error: None,
            load: 0,
            width: 1400.0,
            server: sync::server(),
            info: None,
            icon: None,
            tiers_from: "the app itself",
            report: Report {
                card: None,
                text: text_editor::Content::new(),
                said: None,
                sending: false,
            },
            linking: None,
            test_links: false,
            trace: None,
        };
        (app, Task::none())
    }

    pub fn done_mut(&mut self) -> Option<&mut Done> {
        self.done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every screen the app draws builds, for an empty account in each world: every card open
    /// with its full list, every tab, a Should have line followed.
    #[test]
    fn every_screen_builds() {
        for w in 1..=7u8 {
            let ready = ready_now(None, Packs::Buy, Some(w)).expect("an empty save reads");
            let n = ready.review.mechanics.len();
            let mut app = App::showing(ready, if w % 2 == 0 { 700.0 } else { 1600.0 });
            let _ = app.view();
            for i in 0..n {
                let _ = app.update(Msg::Toggle(i));
                let _ = app.update(Msg::All(i));
            }
            for tab in (1..=7u8).chain([0]) {
                let _ = app.update(Msg::Tab(tab));
                let _ = app.view();
            }
            let _ = app.update(Msg::Go(0));
            let _ = app.update(Msg::FreshMore);
            let _ = app.view();
            let _ = app.update(Msg::Again);
            let _ = app.view();
        }
    }

    #[test]
    fn a_slot_gives_its_value_once() {
        let s = Slot::new(3);
        let c = s.clone();
        assert_eq!(s.take(), Some(3));
        assert_eq!(c.take(), None);
    }
}
