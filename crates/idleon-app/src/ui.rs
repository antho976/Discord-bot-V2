//! What the app draws: the web page's sections in the web page's order, with its sizes, words
//! and layout rules (columns by width, two rows of Should have boxes at most, an open card across
//! its row). The words come from `idleon_core::view`, the same as the page's.

use iced::widget::{
    Space, button, column, container, pick_list, row, scrollable, text, text_editor, tooltip,
};
use iced::{Alignment, Color, Element, Fill, FillPortion, Length, Padding, Shrink};

use idleon_core::game::GAME;
use idleon_core::review::{Count, Item, Mechanic, Packs, Review, State as Step, Tier};
use idleon_core::view::{self, Chip, OweBox, OweLine};

use crate::app::{App, CARD_TEXT, Done, Msg, PAGE, Screen, card_id};
use crate::pics::pic;
use crate::style::{self, INK, INK_2, INK_3, LIVE, MEDIUM, MONO, MONO_SEMI, SEMI, WAITING};

type El<'a> = Element<'a, Msg>;

/// The widest the content gets, as the page's column.
const MAX: f32 = 1500.0;
/// A card's narrowest before the grid takes a column fewer, and the gap (`.ia-grid`).
const CARD_MIN: f32 = 340.0;
const GAP: f32 = 12.0;
/// The scrollbar's width, which the content doesn't get.
const BAR: f32 = 10.0;

fn t<'a>(s: impl text::IntoFragment<'a>, size: f32, color: Color) -> text::Text<'a> {
    text(s).size(size).color(color)
}

/// A line that never wraps, cut at its box's edge (`white-space: nowrap; overflow: hidden`).
fn one_line<'a>(e: impl Into<El<'a>>) -> El<'a> {
    container(e).width(Fill).clip(true).into()
}

/// A text link (`Check another account`, `Show all 40`): underlined, the button's colour.
fn link<'a>(label: impl Into<String>, size: f32, msg: Msg) -> El<'a> {
    let spans: Vec<text::Span<'a, ()>> =
        vec![text::Span::new(label.into()).size(size).underline(true)];
    button(iced::widget::rich_text(spans))
        .padding(0)
        .style(style::link)
        .on_press(msg)
        .into()
}

fn gutter(width: f32) -> f32 {
    (width * 0.022).clamp(24.0, 44.0)
}

impl App {
    /// The content column's width at the window's width.
    pub fn content(&self) -> f32 {
        (self.width - 2.0 * gutter(self.width) - BAR).clamp(200.0, MAX)
    }

    pub fn view(&self) -> El<'_> {
        let started = std::time::Instant::now();
        let w = self.content();
        let body: El = match &self.screen {
            Screen::Ask => column![self.header(None), self.ask(w)].spacing(22).into(),
            Screen::Done(d) => self.results(d, w),
        };
        let page = container(column![body, self.footer()].spacing(22).max_width(MAX))
            .padding(Padding {
                top: 0.0,
                right: gutter(self.width),
                bottom: 72.0,
                left: gutter(self.width),
            })
            .center_x(Fill);
        let el = scrollable(page).id(PAGE).height(Fill).style(style::scroll);
        if let Some(tr) = &self.trace {
            tr.views.set(tr.views.get() + 1);
            tr.view_ms.set(started.elapsed().as_secs_f64() * 1000.0);
            eprintln!(
                "[trace] {:>8.1} ms  view built in {:.2} ms",
                tr.start.elapsed().as_secs_f64() * 1000.0,
                tr.view_ms.get()
            );
        }
        container(el)
            .style(|_| container::Style {
                background: Some(style::PAGE.into()),
                ..Default::default()
            })
            .into()
    }

    /// The server's icon and name, and whose account this is (the page's hero, trimmed to a head).
    fn header(&self, r: Option<&Review>) -> El<'_> {
        let title = r.map_or_else(|| "Account check".to_string(), view::title);
        let guild = self.guild().map(|g| g.name.clone());
        let mark: El = match &self.icon {
            Some(h) => iced::widget::image(h.clone())
                .width(48)
                .height(48)
                .border_radius(12.0)
                .into(),
            None => container(
                t(
                    guild
                        .as_deref()
                        .and_then(|g| g.chars().next())
                        .map(|c| c.to_uppercase().to_string())
                        .unwrap_or_else(|| "I".into()),
                    20.0,
                    INK,
                )
                .font(SEMI),
            )
            .center(48)
            .style(style::fill(style::SOLID, 12.0))
            .into(),
        };
        let sub = match guild {
            Some(g) => format!("{g} · Idleon account check"),
            None => "Idleon account check".into(),
        };
        row![
            mark,
            column![t(title, 26.0, INK).font(SEMI), t(sub, 13.0, INK_3)].spacing(2)
        ]
        .spacing(14)
        .align_y(Alignment::Center)
        .padding(Padding {
            top: 28.0,
            bottom: 2.0,
            ..Padding::ZERO
        })
        .into()
    }

    fn ask(&self, w: f32) -> El<'_> {
        let words = column![
            t("Paste your save", 15.0, INK).font(SEMI),
            t("Copy your account's JSON from IdleonToolbox or Idleon Efficiency and paste it here, or open the file. You get how far each mechanic is from the game's max, and what to work on next.", 14.0, INK_2).line_height(1.55),
        ]
        .spacing(6);
        let drop = container(
            column![
                t(self.busy.unwrap_or("Drop your save file here"), 14.0, INK_2),
                t(
                    "or paste it with Ctrl+V, or open it with Ctrl+O",
                    13.0,
                    INK_3
                )
            ]
            .spacing(4)
            .align_x(Alignment::Center),
        )
        .center_x(Fill)
        .center_y(150)
        .style(|_| container::Style {
            background: Some(style::WELL.into()),
            border: iced::Border {
                radius: 10.0.into(),
                width: 1.0,
                color: style::EDGE_1,
            },
            ..Default::default()
        });
        let mut form = column![drop].spacing(12);
        if let Some(e) = &self.error {
            form = form.push(t(e.as_str(), 13.5, style::HELD));
        }
        form = form.push(packs(self.packs));
        let actions = row![
            Space::new().width(Fill),
            button(t("Open a file", 13.5, INK).font(MEDIUM))
                .padding([8, 14])
                .style(style::btn)
                .on_press(Msg::OpenFile),
            button(t("Paste from clipboard", 13.5, Color::WHITE).font(MEDIUM))
                .padding([8, 14])
                .style(style::primary)
                .on_press_maybe(self.busy.is_none().then_some(Msg::Paste)),
        ]
        .spacing(10);
        form = form.push(actions);
        let top: El = if w >= 900.0 {
            row![container(words).width(320), form.width(Fill)]
                .spacing(28)
                .into()
        } else {
            column![words, form].spacing(18).into()
        };
        let mut panel = column![top].spacing(18);
        if self.test_links {
            let mut links = row![t(
                "Test account, everything at 0, seen as a player in",
                13.0,
                INK_3
            )]
            .spacing(14)
            .align_y(Alignment::Center);
            for wd in 1..=7u8 {
                links = links.push(
                    button(t(format!("World {wd}"), 13.0, INK_2))
                        .padding(0)
                        .style(style::link)
                        .on_press(Msg::Test(wd)),
                );
            }
            panel = panel
                .push(
                    container(Space::new().height(1))
                        .width(Fill)
                        .style(style::rule),
                )
                .push(links.wrap().vertical_spacing(6));
        }
        container(panel)
            .padding([20, 22])
            .width(Fill)
            .style(style::panel)
            .into()
    }

    fn results<'a>(&'a self, d: &'a Done, w: f32) -> El<'a> {
        let r = &d.ready.review;
        let mut page = column![self.header(Some(r))].spacing(22);
        if r.test {
            page = page.push(
                container(row![t("Test account.", 13.5, WAITING).font(SEMI), t(format!(" Everything is at 0, as a World {} player would see the page. Paste a real save to check one.", r.world), 13.5, INK_2)])
                    .padding([12, 16])
                    .width(Fill)
                    .style(style::note(WAITING, true)),
            );
        }
        if self.guild().is_some_and(|g| g.closed) {
            page = page.push(
                container(row![t("Closed test.", 13.5, WAITING).font(SEMI), t(" This is a test version: some numbers or advice may be off. If something looks wrong, use Report a problem at the bottom.", 13.5, INK_2)].wrap())
                    .padding([12, 16])
                    .width(Fill)
                    .style(style::note(WAITING, true)),
            );
        }
        page = page.push(self.lead(r, w));
        if !r.not_started.is_empty() {
            page = page.push(not_started(r, d.fresh_more, w));
        }
        if let Some(o) = &d.ready.owing {
            page = page.push(should_have(r, &o.boxes, &view::owing_about(o), w));
        }
        page = page.push(worlds(d, w));
        page = page.push(self.report(r, w));
        page.into()
    }

    fn lead<'a>(&'a self, r: &'a Review, w: f32) -> El<'a> {
        let mut line = row![t(
            format!("World {} · {} characters", r.world, r.characters),
            13.0,
            INK_3
        )]
        .align_y(Alignment::Center);
        if let Some(p) = view::packs_line(r) {
            line = line.push(t(format!(" · {p}"), 13.0, INK_3));
        }
        line =
            line.push(t(" · ", 13.0, INK_3))
                .push(link("Check another account", 13.0, Msg::Again));
        if let Some(b) = self.busy {
            line = line.push(t(format!(" · {b}"), 13.0, INK_3));
        }
        let who = column![
            t(view::pct(r.share()), 64.0, INK)
                .font(SEMI)
                .line_height(1.0),
            t(
                format!(
                    "of the maxes this check reads, across {} mechanics",
                    view::counted(r)
                ),
                14.0,
                INK_2
            ),
            container(line.wrap()).padding(Padding {
                top: 10.0,
                ..Padding::ZERO
            }),
        ]
        .spacing(4);
        let mut cols = row![].spacing(10).height(150);
        for wd in (1..=7u8).chain([0]) {
            let (share, tip, n) = view::world_col(r, wd);
            let c = style::world(wd);
            let s = share.unwrap_or(0.0).clamp(0.0, 1.0);
            let filled = (s * 1000.0).round() as u16;
            let track_bg = if share.is_some() {
                style::mix(c, 0.12, style::WELL)
            } else {
                style::WELL
            };
            let fill = container(Space::new())
                .width(Fill)
                .height(if filled == 0 {
                    Length::Fixed(3.0)
                } else {
                    FillPortion(filled)
                })
                .style(style::fill(
                    if share.is_some() { c } else { style::EDGE_1 },
                    6.0,
                ));
            let bar = if filled >= 1000 {
                column![fill]
            } else {
                column![Space::new().height(FillPortion(1000 - filled)), fill]
            };
            let track = container(bar)
                .width(Fill)
                .max_width(34)
                .height(Fill)
                .style(style::fill(track_bg, 6.0))
                .clip(true);
            let col = column![
                container(track).center_x(Fill).height(Fill),
                t(n, 13.0, if share.is_some() { INK } else { INK_3 }).font(MONO_SEMI),
                t(view::world_tag(wd), 12.0, INK_3),
            ]
            .spacing(5)
            .align_x(Alignment::Center)
            .width(Fill);
            cols = cols.push(tooltip(
                col,
                container(t(tip, 12.5, INK))
                    .padding([6, 10])
                    .style(style::tooltip),
                tooltip::Position::Top,
            ));
        }
        let inner: El = if w >= 820.0 {
            row![
                container(who).width(Fill).align_bottom(Fill),
                container(cols).width(Length::Fixed((w * 0.46).max(360.0)))
            ]
            .spacing(22)
            .align_y(Alignment::End)
            .into()
        } else {
            column![who, cols].spacing(22).into()
        };
        container(inner)
            .padding([20, 22])
            .width(Fill)
            .style(style::panel)
            .into()
    }

    fn report<'a>(&'a self, r: &'a Review, w: f32) -> El<'a> {
        let signed = self.settings.session.as_ref();
        let mut words = column![
            t("Report a problem", 15.0, INK).font(SEMI),
            t("A number that's wrong, advice that doesn't make sense, something missing: tell staff here. It goes to the suggestion box with your name, and says which account and card it's about.", 14.0, INK_2).line_height(1.55),
        ]
        .spacing(6);
        if let Some(s) = signed {
            words = words.push(
                row![
                    t("Signed in as ", 13.0, INK_3),
                    t(s.name.as_str(), 13.0, INK_2).font(MEDIUM),
                    Space::new().width(10),
                    link("Sign out", 13.0, Msg::SignOut)
                ]
                .align_y(Alignment::Center)
                .padding(Padding {
                    top: 8.0,
                    ..Padding::ZERO
                }),
            );
        }
        let mut cards: Vec<String> = vec![CARD_TEXT.to_string()];
        cards.extend(r.mechanics.iter().map(|m| m.name.to_string()));
        let chosen = Some(
            self.report
                .card
                .clone()
                .unwrap_or_else(|| CARD_TEXT.to_string()),
        );
        let mut form = column![].spacing(12);
        let reachable = self.guild().is_some();
        if signed.is_none() {
            let note = match (&self.linking, reachable) {
                (Some(code), _) => format!("Waiting for your browser: sign in there and check it shows {code}."),
                (None, true) => "Sign in with Discord to send it from here, or copy it and send it with /suggest in the server.".into(),
                (None, false) => "The app can't reach the server right now, so copy it and send it with /suggest in the server.".into(),
            };
            form = form.push(t(note, 13.0, INK_3));
        }
        let mut card_row = row![
            t("Card (optional)", 13.0, INK_2),
            pick_list(cards, chosen, Msg::ReportCard)
                .text_size(13.0)
                .padding([7, 10])
        ]
        .spacing(10)
        .align_y(Alignment::Center);
        if let Some(info) = &self.info
            && info.guilds.len() > 1
        {
            let names: Vec<String> = info.guilds.iter().map(|g| g.name.clone()).collect();
            let now = self.guild().map(|g| g.name.clone());
            let gs = info.guilds.clone();
            card_row = card_row
                .push(Space::new().width(Fill))
                .push(t("Server", 13.0, INK_2))
                .push(
                    pick_list(names, now, move |n| {
                        Msg::Guild(
                            gs.iter()
                                .find(|g| g.name == n)
                                .map(|g| g.id.clone())
                                .unwrap_or_default(),
                        )
                    })
                    .text_size(13.0)
                    .padding([7, 10]),
                );
        }
        form = form.push(card_row);
        form = form.push(
            text_editor(&self.report.text)
                .placeholder("What looks wrong, and what you expected")
                .height(110)
                .padding([12, 14])
                .size(14.0)
                .style(style::editor)
                .on_action(Msg::ReportEdit),
        );
        let said = t(self.report.said.as_deref().unwrap_or(""), 13.0, INK_2).width(Fill);
        let actions: El = match signed {
            Some(_) => row![
                said,
                button(t("Send to staff", 13.5, Color::WHITE).font(MEDIUM))
                    .padding([8, 14])
                    .style(style::primary)
                    .on_press_maybe((!self.report.sending && reachable).then_some(Msg::ReportSend))
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .into(),
            None => {
                let mut a = row![
                    said,
                    button(t("Copy for /suggest", 13.5, INK).font(MEDIUM))
                        .padding([8, 14])
                        .style(style::btn)
                        .on_press(Msg::ReportCopy)
                ]
                .spacing(10)
                .align_y(Alignment::Center);
                if reachable {
                    a = a.push(
                        button(t("Sign in with Discord to send", 13.5, Color::WHITE).font(MEDIUM))
                            .padding([8, 14])
                            .style(style::primary)
                            .on_press_maybe(self.linking.is_none().then_some(Msg::SignIn)),
                    );
                }
                a.into()
            }
        };
        form = form.push(actions);
        let inner: El = if w >= 900.0 {
            row![container(words).width(320), form.width(Fill)]
                .spacing(28)
                .into()
        } else {
            column![words, form].spacing(18).into()
        };
        container(inner)
            .padding([20, 22])
            .width(Fill)
            .style(style::panel)
            .into()
    }

    fn footer(&self) -> El<'_> {
        column![
            container(Space::new().height(1)).width(Fill).style(style::rule),
            t(
                format!(
                    "Maxes and names come from the game's own data, from {}. Tiers from {}. Your save is read on this computer and never sent anywhere; the app keeps the last one to open on it.",
                    GAME.updated, self.tiers_from
                ),
                12.5,
                INK_3
            ),
        ]
        .spacing(16)
        .into()
    }
}

/// The packs choice: three trim buttons, the chosen one in the accent's wash.
fn packs<'a>(now: Packs) -> El<'a> {
    let mut r = row![t("Packs", 13.0, INK_3)]
        .spacing(6)
        .align_y(Alignment::Center);
    for (p, tip) in [
        (Packs::Buy, "Every tier counts everything."),
        (
            Packs::NoMore,
            "Leaves out what only a pack you don't own gives: the Potluck Pack's vault levels, pack-only Gallery items.",
        ),
        (
            Packs::Free,
            "That, and the Gem Shop leaves Should have by now.",
        ),
    ] {
        let b = button(t(p.label(), 13.0, if p == now { INK } else { INK_2 }))
            .padding([6, 11])
            .style(style::choice(p == now))
            .on_press(Msg::Packs(p));
        r = r.push(
            tooltip(
                b,
                container(t(tip, 12.5, INK))
                    .padding([6, 10])
                    .max_width(320)
                    .style(style::tooltip),
                tooltip::Position::Bottom,
            )
            .delay(std::time::Duration::from_millis(400)),
        );
    }
    r.into()
}

/// A mechanic's picture, or a dot in its world's colour.
fn mark<'a>(m: &Mechanic, px: f32) -> El<'a> {
    match m.picture.as_deref().and_then(|p| pic(p, px)) {
        Some(i) => i.into(),
        None => {
            let dot = (px * 0.36).round();
            container(
                container(Space::new())
                    .width(dot)
                    .height(dot)
                    .style(style::fill(style::world(m.world), dot / 2.0)),
            )
            .center(px)
            .into()
        }
    }
}

fn item_mark<'a>(i: &Item, px: f32, dim: bool) -> El<'a> {
    match i.picture.as_deref().and_then(|p| pic(p, px)) {
        Some(img) => img.opacity(if dim { 0.55_f32 } else { 1.0_f32 }).into(),
        None => container(
            container(Space::new())
                .width(6)
                .height(6)
                .style(style::fill(style::EDGE_3, 3.0)),
        )
        .center(px)
        .into(),
    }
}

/// A bar: the world's colour over its own wash.
fn meter<'a>(share: f64, c: Color, h: f32) -> El<'a> {
    let s = (share.clamp(0.0, 1.0) * 1000.0).round() as u16;
    let fill: El = if s == 0 {
        container(Space::new())
            .width(2)
            .height(Fill)
            .style(style::fill(c, h / 2.0))
            .into()
    } else {
        container(Space::new())
            .width(FillPortion(s))
            .height(Fill)
            .style(style::fill(c, h / 2.0))
            .into()
    };
    // A zero portion isn't nothing to iced: a full bar is the fill alone.
    let bar: El = if s >= 1000 {
        fill
    } else {
        row![fill, Space::new().width(FillPortion(1000 - s))].into()
    };
    container(bar)
        .height(h)
        .width(Fill)
        .style(style::fill(style::mix(c, 0.14, style::WELL), h / 2.0))
        .into()
}

fn rule<'a>() -> El<'a> {
    container(Space::new().height(1))
        .width(Fill)
        .style(style::rule)
        .into()
}

/// Rows with a rule between them (`border-bottom` on all but the last).
fn ruled<'a>(rows: impl IntoIterator<Item = El<'a>>) -> iced::widget::Column<'a, Msg> {
    let mut c = column![];
    for (i, r) in rows.into_iter().enumerate() {
        if i > 0 {
            c = c.push(rule());
        }
        c = c.push(r);
    }
    c
}

/// A section's heading and its note: the note at the right end when both fit on a line, under
/// the heading when they don't (`.ia-head`, space-between, wrapping).
fn head<'a>(h: &'a str, p: String, w: f32) -> El<'a> {
    // Geist at 13 px averages about 6.4 px a character; the heading takes about 190.
    if 190.0 + p.chars().count() as f32 * 6.4 <= w {
        row![
            t(h, 15.0, INK).font(SEMI),
            Space::new().width(Fill),
            t(p, 13.0, INK_3)
        ]
        .spacing(14)
        .align_y(Alignment::End)
        .into()
    } else {
        column![t(h, 15.0, INK).font(SEMI), t(p, 13.0, INK_3)]
            .spacing(4)
            .into()
    }
}

fn not_started(r: &Review, more: bool, w: f32) -> El<'_> {
    let chip = |i: usize| -> El {
        let m = &r.mechanics[i];
        let c = style::world(m.world);
        button(
            row![
                mark(m, 26.0),
                t(m.name, 13.5, INK),
                container(t(view::world_tag(m.world), 11.0, c).font(MONO_SEMI))
                    .padding([2, 6])
                    .style(style::chip(c, 0.14)),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .height(40)
        .padding(Padding {
            top: 0.0,
            right: 8.0,
            bottom: 0.0,
            left: 6.0,
        })
        .style(style::fresh)
        .on_press(Msg::Go(i))
        .into()
    };
    let shown = if more {
        r.not_started.len()
    } else {
        r.not_started.len().min(view::FRESH)
    };
    let chips = row(r.not_started.iter().take(shown).map(|&i| chip(i)))
        .spacing(8)
        .wrap()
        .vertical_spacing(8);
    let mut c = column![head("Not started yet", "In worlds you've reached, with no progress at all. New since you last played, or skipped".into(), w - 44.0), chips].spacing(10);
    if r.not_started.len() > shown {
        c = c.push(
            container(link(
                format!("Show {} more", r.not_started.len() - shown),
                13.0,
                Msg::FreshMore,
            ))
            .padding(Padding {
                top: 10.0,
                ..Padding::ZERO
            }),
        );
    }
    container(c)
        .padding([20, 22])
        .width(Fill)
        .style(style::panel)
        .into()
}

fn should_have<'a>(r: &'a Review, boxes: &'a [OweBox], about: &str, w: f32) -> El<'a> {
    let (cols, shown) = view::owe_columns(w);
    let mut grid = column![].spacing(GAP);
    for chunk in boxes[..boxes.len().min(shown)].chunks(cols) {
        let mut line = row![].spacing(GAP);
        for b in chunk {
            line = line.push(owe_box(r, b));
        }
        for _ in chunk.len()..cols {
            line = line.push(Space::new().width(Fill));
        }
        grid = grid.push(line);
    }
    column![head("Should have by now", about.to_string(), w), grid]
        .spacing(12)
        .into()
}

fn owe_box<'a>(r: &'a Review, b: &'a OweBox) -> El<'a> {
    let c = style::world(b.world);
    let chip: El = match b.chip {
        Chip::Goal => container(t(b.chip.label(), 12.0, INK_2).font(SEMI))
            .padding([3, 8])
            .style(style::chip(INK_2, 0.12))
            .into(),
        Chip::Missed => container(t(b.chip.label(), 12.0, WAITING).font(SEMI))
            .padding([3, 8])
            .style(style::chip(WAITING, 0.14))
            .into(),
        Chip::NextUp => container(t(b.chip.label(), 12.0, c).font(SEMI))
            .padding([3, 8])
            .style(style::chip(c, 0.14))
            .into(),
    };
    let headline = row![
        t(b.label.as_str(), 14.0, c).font(SEMI),
        chip,
        Space::new().width(Fill),
        t(b.count.as_str(), 13.0, INK_3)
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let lines = ruled(b.lines.iter().map(|l| owe_line(r, l)));
    let list = container(scrollable(lines)
            .direction(scrollable::Direction::Vertical(scrollable::Scrollbar::new().width(4).scroller_width(4).spacing(8))).style(style::scroll).height(Shrink)).max_height(312);
    container(column![headline, list].spacing(6))
        .padding(Padding {
            top: 14.0,
            right: 16.0,
            bottom: 6.0,
            left: 16.0,
        })
        .width(Fill)
        .style(style::panel)
        .into()
}

fn owe_line<'a>(r: &'a Review, l: &'a OweLine) -> El<'a> {
    let m = &r.mechanics[l.mechanic];
    let name = match &l.item {
        Some(i) => row![
            t(m.name, 14.0, INK).font(MEDIUM).wrapping(text::Wrapping::None),
            t(format!(" · {i}"), 14.0, INK_3).wrapping(text::Wrapping::None)
        ],
        None => row![t(m.name, 14.0, INK).font(MEDIUM).wrapping(text::Wrapping::None)],
    };
    let top = row![
        mark(m, 24.0),
        one_line(name),
        t(l.tiers.as_str(), 12.5, INK_2).font(MONO)
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let mut c = column![top].spacing(2);
    if let Some(n) = &l.need {
        c = c.push(row![
            Space::new().width(34),
            t(n.as_str(), 12.0, WAITING).line_height(1.35)
        ]);
    }
    let b = button(c)
        .padding(Padding {
            top: 8.0,
            right: 4.0,
            bottom: 8.0,
            left: 0.0,
        })
        .width(Fill)
        .style(style::row_btn(6.0))
        .on_press(Msg::Go(l.mechanic));
    tooltip(
        b,
        container(t(l.title.as_str(), 12.5, INK))
            .padding([6, 10])
            .max_width(360)
            .style(style::tooltip),
        tooltip::Position::Bottom,
    )
    .delay(std::time::Duration::from_millis(600))
    .into()
}

fn worlds<'a>(d: &'a Done, w: f32) -> El<'a> {
    let r = &d.ready.review;
    let (ws, _) = view::worlds(r);
    let mut tabs = row![].spacing(4);
    for &wd in &ws {
        let c = style::world(wd);
        let mut label = row![
            container(Space::new())
                .width(8)
                .height(8)
                .style(style::fill(c, 4.0)),
            t(
                view::world_name(wd),
                13.5,
                if d.tab == wd { INK } else { INK_2 }
            )
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        if wd > r.world && wd != 0 {
            label = label.push(t("not reached", 11.5, INK_3));
        }
        tabs = tabs.push(
            button(label)
                .height(34)
                .padding([0, 13])
                .style(style::tab(c, d.tab == wd))
                .on_press(Msg::Tab(wd)),
        );
    }
    let cols = (((w + GAP) / (CARD_MIN + GAP)).floor() as usize).max(1);
    let mut grid = column![].spacing(GAP);
    let mut line: Vec<El> = Vec::new();
    let flush = |grid: iced::widget::Column<'a, Msg>, line: &mut Vec<El<'a>>| {
        if line.is_empty() {
            return grid;
        }
        let n = line.len();
        let mut rw = row(std::mem::take(line)).spacing(GAP);
        for _ in n..cols {
            rw = rw.push(Space::new().width(Fill));
        }
        grid.push(rw)
    };
    for (i, m) in r
        .mechanics
        .iter()
        .enumerate()
        .filter(|(_, m)| m.world == d.tab)
    {
        if d.open.contains(&i) {
            // An open card takes the whole row, after the cards before it.
            grid = flush(grid, &mut line);
            grid = grid.push(card(d, i, m, w, true));
        } else {
            line.push(card(d, i, m, w, false));
            if line.len() == cols {
                grid = flush(grid, &mut line);
            }
        }
    }
    grid = flush(grid, &mut line);
    column![tabs.wrap().vertical_spacing(4), grid]
        .spacing(14)
        .into()
}

fn card<'a>(d: &'a Done, i: usize, m: &'a Mechanic, w: f32, open: bool) -> El<'a> {
    let r = &d.ready.review;
    let tiers = &d.ready.tiers[i];
    let c = style::world(m.world);
    let right: El = if m.count == Count::Open {
        t("No max", 12.0, INK_3).into()
    } else {
        t(
            view::pct(m.share()),
            15.0,
            if m.done() { LIVE } else { INK },
        )
        .font(MONO_SEMI)
        .into()
    };
    let mut summary = column![
        row![
            mark(m, 28.0),
            column![
                one_line(
                    t(m.name, 14.0, INK)
                        .font(MEDIUM)
                        .wrapping(text::Wrapping::None)
                ),
                one_line(
                    t(view::card_line(m, tiers, r.world), 12.5, INK_3)
                        .font(MONO)
                        .wrapping(text::Wrapping::None)
                )
            ]
            .width(Fill),
            right,
        ]
        .spacing(12)
        .align_y(Alignment::Center)
    ]
    .spacing(10);
    // A card with no bar keeps the bar's room, so a row of cards stays one height.
    summary = summary.push(if m.count != Count::Open {
        meter(m.share(), c, 5.0)
    } else {
        Space::new().height(5).into()
    });
    let head = button(summary)
        .padding([14, 16])
        .width(Fill)
        .style(style::row_btn(14.0))
        .on_press(Msg::Toggle(i));
    let mut whole = column![head];
    if open {
        whole = whole.push(body(d, i, m, tiers, w));
    }
    container(whole)
        .id(card_id(m.key))
        .width(Fill)
        .style(style::panel)
        .into()
}

fn body<'a>(d: &'a Done, i: usize, m: &'a Mechanic, tiers: &'a [Tier], w: f32) -> El<'a> {
    let reached = d.ready.review.world;
    let c = style::world(m.world);
    let current = tiers.iter().position(|t| !t.complete());
    let side = current.is_some() && (m.have.is_some() || !m.raises.is_empty());
    let mut b = column![].spacing(18);
    if view::on_hold(m) {
        b = b.push(
            container(row![t("Order coming later.", 13.0, WAITING).font(SEMI), t(" The numbers here are right, but these tiers aren't in the order you'd really do them yet. That comes in a later update.", 13.0, INK_2)].wrap())
                .padding([10, 14])
                .width(Fill)
                .style(style::note(WAITING, false)),
        );
    }
    if let Some(n) = &m.note {
        b = b.push(t(n.as_str(), 13.0, INK_3));
    }
    if let Some(line) = m.pace.as_ref().and_then(|p| view::pace_line(p, reached)) {
        b = b.push(t(line, 13.0, INK_2));
    }
    if m.count == Count::Open
        && let Some([avg, median, top, low]) = m.spread()
    {
        let stat = |v: String, k: &'static str| {
            column![t(v, 18.0, INK).font(MONO_SEMI), t(k, 12.0, INK_3)].spacing(1)
        };
        b = b.push(
            row![
                stat(view::num(avg.round()), "Average"),
                stat(view::num(median.round()), "Median"),
                stat(view::num(top), "Highest"),
                stat(view::num(low), "Lowest")
            ]
            .spacing(32)
            .wrap()
            .vertical_spacing(8),
        );
    }
    if !tiers.is_empty() {
        b = b.push(ladder(m, tiers, current, w));
    }
    let tasks = tasks(m, tiers, current);
    let work: El = if side {
        let s = side_boxes(m);
        if w >= 1000.0 {
            row![container(tasks).width(Fill), container(s).width(400)]
                .spacing(24)
                .into()
        } else {
            column![tasks, s].spacing(24).into()
        }
    } else {
        tasks
    };
    b = b.push(work);
    if let Some(l) = view::later_worlds(m) {
        b = b.push(t(l, 13.0, INK_3));
    }
    if !m.items.is_empty() {
        let shown = d.all.contains(&i);
        b = b.push(link(
            if shown {
                "Hide the list".to_string()
            } else {
                format!("Show all {}", m.items.len())
            },
            13.0,
            Msg::All(i),
        ));
        if shown {
            b = b.push(all_items(m, c, w));
        }
    }
    container(b)
        .padding(Padding {
            top: 4.0,
            right: 16.0,
            bottom: 18.0,
            left: 16.0,
        })
        .width(Fill)
        .style(style::plain)
        .into()
}

fn ladder<'a>(m: &'a Mechanic, tiers: &'a [Tier], current: Option<usize>, w: f32) -> El<'a> {
    let (from, to) = view::window(tiers.len(), current);
    let mut out = column![].spacing(6);
    if from > 0 || to < tiers.len() {
        let mut s = format!("Tiers {}–{} of {}", from + 1, to, tiers.len());
        if from > 0 {
            s.push_str(&format!(" · {from} before these done"));
        }
        out = out.push(t(s, 12.0, INK_3));
    }
    let c = style::world(m.world);
    let rungs = tiers
        .iter()
        .enumerate()
        .skip(from)
        .take(to - from)
        .map(|(k, tier)| {
            let here = current == Some(k);
            let (state, kc) = if here {
                (" · you're here", c)
            } else if tier.complete() {
                (" · done", LIVE)
            } else {
                ("", INK_2)
            };
            let mut top = row![
                t(format!("Tier {}{state}", k + 1), 12.0, kc),
                Space::new().width(Fill)
            ];
            if let Some(wd) = m
                .pace
                .as_ref()
                .and_then(|p| p.worlds.get(k).copied().flatten())
            {
                top = top.push(t(format!("W{wd}"), 11.0, style::world(wd)).font(MONO_SEMI));
            }
            let mut r = column![
                top,
                one_line(
                    t(tier.label.as_str(), 13.5, INK)
                        .font(MEDIUM)
                        .wrapping(text::Wrapping::None)
                ),
                t(format!("{} of {}", tier.met, tier.need), 12.5, INK_2).font(MONO),
                container(meter(tier.met as f64 / tier.need.max(1) as f64, c, 4.0)).padding(
                    Padding {
                        top: 2.0,
                        ..Padding::ZERO
                    }
                )
            ]
            .spacing(4);
            if let Some(n) = m
                .tier_notes
                .get(k)
                .filter(|n| !n.is_empty() && m.crowd.is_none())
            {
                r = r.push(t(n.as_str(), 12.0, INK_3).line_height(1.35));
            }
            let e: El = container(r)
                .padding([11, 13])
                .width(Fill)
                .style(style::rung(c, here))
                .into();
            e
        });
    // Narrow, the ladder is a short list: a row per tier.
    let rungs: El = if w >= 700.0 {
        row(rungs).spacing(8).into()
    } else {
        column(rungs).spacing(4).into()
    };
    out.push(rungs).into()
}

fn tasks<'a>(m: &'a Mechanic, tiers: &'a [Tier], current: Option<usize>) -> El<'a> {
    match current {
        Some(k) => {
            let tier = &tiers[k];
            let tasks = m.tasks(tier);
            let mut c = column![
                t(format!("To reach Tier {}", k + 1), 15.0, INK).font(SEMI),
                t(view::tasks_note(tier, &tasks), 13.0, INK_3)
            ]
            .spacing(6);
            c = c.push(ruled(
                tasks
                    .iter()
                    .take(view::TASKS)
                    .map(|(i, target)| task(m, i, *target)),
            ));
            if tasks.len() > view::TASKS {
                c = c.push(t(
                    format!(
                        "And {} more, in the full list below.",
                        tasks.len() - view::TASKS
                    ),
                    13.0,
                    INK_3,
                ));
            }
            if k + 1 < tiers.len() {
                c = c.push(container(t("AFTER THAT", 12.0, INK_2)).padding(Padding {
                    top: 12.0,
                    ..Padding::ZERO
                }));
                c =
                    c.push(ruled(
                        tiers.iter().enumerate().skip(k + 1).take(view::LATER).map(
                            |(j, tr)| -> El {
                                row![
                                    t(format!("Tier {} · {}", j + 1, tr.label), 13.5, INK)
                                        .wrapping(text::Wrapping::None),
                                    container(
                                        t(view::later_who(m, tr), 13.0, INK_3)
                                            .wrapping(text::Wrapping::None)
                                    )
                                    .width(Fill)
                                    .align_right(Fill)
                                    .clip(true)
                                ]
                                .spacing(16)
                                .padding([9, 0])
                                .align_y(Alignment::Center)
                                .into()
                            },
                        ),
                    ));
                if tiers.len() > k + 1 + view::LATER {
                    c = c.push(t(
                        format!(
                            "And {} more tiers after those.",
                            tiers.len() - k - 1 - view::LATER
                        ),
                        13.0,
                        INK_3,
                    ));
                }
            }
            c.into()
        }
        None if !tiers.is_empty() => t("Every tier done.", 13.0, INK_3).into(),
        None => {
            let open = m.open_items();
            let mut c =
                column![t(m.open_order.unwrap_or("Lowest first"), 15.0, INK).font(SEMI)].spacing(6);
            if m.open_order.is_none() {
                c = c.push(t(
                    "Nothing here has a max, so these are the ones furthest behind.",
                    13.0,
                    INK_3,
                ));
            }
            c.push(ruled(
                open.into_iter()
                    .take(view::TASKS)
                    .map(|i| item(i, style::world(m.world))),
            ))
            .into()
        }
    }
}

fn task<'a>(m: &'a Mechanic, i: &'a Item, target: f64) -> El<'a> {
    let mut text_col = column![t(i.name.as_str(), 14.0, INK).font(MEDIUM)].spacing(1);
    if let Some(h) = &i.how {
        text_col = text_col.push(t(h.as_str(), 13.0, INK_3));
    }
    let chip: El = match i.state {
        Step::Ready => container(t("Do now", 12.0, LIVE).font(SEMI))
            .padding([3, 8])
            .style(style::chip(LIVE, 0.14))
            .into(),
        Step::Locked => container(t("Locked", 12.0, WAITING).font(SEMI))
            .padding([3, 8])
            .style(style::chip(WAITING, 0.14))
            .into(),
        Step::Unknown => Space::new().into(),
    };
    row![
        item_mark(i, 28.0, i.state == Step::Locked),
        text_col.width(Fill),
        t(view::task_fig(m, i, target), 12.5, INK_2).font(MONO),
        chip
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .padding([6, 0])
    .height(Length::Shrink)
    .into()
}

fn item<'a>(i: &'a Item, c: Color) -> El<'a> {
    let ink = if i.done() { INK_3 } else { INK };
    let bar: El = if i.max > 0.0 {
        meter(
            i.share(),
            if i.done() {
                style::mix(c, 0.45, style::WELL)
            } else {
                c
            },
            5.0,
        )
    } else {
        Space::new().width(Fill).into()
    };
    row![
        item_mark(i, 24.0, false),
        one_line(t(i.name.as_str(), 13.5, ink).wrapping(text::Wrapping::None)),
        container(bar).width(FillPortion(1)).max_width(160),
        container(t(view::item_fig(i), 12.5, INK_2).font(MONO))
            .width(90)
            .align_right(90)
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .height(36)
    .into()
}

fn all_items<'a>(m: &'a Mechanic, c: Color, w: f32) -> El<'a> {
    let cols = (((w - 32.0 + 28.0) / (330.0 + 28.0)).floor() as usize).max(1);
    let grid = |items: Vec<&'a Item>| -> El<'a> {
        let mut g = column![];
        for chunk in items.chunks(cols) {
            let mut r = row![].spacing(28);
            for i in chunk {
                r = r.push(column![item(i, c), rule()].width(Fill));
            }
            for _ in chunk.len()..cols {
                r = r.push(Space::new().width(Fill));
            }
            g = g.push(r);
        }
        g.into()
    };
    let maxed: Vec<&Item> = m.items.iter().filter(|i| i.done()).collect();
    let mut out = column![grid(m.open_items())];
    if !maxed.is_empty() {
        out = out.push(
            container(t(format!("{} at max", maxed.len()), 13.0, INK_3)).padding(Padding {
                top: 14.0,
                bottom: 8.0,
                ..Padding::ZERO
            }),
        );
        out = out.push(grid(maxed));
    }
    out.into()
}

fn side_boxes(m: &Mechanic) -> El<'_> {
    let mut s = column![].spacing(12);
    if let Some(h) = &m.have {
        s = s.push(
            container(
                column![
                    t("With what you have", 12.0, INK_2),
                    t(h.figure.as_str(), 18.0, INK).font(MONO).line_height(1.3),
                    t(h.text.as_str(), 13.5, INK_2).line_height(1.45)
                ]
                .spacing(6),
            )
            .padding([14, 16])
            .width(Fill)
            .style(style::fill(style::SOLID, 12.0)),
        );
    }
    if !m.raises.is_empty() {
        let mut c = column![t("Raise the cap", 12.0, INK_2)].spacing(6);
        // The ones still to get first: those are the ones to act on.
        for r in m
            .raises
            .iter()
            .filter(|r| !r.owned)
            .chain(m.raises.iter().filter(|r| r.owned))
        {
            // A ring until you have it, then filled: the shape says it, not only the colour.
            let ring =
                container(Space::new())
                    .width(14)
                    .height(14)
                    .style(move |_: &iced::Theme| container::Style {
                        background: r.owned.then(|| LIVE.into()),
                        border: iced::Border {
                            radius: 7.0.into(),
                            width: if r.owned { 0.0 } else { 1.5 },
                            color: WAITING,
                        },
                        ..Default::default()
                    });
            c = c.push(
                row![
                    container(ring).padding(Padding {
                        top: 2.0,
                        ..Padding::ZERO
                    }),
                    column![
                        t(r.name.as_str(), 13.5, if r.owned { INK_2 } else { INK })
                            .font(if r.owned { style::SANS } else { MEDIUM }),
                        t(r.amount.as_str(), 13.0, INK_3).line_height(1.4)
                    ]
                    .spacing(1)
                    .width(Fill)
                ]
                .spacing(10)
                .padding([4, 0]),
            );
        }
        s = s.push(
            container(c)
                .padding([14, 16])
                .width(Fill)
                .style(style::fill(style::SOLID, 12.0)),
        );
    }
    s.into()
}
