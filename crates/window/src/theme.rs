//! The window's look: the dark palette of the approved mockups, the bundled fonts
//! (Barlow for the interface, Barlow Condensed for the page titles, JetBrains Mono for the
//! logs and figures; all SIL OFL 1.1, see `assets/fonts`) and the few painted widgets egui
//! lacks (switch, status pill, side menu icons). Nothing here touches the GPU beyond egui's own
//! painting.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Response, RichText,
    Sense, Stroke, TextStyle, Ui, vec2,
};
use std::sync::Arc;

/// Window background.
pub const BG: Color32 = Color32::from_rgb(0x0b, 0x0d, 0x10);
/// Side menu background.
pub const NAV_BG: Color32 = Color32::from_rgb(0x0f, 0x12, 0x16);
/// Card background.
pub const CARD: Color32 = Color32::from_rgb(0x14, 0x17, 0x1c);
/// Card border.
pub const CARD_LINE: Color32 = Color32::from_rgb(0x26, 0x2a, 0x31);
/// Hairlines inside cards and between panes.
pub const LINE: Color32 = Color32::from_rgb(0x1f, 0x23, 0x29);
/// Buttons, the selected menu item and the selected segment.
pub const RAISED: Color32 = Color32::from_rgb(0x1d, 0x21, 0x28);
/// Button and input borders.
pub const BORDER: Color32 = Color32::from_rgb(0x35, 0x3a, 0x43);
/// Tiles, inputs and the log list.
pub const WELL: Color32 = Color32::from_rgb(0x0f, 0x12, 0x16);
/// Primary text.
pub const TEXT: Color32 = Color32::from_rgb(0xe8, 0xea, 0xed);
/// Text of controls.
pub const TEXT_SOFT: Color32 = Color32::from_rgb(0xc9, 0xce, 0xd6);
/// Secondary text and notes.
pub const MUTED: Color32 = Color32::from_rgb(0xa3, 0xa9, 0xb3);
/// Captions and the version.
pub const FAINT: Color32 = Color32::from_rgb(0x8b, 0x92, 0x9c);
/// Log timestamps.
pub const DIM: Color32 = Color32::from_rgb(0x6b, 0x72, 0x80);
/// Accent: the switch when on.
pub const ACCENT: Color32 = Color32::from_rgb(0xa8, 0x55, 0xf7);
/// Accent for text links.
pub const LINK: Color32 = Color32::from_rgb(0xc4, 0xb5, 0xfd);
/// A good state (Pitwall connected).
pub const GOOD: Color32 = Color32::from_rgb(0x86, 0xef, 0xac);
/// Amber text (warnings).
pub const WARN_TEXT: Color32 = Color32::from_rgb(0xfd, 0xe6, 0x8a);
/// Amber dot and level.
pub const WARN_DOT: Color32 = Color32::from_rgb(0xfa, 0xcc, 0x15);
/// Amber background.
pub const WARN_BG: Color32 = Color32::from_rgb(0x2a, 0x24, 0x10);
/// Error text.
pub const ERROR_TEXT: Color32 = Color32::from_rgb(0xfc, 0xa5, 0xa5);
/// Error dot.
pub const ERROR_DOT: Color32 = Color32::from_rgb(0xef, 0x44, 0x44);
/// Error background.
pub const ERROR_BG: Color32 = Color32::from_rgb(0x2a, 0x12, 0x15);

/// The family of the semibold weight (names, buttons).
pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

/// The family of the page titles.
pub fn title() -> FontFamily {
    FontFamily::Name("title".into())
}

/// The colours of a status pill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PillColors {
    /// Background.
    pub bg: Color32,
    /// Dot.
    pub dot: Color32,
    /// Text.
    pub text: Color32,
}

/// The pill of a recorder state: waiting and degraded amber, recording green, connected or
/// starting blue, stopped grey, failed red.
pub fn pill_colors(state: &str) -> PillColors {
    let c = |bg: [u8; 3], dot: [u8; 3], text: [u8; 3]| PillColors {
        bg: Color32::from_rgb(bg[0], bg[1], bg[2]),
        dot: Color32::from_rgb(dot[0], dot[1], dot[2]),
        text: Color32::from_rgb(text[0], text[1], text[2]),
    };
    match state {
        "waiting" | "not_linked" | "shm_error" => PillColors {
            bg: WARN_BG,
            dot: WARN_DOT,
            text: WARN_TEXT,
        },
        "recording" => c([0x0f, 0x2a, 0x1a], [0x22, 0xc5, 0x5e], [0x86, 0xef, 0xac]),
        "garage" | "connected" | "starting" | "stopping" => {
            c([0x0f, 0x1f, 0x33], [0x3b, 0x82, 0xf6], [0x93, 0xc5, 0xfd])
        }
        "crashed" | "not_installed" => PillColors {
            bg: ERROR_BG,
            dot: ERROR_DOT,
            text: ERROR_TEXT,
        },
        _ => c([0x1d, 0x21, 0x28], [0x6b, 0x72, 0x80], [0xa3, 0xa9, 0xb3]),
    }
}

fn font(name: &str, bytes: &'static [u8]) -> (String, Arc<FontData>) {
    (name.to_string(), Arc::new(FontData::from_static(bytes)))
}

/// Installs the fonts, the dark visuals and the text sizes.
pub fn apply(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    for (name, data) in [
        font(
            "barlow",
            include_bytes!("../assets/fonts/Barlow-Regular.ttf"),
        ),
        font(
            "barlow-semibold",
            include_bytes!("../assets/fonts/Barlow-SemiBold.ttf"),
        ),
        font(
            "barlow-condensed",
            include_bytes!("../assets/fonts/BarlowCondensed-ExtraBold.ttf"),
        ),
        font(
            "jetbrains-mono",
            include_bytes!("../assets/fonts/JetBrainsMono-Variable.ttf"),
        ),
    ] {
        fonts.font_data.insert(name, data);
    }
    // egui's own fonts stay behind ours as the fallback for any glyph they lack.
    let fallback = |family: &FontFamily, fonts: &FontDefinitions| {
        fonts.families.get(family).cloned().unwrap_or_default()
    };
    let with_first = |first: &str, family: FontFamily, fonts: &mut FontDefinitions| {
        let mut list = vec![first.to_string()];
        list.extend(fallback(&family, fonts));
        fonts.families.insert(family, list);
    };
    with_first("barlow", FontFamily::Proportional, &mut fonts);
    with_first("jetbrains-mono", FontFamily::Monospace, &mut fonts);
    let prop = fallback(&FontFamily::Proportional, &fonts);
    for (family, first) in [
        (semibold(), "barlow-semibold"),
        (title(), "barlow-condensed"),
    ] {
        let mut list = vec![first.to_string()];
        list.extend(prop.iter().filter(|f| *f != "barlow").cloned());
        fonts.families.insert(family, list);
    }
    ctx.set_fonts(fonts);

    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (
                TextStyle::Small,
                FontId::new(12.0, FontFamily::Proportional),
            ),
            (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(13.0, semibold())),
            (TextStyle::Heading, FontId::new(26.0, title())),
            (
                TextStyle::Monospace,
                FontId::new(12.0, FontFamily::Monospace),
            ),
        ]
        .into();
        style.spacing.item_spacing = vec2(8.0, 6.0);
        style.spacing.button_padding = vec2(14.0, 8.0);
        style.spacing.interact_size.y = 20.0;
        let v = &mut style.visuals;
        v.dark_mode = true;
        v.override_text_color = None;
        v.panel_fill = BG;
        v.window_fill = CARD;
        v.extreme_bg_color = WELL;
        v.faint_bg_color = CARD;
        v.hyperlink_color = LINK;
        v.selection.bg_fill = ACCENT.gamma_multiply(0.45);
        v.selection.stroke = Stroke::new(1.0, TEXT);
        let round = CornerRadius::same(9);
        for (w, fill) in [
            (&mut v.widgets.inactive, RAISED),
            (&mut v.widgets.hovered, Color32::from_rgb(0x26, 0x2b, 0x33)),
            (&mut v.widgets.active, Color32::from_rgb(0x2d, 0x33, 0x3c)),
        ] {
            w.bg_fill = fill;
            w.weak_bg_fill = fill;
            w.bg_stroke = Stroke::new(1.0, BORDER);
            w.fg_stroke = Stroke::new(1.0, TEXT);
            w.corner_radius = round;
        }
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_SOFT);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
        v.widgets.noninteractive.corner_radius = round;
        v.widgets.open.corner_radius = round;
    });
}

/// A text with the semibold weight.
pub fn strong(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text)
        .family(semibold())
        .size(size)
        .color(TEXT)
}

/// A secondary text.
pub fn note(text: impl Into<String>, size: f32, color: Color32) -> RichText {
    RichText::new(text).size(size).color(color)
}

/// A page title.
pub fn page_title(text: impl Into<String>) -> RichText {
    RichText::new(text).family(title()).size(26.0).color(TEXT)
}

/// The switch: 44x24, accent when on. Returns the response; `changed()` is set on a click.
pub fn switch(ui: &mut Ui, on: &mut bool, enabled: bool) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, mut response) = ui.allocate_exact_size(vec2(44.0, 24.0), sense);
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let dim = |c: Color32| if enabled { c } else { c.gamma_multiply(0.55) };
    let (track, knob) = if *on {
        (ACCENT, Color32::WHITE)
    } else {
        (BORDER, TEXT_SOFT)
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(12), dim(track));
    let x = if *on {
        rect.right() - 12.0
    } else {
        rect.left() + 12.0
    };
    painter.circle_filled(egui::pos2(x, rect.center().y), 9.0, dim(knob));
    if enabled {
        response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    response
}

/// A switch with its state in words ("On" / "Off") to its left, right-aligned in the current
/// layout. Returns true when it was toggled.
pub fn switch_with_state(ui: &mut Ui, on: &mut bool, enabled: bool, word: &str) -> bool {
    let mut changed = false;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        changed = switch(ui, on, enabled).changed();
        ui.label(note(word, 13.0, MUTED));
    });
    changed
}

/// A card: the rounded panel of the mockups.
pub fn card<R>(
    ui: &mut Ui,
    margin: egui::Margin,
    add: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<R> {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, CARD_LINE))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(margin)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
}

/// A status pill (dot and text), the text cut with an ellipsis if it does not fit
/// `max_width`; the full text on hover.
pub fn pill(ui: &mut Ui, colors: PillColors, text: &str, max_width: f32) -> Response {
    let pad = vec2(10.0, 3.0);
    let mut job = egui::text::LayoutJob::simple_singleline(
        text.to_string(),
        FontId::new(13.0, FontFamily::Proportional),
        colors.text,
    );
    job.wrap = egui::text::TextWrapping {
        max_width: (max_width - 2.0 * pad.x - 14.0).max(40.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let size = vec2(
        galley.size().x + 2.0 * pad.x + 14.0,
        galley.size().y + 2.0 * pad.y,
    );
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(255), colors.bg);
    painter.circle_filled(
        egui::pos2(rect.left() + pad.x + 4.0, rect.center().y),
        4.0,
        colors.dot,
    );
    painter.galley(
        egui::pos2(rect.left() + pad.x + 14.0, rect.top() + pad.y),
        galley,
        colors.text,
    );
    response.on_hover_text(text)
}

/// Which icon a menu item draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// Two rings (services).
    Services,
    /// Sliders (options).
    Options,
    /// Three lines (logs).
    Logs,
}

/// Paints a 16 px icon centred on `c`; `filled` is the selected item.
pub fn paint_icon(
    painter: &egui::Painter,
    icon: Icon,
    c: egui::Pos2,
    color: Color32,
    filled: bool,
) {
    let s = 16.0 / 24.0;
    let p = |x: f32, y: f32| egui::pos2(c.x + (x - 12.0) * s, c.y + (y - 12.0) * s);
    let stroke = Stroke::new(1.6, color);
    match icon {
        Icon::Services => {
            if filled {
                painter.circle_filled(c, 8.0 * s, color.gamma_multiply(0.35));
            }
            painter.circle_stroke(c, 8.0 * s, stroke);
            painter.circle_filled(c, 3.0 * s + 0.6, color);
        }
        Icon::Options => {
            for (y, x0, x1, knob) in [(7.0, 4.0, 20.0, 16.0), (17.0, 4.0, 20.0, 10.0)] {
                painter.line_segment([p(x0, y), p(x1, y)], stroke);
                let k = p(knob, y);
                painter.circle_filled(k, 2.6 * s * 1.5, if filled { color } else { NAV_BG });
                painter.circle_stroke(k, 2.6 * s * 1.5, stroke);
            }
        }
        Icon::Logs => {
            for (y, x1) in [(6.0, 19.0), (12.0, 19.0), (18.0, 14.0)] {
                painter.line_segment([p(5.0, y), p(x1, y)], stroke);
            }
        }
    }
}
