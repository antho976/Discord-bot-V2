//! The web page's look, as the app draws it: the same tokens (app.css and the page's own CSS), the
//! same Geist faces, sizes and corners. Colours are named as the page names them.

use iced::font::{Family, Weight};
use iced::widget::{button, container, scrollable, text_editor};
use iced::{Background, Border, Color, Font, Shadow, Theme};

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
}

const fn ink_alpha(a: f32) -> Color {
    Color {
        r: 237.0 / 255.0,
        g: 237.0 / 255.0,
        b: 237.0 / 255.0,
        a,
    }
}

pub const PAGE: Color = rgb(0x14, 0x14, 0x15);
pub const RAISED: Color = rgb(0x19, 0x19, 0x1b);
pub const SOLID: Color = rgb(0x23, 0x23, 0x25);
pub const INK: Color = rgb(0xed, 0xed, 0xed);
pub const INK_2: Color = rgb(0xa1, 0xa1, 0xa6);
pub const INK_3: Color = rgb(0x85, 0x85, 0x8b);
pub const WELL: Color = ink_alpha(0.03);
pub const CONTROL: Color = ink_alpha(0.06);
pub const HOVER: Color = ink_alpha(0.05);
pub const EDGE_1: Color = ink_alpha(0.07);
pub const EDGE_3: Color = ink_alpha(0.2);
pub const ACCENT: Color = rgb(0x9d, 0x7b, 0xff);
pub const LIVE: Color = rgb(0x4c, 0xc3, 0x8a);
pub const HELD: Color = rgb(0xe5, 0x48, 0x4d);
pub const WAITING: Color = rgb(0xff, 0xb5, 0x47);

/// Each world's colour (data colour, not decoration): the bars, meters, tags and box titles.
pub fn world(w: u8) -> Color {
    match w {
        1 => rgb(0x7f, 0xd1, 0x8b),
        2 => rgb(0xe6, 0xc3, 0x6a),
        3 => rgb(0x79, 0xc3, 0xf0),
        4 => rgb(0xa8, 0x8b, 0xff),
        5 => rgb(0xff, 0x9a, 0x5c),
        6 => rgb(0xf2, 0x97, 0xc6),
        7 => rgb(0x46, 0xd1, 0xc0),
        _ => INK_2,
    }
}

/// `color-mix(a p%, b)`, close enough in sRGB for these small washes.
pub fn mix(a: Color, p: f32, b: Color) -> Color {
    let q = 1.0 - p;
    Color {
        r: a.r * p + b.r * q,
        g: a.g * p + b.g * q,
        b: a.b * p + b.b * q,
        a: a.a * p + b.a * q,
    }
}

/// `color-mix(c p%, transparent)`.
pub fn wash(c: Color, p: f32) -> Color {
    Color { a: c.a * p, ..c }
}

pub const SANS: Font = Font::with_name("Geist");
pub const MEDIUM: Font = Font {
    weight: Weight::Medium,
    ..SANS
};
pub const SEMI: Font = Font {
    weight: Weight::Semibold,
    ..SANS
};
pub const MONO: Font = Font {
    family: Family::Name("Geist Mono"),
    weight: Weight::Medium,
    ..Font::DEFAULT
};
pub const MONO_SEMI: Font = Font {
    weight: Weight::Semibold,
    ..MONO
};

/// The faces the app carries, so it reads the same on every computer.
pub const FONTS: [&[u8]; 5] = [
    include_bytes!("../fonts/Geist-Regular.ttf"),
    include_bytes!("../fonts/Geist-Medium.ttf"),
    include_bytes!("../fonts/Geist-SemiBold.ttf"),
    include_bytes!("../fonts/GeistMono-Medium.ttf"),
    include_bytes!("../fonts/GeistMono-SemiBold.ttf"),
];

pub fn theme() -> Theme {
    Theme::custom(
        "Account check",
        iced::theme::Palette {
            background: PAGE,
            text: INK,
            primary: ACCENT,
            success: LIVE,
            warning: WAITING,
            danger: HELD,
        },
    )
}

fn round(r: f32) -> Border {
    Border {
        radius: r.into(),
        ..Border::default()
    }
}

fn edged(r: f32, c: Color) -> Border {
    Border {
        radius: r.into(),
        width: 1.0,
        color: c,
    }
}

/// `.ia-panel`, `.ia-card`, `.ia-owe-box`: raised, an edge, 14 px corners.
pub fn panel(_: &Theme) -> container::Style {
    container::Style {
        background: Some(RAISED.into()),
        border: edged(14.0, EDGE_1),
        ..Default::default()
    }
}

/// An open card's body, the same surface without its own edge (the card has it).
pub fn plain(_: &Theme) -> container::Style {
    container::Style::default()
}

pub fn well(r: f32) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(WELL.into()),
        border: round(r),
        ..Default::default()
    }
}

pub fn fill(c: Color, r: f32) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(c.into()),
        border: round(r),
        ..Default::default()
    }
}

/// A tinted note (`.ia-test`, `.ia-hold`): the colour at a tenth over raised.
pub fn note(c: Color, edge: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(mix(c, 0.10, RAISED).into()),
        border: if edge {
            edged(12.0, wash(c, 0.35))
        } else {
            round(10.0)
        },
        ..Default::default()
    }
}

/// A rung: the well, or the world's wash with an edge when it's the one you're on.
pub fn rung(w: Color, here: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| {
        if here {
            container::Style {
                background: Some(mix(w, 0.12, RAISED).into()),
                border: edged(10.0, wash(w, 0.6)),
                ..Default::default()
            }
        } else {
            container::Style {
                background: Some(WELL.into()),
                border: round(10.0),
                ..Default::default()
            }
        }
    }
}

/// A chip: its colour at 14% behind its text, 6 px corners.
pub fn chip(c: Color, p: f32) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(wash(c, p).into()),
        border: round(6.0),
        text_color: Some(c),
        ..Default::default()
    }
}

pub fn tooltip(_: &Theme) -> container::Style {
    container::Style {
        background: Some(SOLID.into()),
        border: edged(8.0, EDGE_1),
        text_color: Some(INK),
        ..Default::default()
    }
}

/// A rule between rows: lines are data's, here the row edge the page draws.
pub fn rule(_: &Theme) -> container::Style {
    container::Style {
        background: Some(EDGE_1.into()),
        ..Default::default()
    }
}

fn hovered(s: button::Status) -> bool {
    matches!(s, button::Status::Hovered | button::Status::Pressed)
}

/// `.btn`: a control surface, ink text.
pub fn btn(_: &Theme, s: button::Status) -> button::Style {
    let bg = if hovered(s) { ink_alpha(0.10) } else { CONTROL };
    button::Style {
        background: Some(bg.into()),
        text_color: if s == button::Status::Disabled {
            INK_3
        } else {
            INK
        },
        border: edged(9.0, EDGE_1),
        shadow: Shadow::default(),
        snap: true,
    }
}

/// `.btn.primary`: the accent, filled. One per surface.
pub fn primary(_: &Theme, s: button::Status) -> button::Style {
    let bg = if hovered(s) {
        mix(ACCENT, 0.85, Color::WHITE)
    } else {
        ACCENT
    };
    let bg = if s == button::Status::Disabled {
        wash(ACCENT, 0.5)
    } else {
        bg
    };
    button::Style {
        background: Some(bg.into()),
        text_color: Color::WHITE,
        border: round(9.0),
        shadow: Shadow::default(),
        snap: true,
    }
}

/// A whole row that opens something (a card's head, a Should have line, a chip): no surface
/// until hovered.
pub fn row_btn(r: f32) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, s| button::Style {
        background: hovered(s).then_some(Background::Color(HOVER)),
        text_color: INK,
        border: round(r),
        shadow: Shadow::default(),
        snap: true,
    }
}

/// A Not started chip: a control surface.
pub fn fresh(_: &Theme, s: button::Status) -> button::Style {
    button::Style {
        background: Some(if hovered(s) { HOVER } else { CONTROL }.into()),
        text_color: INK,
        border: round(10.0),
        shadow: Shadow::default(),
        snap: true,
    }
}

/// A text link (`Check another account`, `Show all 40`).
pub fn link(_: &Theme, s: button::Status) -> button::Style {
    button::Style {
        background: None,
        text_color: if hovered(s) { INK } else { INK_2 },
        border: Border::default(),
        shadow: Shadow::default(),
        snap: true,
    }
}

/// A world tab: its colour's wash when chosen.
pub fn tab(w: Color, on: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, s| {
        let (bg, border, ink) = if on {
            (
                Some(mix(w, 0.14, mix(CONTROL, 1.0, RAISED)).into()),
                edged(9.0, wash(w, 0.5)),
                INK,
            )
        } else if hovered(s) {
            (Some(HOVER.into()), round(9.0), INK)
        } else {
            (None, round(9.0), INK_2)
        };
        button::Style {
            background: bg,
            text_color: ink,
            border,
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// A packs choice: the accent's wash and edge when chosen.
pub fn choice(on: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, s| {
        let (bg, border, ink) = if on {
            (
                Some(wash(ACCENT, 0.16).into()),
                edged(8.0, wash(ACCENT, 0.55)),
                INK,
            )
        } else {
            (
                None,
                edged(8.0, EDGE_1),
                if hovered(s) { INK } else { INK_2 },
            )
        };
        button::Style {
            background: bg,
            text_color: ink,
            border,
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

pub fn scroll(_: &Theme, s: scrollable::Status) -> scrollable::Style {
    let hover = matches!(
        s,
        scrollable::Status::Hovered { .. } | scrollable::Status::Dragged { .. }
    );
    let rail = scrollable::Rail {
        background: None,
        border: Border::default(),
        scroller: scrollable::Scroller {
            background: (if hover { EDGE_3 } else { ink_alpha(0.12) }).into(),
            border: round(4.0),
        },
    };
    scrollable::Style {
        container: container::Style::default(),
        vertical_rail: rail,
        horizontal_rail: rail,
        gap: None,
        auto_scroll: scrollable::AutoScroll {
            background: SOLID.into(),
            border: round(8.0),
            shadow: Shadow::default(),
            icon: INK,
        },
    }
}

pub fn editor(_: &Theme, s: text_editor::Status) -> text_editor::Style {
    let focus = matches!(s, text_editor::Status::Focused { .. });
    text_editor::Style {
        background: WELL.into(),
        border: edged(10.0, if focus { mix(ACCENT, 0.8, INK) } else { EDGE_1 }),
        placeholder: INK_3,
        value: INK,
        selection: wash(ACCENT, 0.35),
    }
}
