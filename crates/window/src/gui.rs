//! The window: Services, Options and Logs pages over the control app's snapshot, in the dark
//! look of the window ([`crate::theme`]); the scale follows the monitor's DPI and every text comes
//! from the catalog.

use crate::link::Link;
use crate::theme::{self, Icon};
use eframe::egui::{self, Align, Color32, Layout, Margin, RichText, Sense, Stroke, vec2};
use serde_json::Value;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use stintmate_connector_control::api::{Command, OptionsChange, Snapshot};
use stintmate_connector_control::config::{HZ_MAX, HZ_MIN, LOGS};
use stintmate_connector_control::logtail::{LevelFilter, LogTail, line_level};
use stintmate_connector_control::texts::Texts;
use stintmate_connector_i18n::Arg;

/// How many log lines "Copy last N" copies.
const COPY_LINES: usize = 50;
/// The window redraws at least this often while open (connection state, logs).
const TICK: Duration = Duration::from_secs(1);
/// Width of the side menu.
const NAV_WIDTH: f32 = 148.0;
/// Height of a log row (12 px at a 1.55 line height).
const LOG_ROW: f32 = 18.6;
/// Width reserved at the right of a settings row for its switch and the state in words.
const CONTROL_WIDTH: f32 = 150.0;

/// The pages, in the order of the navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    /// The services' switches and states.
    Services,
    /// The options.
    Options,
    /// The log viewer.
    Logs,
}

/// Two readings of the recorder's CPU time.
struct CpuSample {
    pid: u32,
    at: Instant,
    cpu: Duration,
    percent: Option<f64>,
}

/// The egui application.
pub struct Window {
    /// `None` in a preview (a render test): commands go nowhere.
    link: Option<Link>,
    texts: Texts,
    page: Page,
    /// The last `window_request` sequence acted on (`None` until the first snapshot).
    seen_request: Option<u64>,
    announced: bool,
    log: String,
    tail: Option<(PathBuf, LogTail)>,
    last_poll: Instant,
    alerts_only: bool,
    hz: Option<f64>,
    cpu: Option<CpuSample>,
    /// A fixed CPU figure (a preview has no process to measure).
    cpu_fixed: Option<f64>,
    exiting: bool,
    closing: bool,
}

impl Window {
    /// A window on `link`, opening on `page`.
    pub fn new(link: Link, texts: Texts, page: Page) -> Self {
        Self::build(Some(link), texts, page)
    }

    /// A window with no link, for rendering a snapshot offscreen: `cpu` is shown as the
    /// recorder's CPU use and `log` is the log it opens on.
    pub fn preview(texts: Texts, page: Page, log: &str, cpu: f64) -> Self {
        let mut w = Self::build(None, texts, page);
        w.log = log.to_string();
        w.cpu_fixed = Some(cpu);
        w
    }

    fn build(link: Option<Link>, texts: Texts, page: Page) -> Self {
        Self {
            link,
            texts,
            page,
            seen_request: None,
            announced: false,
            log: String::new(),
            tail: None,
            last_poll: Instant::now() - TICK,
            alerts_only: false,
            hz: None,
            cpu: None,
            cpu_fixed: None,
            exiting: false,
            closing: false,
        }
    }

    fn s(&self, key: &str) -> String {
        self.texts.s(key)
    }

    fn send(&self, command: Command) {
        if let Some(link) = &self.link {
            link.send(command);
        }
    }

    fn follow_requests(&mut self, ctx: &egui::Context, snap: &Snapshot) {
        let req = &snap.0["window_request"];
        let seq = req["seq"].as_u64().unwrap_or(0);
        match self.seen_request {
            None => {
                self.seen_request = Some(seq);
                if self.log.is_empty() {
                    self.log = snap.0["config"]["logs_default"]
                        .as_str()
                        .filter(|l| LOGS.contains(l))
                        .unwrap_or("recorder")
                        .to_string();
                }
            }
            Some(seen) if seq > seen => {
                self.seen_request = Some(seq);
                match req["action"].as_str() {
                    Some("logs") => self.page = Page::Logs,
                    Some("hide") => {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        return;
                    }
                    _ => {}
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            Some(_) => {}
        }
    }

    // ---- the side menu -------------------------------------------------------------------

    fn nav(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        let edge = ui.max_rect().right() + 10.0;
        let tall = ui.ctx().content_rect().y_range();
        ui.painter()
            .vline(edge, tall, Stroke::new(1.0, theme::LINE));
        ui.spacing_mut().item_spacing.y = 4.0;
        for (page, key, icon) in [
            (Page::Services, "page.services", Icon::Services),
            (Page::Options, "page.options", Icon::Options),
            (Page::Logs, "page.logs", Icon::Logs),
        ] {
            let label = self.s(key);
            if self.menu_item(ui, label, icon, self.page == page) {
                self.page = page;
            }
        }
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.add_space(2.0);
            ui.label(theme::note(
                self.texts.t(
                    "nav.version",
                    &[("version", self.texts.spec().version.into())],
                ),
                12.0,
                theme::FAINT,
            ));
            if !snap.managed() {
                let text = if self.exiting {
                    self.s("status.stopping")
                } else {
                    self.s("exit.button")
                };
                let button = ui
                    .add_enabled(!self.exiting, egui::Button::new(text))
                    .on_hover_text(self.s("exit.tip"));
                if button.clicked() {
                    self.exiting = true;
                    self.send(Command::Exit {
                        origin: "window".into(),
                    });
                }
            }
        });
    }

    fn menu_item(&self, ui: &mut egui::Ui, label: String, icon: Icon, selected: bool) -> bool {
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
        let painter = ui.painter();
        if selected || response.hovered() {
            let fill = if selected {
                theme::RAISED
            } else {
                theme::RAISED.gamma_multiply(0.55)
            };
            painter.rect_filled(rect, 8.0, fill);
        }
        let color = if selected { theme::TEXT } else { theme::MUTED };
        theme::paint_icon(
            painter,
            icon,
            egui::pos2(rect.left() + 18.0, rect.center().y),
            color,
            selected,
        );
        let font = if selected {
            egui::FontId::new(14.0, theme::semibold())
        } else {
            egui::FontId::new(14.0, egui::FontFamily::Proportional)
        };
        painter.text(
            egui::pos2(rect.left() + 36.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            font,
            color,
        );
        response.clicked()
    }

    // ---- Services ------------------------------------------------------------------------

    fn services(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        ui.label(theme::page_title(self.s("page.services")));
        ui.add_space(8.0);
        if snap.0["exiting"] == true {
            ui.label(theme::note(
                self.s("services.stopping"),
                13.0,
                theme::WARN_TEXT,
            ));
        }
        let services = snap.0["services"].as_array().cloned().unwrap_or_default();
        for svc in &services {
            self.service_card(ui, snap, svc);
            ui.add_space(6.0);
        }
        self.data_folder_card(ui, snap);
    }

    fn service_card(&mut self, ui: &mut egui::Ui, snap: &Snapshot, svc: &Value) {
        let id = svc["id"].as_str().unwrap_or_default();
        let name = svc["name"].as_str().unwrap_or(id);
        let running = svc["running"].as_bool().unwrap_or(false);
        let state = svc["state"].as_str().unwrap_or("off");
        let busy = snap.0["exiting"] == true || state == "stopping";
        let cpu = if running {
            self.cpu_percent(svc["pid"].as_u64().map(|p| p as u32))
        } else {
            None
        };
        let mut on = running;
        let label = self.pill_label(svc["label"].as_str().unwrap_or_default());
        let word = self.s(if on { "switch.on" } else { "switch.off" });
        theme::card(ui, Margin::symmetric(16, 14), |ui| {
            ui.spacing_mut().item_spacing.y = 10.0;
            ui.horizontal(|ui| {
                ui.set_min_height(26.0);
                ui.label(theme::strong(name, 16.0));
                let room = ui.available_width() - 130.0;
                theme::pill(ui, theme::pill_colors(state), &label, room);
                if theme::switch_with_state(ui, &mut on, !busy, &word) {
                    self.send(Command::Switch {
                        service: id.to_string(),
                        on,
                    });
                }
            });
            self.tiles(ui, snap, running, cpu);
            if running {
                let game = self.s(if snap.0["game_running"] == true {
                    "detail.game_running"
                } else {
                    "detail.game_not_running"
                });
                ui.label(theme::note(game, 13.0, theme::MUTED));
                for (line, color) in self.alert_lines(snap, svc) {
                    ui.label(theme::note(line, 12.0, color));
                }
            }
        });
    }

    /// The state text of the pill without the Pitwall link (its tile says it).
    fn pill_label(&self, label: &str) -> String {
        for key in [
            "status.pitwall.connected",
            "status.pitwall.absent",
            "status.pitwall.refused",
        ] {
            let suffix = format!(" · {}", self.s(key));
            if let Some(head) = label.strip_suffix(&suffix) {
                return head.to_string();
            }
        }
        label.to_string()
    }

    fn tiles(&self, ui: &mut egui::Ui, snap: &Snapshot, running: bool, cpu: Option<f64>) {
        let none = "—".to_string();
        let (pitwall, pitwall_color) = if running {
            match snap.0["pitwall"].as_str() {
                Some("connected") => (self.s("tile.pitwall.connected"), theme::GOOD),
                Some("refused") => (self.s("tile.pitwall.refused"), theme::ERROR_TEXT),
                _ => (self.s("tile.pitwall.absent"), theme::WARN_TEXT),
            }
        } else {
            (none.clone(), theme::MUTED)
        };
        let cpu = match (running, cpu) {
            (true, Some(c)) => format!("{c:.1} %"),
            _ => none.clone(),
        };
        let hz = snap.0["config"]["recorder_hz"].as_f64().unwrap_or(50.0);
        let rate = format!("{}{}", hz.round(), self.s("options.hz.suffix"));
        let gap = 8.0;
        let width = (ui.available_width() - 2.0 * gap) / 3.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            let mono = egui::FontFamily::Monospace;
            let tiles = [
                (
                    "tile.pitwall",
                    pitwall,
                    pitwall_color,
                    egui::FontFamily::Proportional,
                ),
                ("tile.cpu", cpu, theme::TEXT, mono.clone()),
                ("tile.rate", rate, theme::TEXT, mono),
            ];
            for (key, value, color, family) in tiles {
                egui::Frame::new()
                    .fill(theme::WELL)
                    .corner_radius(8)
                    .inner_margin(Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.set_width(width - 20.0);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            ui.label(theme::note(self.s(key), 12.0, theme::FAINT));
                            ui.label(
                                RichText::new(value)
                                    .family(family)
                                    .size(if key == "tile.pitwall" { 14.0 } else { 13.0 })
                                    .color(color),
                            );
                        });
                    });
            }
        });
    }

    /// The pending and rejected laps and the PID: what needs a look, small.
    fn alert_lines(&self, snap: &Snapshot, svc: &Value) -> Vec<(String, Color32)> {
        let mut out = Vec::new();
        let outbox = &snap.0["outbox"];
        if let Some(n) = outbox["pending"].as_i64().filter(|n| *n > 0) {
            out.push((
                self.texts.t("detail.outbox", &[("n", Arg::Int(n))]),
                theme::WARN_TEXT,
            ));
        }
        if let Some(n) = outbox["rejected"].as_i64().filter(|n| *n > 0) {
            out.push((
                self.texts.t("detail.rejected", &[("n", Arg::Int(n))]),
                theme::ERROR_TEXT,
            ));
        }
        if let Some(pid) = svc["pid"].as_u64() {
            out.push((
                self.texts
                    .t("detail.pid", &[("pid", pid.to_string().into())]),
                theme::FAINT,
            ));
        }
        out
    }

    /// The recorder's CPU use, in percent of one core, from two readings a second apart.
    fn cpu_percent(&mut self, pid: Option<u32>) -> Option<f64> {
        if self.cpu_fixed.is_some() {
            return self.cpu_fixed;
        }
        let pid = pid?;
        let cpu = stintmate_connector_platform::proc::process_cpu_time(pid)?;
        let now = Instant::now();
        match &mut self.cpu {
            Some(s) if s.pid == pid => {
                let dt = now.duration_since(s.at);
                if dt >= Duration::from_millis(900) {
                    let used = cpu.saturating_sub(s.cpu);
                    s.percent = Some(used.as_secs_f64() / dt.as_secs_f64() * 100.0);
                    s.at = now;
                    s.cpu = cpu;
                }
                s.percent
            }
            _ => {
                self.cpu = Some(CpuSample {
                    pid,
                    at: now,
                    cpu,
                    percent: None,
                });
                None
            }
        }
    }

    fn data_folder_card(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        let folder = snap.0["data_dir"].as_str().unwrap_or_default();
        let shown = shorten_path(folder, home_dir().as_deref());
        theme::card(ui, Margin::symmetric(16, 12), |ui| {
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 90.0).max(100.0);
                ui.allocate_ui_with_layout(vec2(w, 0.0), Layout::top_down(Align::Min), |ui| {
                    ui.set_width(w);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(theme::strong(self.s("services.data_folder"), 14.0));
                    ui.add(egui::Label::new(theme::note(&shown, 13.0, theme::FAINT)).truncate())
                        .on_hover_text(folder);
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button(self.s("services.open")).clicked() {
                        self.send(Command::OpenDataFolder);
                    }
                });
            });
        });
        if snap.managed() {
            ui.add_space(8.0);
            ui.label(theme::note(self.s("services.managed"), 12.0, theme::FAINT));
        }
    }

    // ---- Options -------------------------------------------------------------------------

    /// A settings row: title and note on the left, the controls on the right.
    fn setting_row(
        ui: &mut egui::Ui,
        title: RichText,
        note: &str,
        control_width: f32,
        controls: impl FnOnce(&mut egui::Ui),
    ) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let w = (ui.available_width() - control_width).max(120.0);
            let left =
                ui.allocate_ui_with_layout(vec2(w, 0.0), Layout::top_down(Align::Min), |ui| {
                    ui.set_width(w);
                    ui.spacing_mut().item_spacing.y = 3.0;
                    ui.label(title);
                    ui.label(theme::note(note, 13.0, theme::MUTED));
                });
            let h = left.response.rect.height().max(26.0);
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), h),
                Layout::right_to_left(Align::Center),
                controls,
            );
        });
        ui.add_space(8.0);
    }

    fn options(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        ui.label(theme::page_title(self.s("page.options")));
        ui.add_space(8.0);
        let cfg = &snap.0["config"];
        theme::card(ui, Margin::symmetric(16, 0), |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let mut first = true;
            let mut separate = |ui: &mut egui::Ui| {
                if !first {
                    ui.separator();
                }
                first = false;
            };
            if !snap.managed() {
                separate(ui);
                let mut on = snap.0["autostart"].as_bool().unwrap_or(false);
                let word = self.s(if on { "switch.on" } else { "switch.off" });
                let mut changed = false;
                Self::setting_row(
                    ui,
                    theme::strong(self.s("options.autostart"), 14.0),
                    &self.s("options.autostart.note"),
                    CONTROL_WIDTH,
                    |ui| changed = theme::switch_with_state(ui, &mut on, true, &word),
                );
                if changed {
                    self.send(Command::SetAutostart(on));
                }
            }
            for service in stintmate_connector_control::SERVICES {
                separate(ui);
                let mut on = cfg["start_with_app"][service]
                    .as_bool()
                    .unwrap_or(service == "recorder");
                let name = stintmate_connector_control::status::display_name(&self.texts, service);
                let word = self.s(if on { "switch.on" } else { "switch.off" });
                // Read-only here: it follows the switch on the Services page.
                Self::setting_row(
                    ui,
                    theme::strong(
                        self.texts.t("options.start", &[("service", name.into())]),
                        14.0,
                    ),
                    &self.s("options.start.note"),
                    CONTROL_WIDTH,
                    |ui| {
                        theme::switch_with_state(ui, &mut on, false, &word);
                    },
                );
            }
            separate(ui);
            let mut minimized = cfg["start_minimized"].as_bool().unwrap_or(true);
            let word = self.s(if minimized { "switch.on" } else { "switch.off" });
            let mut changed = false;
            Self::setting_row(
                ui,
                theme::strong(self.s("options.minimized"), 14.0),
                &self.s("options.minimized.note"),
                CONTROL_WIDTH,
                |ui| changed = theme::switch_with_state(ui, &mut minimized, true, &word),
            );
            if changed {
                self.send(Command::SetOptions(OptionsChange {
                    start_minimized: Some(minimized),
                    ..Default::default()
                }));
            }
        });
        ui.add_space(6.0);
        theme::card(ui, Margin::symmetric(16, 4), |ui| {
            let title = format!(
                "{} {}",
                self.s("options.hz.name"),
                self.s("options.hz.advanced")
            );
            let mut hz = self
                .hz
                .unwrap_or_else(|| cfg["recorder_hz"].as_f64().unwrap_or(50.0));
            let suffix = self.s("options.hz.suffix");
            let mut response = None;
            Self::setting_row(
                ui,
                theme::strong(title, 14.0),
                &self.s("options.hz.note"),
                110.0,
                |ui| {
                    let style = ui.style_mut();
                    style.override_font_id =
                        Some(egui::FontId::new(14.0, egui::FontFamily::Monospace));
                    style.visuals.widgets.inactive.weak_bg_fill = theme::WELL;
                    style.visuals.widgets.inactive.bg_fill = theme::WELL;
                    style.spacing.button_padding = vec2(10.0, 7.0);
                    response = Some(
                        ui.add(
                            egui::DragValue::new(&mut hz)
                                .range(HZ_MIN..=HZ_MAX)
                                .suffix(suffix),
                        ),
                    );
                },
            );
            if let Some(response) = response {
                if response.dragged() || response.has_focus() {
                    self.hz = Some(hz);
                }
                if response.drag_stopped() || response.lost_focus() {
                    self.hz = None;
                    self.send(Command::SetOptions(OptionsChange {
                        recorder_hz: Some(hz.round()),
                        ..Default::default()
                    }));
                }
            }
        });
    }

    // ---- Logs ----------------------------------------------------------------------------

    /// The two-way control Recorder | App, one painted widget at the right of the title.
    fn segmented(&mut self, ui: &mut egui::Ui) {
        let labels: Vec<(&str, String)> = LOGS
            .iter()
            .map(|name| {
                let key = if *name == "ui" {
                    "logs.ui"
                } else {
                    "service.recorder"
                };
                (*name, self.s(key))
            })
            .collect();
        let layout = |ui: &egui::Ui, text: &str, selected: bool| {
            let (family, color) = if selected {
                (theme::semibold(), theme::TEXT)
            } else {
                (egui::FontFamily::Proportional, theme::MUTED)
            };
            ui.painter()
                .layout_no_wrap(text.to_string(), egui::FontId::new(13.0, family), color)
        };
        let galleys: Vec<_> = labels
            .iter()
            .map(|(name, text)| layout(ui, text, self.log == *name))
            .collect();
        let widths: Vec<f32> = galleys.iter().map(|g| g.size().x + 24.0).collect();
        let total: f32 = widths.iter().sum::<f32>() + 6.0;
        let (rect, _) = ui.allocate_exact_size(vec2(total, 34.0), Sense::hover());
        ui.painter().rect(
            rect,
            9.0,
            theme::WELL,
            Stroke::new(1.0, theme::CARD_LINE),
            egui::StrokeKind::Inside,
        );
        let mut x = rect.left() + 3.0;
        for (((name, _), galley), width) in labels.iter().zip(galleys).zip(widths) {
            let seg = egui::Rect::from_min_size(egui::pos2(x, rect.top() + 3.0), vec2(width, 28.0));
            let response = ui.interact(
                seg,
                ui.make_persistent_id(("log-segment", *name)),
                Sense::click(),
            );
            if self.log == *name {
                ui.painter().rect_filled(seg, 7.0, theme::RAISED);
            }
            ui.painter()
                .galley(seg.center() - galley.size() / 2.0, galley, theme::TEXT);
            if response.clicked() && self.log != *name {
                self.log = name.to_string();
                self.tail = None;
            }
            x += width;
        }
    }

    fn logs(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        ui.spacing_mut().item_spacing.y = 10.0;
        ui.horizontal(|ui| {
            ui.set_min_height(34.0);
            ui.label(theme::page_title(self.s("page.logs")));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                self.segmented(ui);
            });
        });
        let dir = PathBuf::from(snap.0["data_dir"].as_str().unwrap_or_default());
        let file = format!("{}.log", self.log);
        let path = dir.join(&file);
        if self.tail.as_ref().is_none_or(|(p, _)| *p != path) {
            let mut tail = LogTail::new(&path);
            tail.open();
            self.tail = Some((path.clone(), tail));
        } else if self.last_poll.elapsed() >= TICK {
            if let Some((_, tail)) = &mut self.tail {
                tail.poll();
            }
            self.last_poll = Instant::now();
        }
        let Some((_, tail)) = &self.tail else { return };
        let mut filter = LevelFilter::default();
        let shown: Vec<&str> = if self.alerts_only {
            tail.lines
                .iter()
                .map(String::as_str)
                .filter(|l| filter.accept(l))
                .collect()
        } else {
            tail.lines.iter().map(String::as_str).collect()
        };
        let exists = tail.exists;
        let copy_label = self
            .texts
            .t("logs.copy", &[("n", Arg::Int(COPY_LINES as i64))]);
        let open_label = self.s("logs.open");
        let alerts_label = self.s("logs.alerts_only");
        ui.horizontal(|ui| {
            ui.set_min_height(32.0);
            ui.checkbox(
                &mut self.alerts_only,
                theme::note(alerts_label, 13.0, theme::TEXT_SOFT),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let open = ui.add(
                    egui::Button::new(RichText::new(open_label).color(theme::LINK))
                        .fill(Color32::TRANSPARENT)
                        .stroke(Stroke::NONE),
                );
                if open.clicked() {
                    let _ = std::process::Command::new("explorer.exe")
                        .arg(&path)
                        .spawn();
                }
                let copy = ui.add(egui::Button::new(copy_label).min_size(vec2(0.0, 32.0)));
                if copy.clicked() {
                    let from = shown.len().saturating_sub(COPY_LINES);
                    ui.ctx().copy_text(shown[from..].join("\n"));
                }
            });
        });
        let height = (ui.available_height() - 28.0).max(60.0);
        let rows = parse_rows(&shown);
        egui::Frame::new()
            .fill(theme::WELL)
            .stroke(Stroke::new(1.0, theme::LINE))
            .corner_radius(10)
            .inner_margin(Margin::symmetric(0, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if !exists {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        ui.label(theme::note(
                            self.texts.t(
                                "logs.none_yet",
                                &[("file", path.display().to_string().into())],
                            ),
                            12.0,
                            theme::FAINT,
                        ));
                    });
                }
                ui.spacing_mut().item_spacing.y = 0.0;
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(height - 16.0)
                    .stick_to_bottom(true)
                    .show_rows(ui, LOG_ROW, rows.len(), |ui, range| {
                        for row in &rows[range] {
                            log_row(ui, row);
                        }
                    });
            });
        ui.label(theme::note(
            self.texts.t("logs.footer", &[("file", file.into())]),
            12.0,
            theme::FAINT,
        ));
    }

    // ---- the frame -----------------------------------------------------------------------

    /// Draws the whole window over `snap`: the side menu and the current page.
    pub fn show(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        let ctx = ui.ctx().clone();
        self.follow_requests(&ctx, snap);
        egui::Panel::left("nav")
            .resizable(false)
            .exact_size(NAV_WIDTH)
            .frame(egui::Frame::new().fill(theme::NAV_BG).inner_margin(Margin {
                left: 10,
                right: 10,
                top: 14,
                bottom: 12,
            }))
            .show(ui, |ui| self.nav(ui, snap));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(Margin {
                left: 22,
                right: 22,
                top: 18,
                bottom: 14,
            }))
            .show(ui, |ui| {
                if let Some(link) = &self.link
                    && let Some(error) = link.take_error()
                {
                    ui.colored_label(theme::ERROR_TEXT, error);
                }
                match self.page {
                    Page::Logs => self.logs(ui, snap),
                    page => {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| match page {
                                Page::Services => self.services(ui, snap),
                                _ => self.options(ui, snap),
                            });
                    }
                }
            });
    }
}

/// One displayed log row: time, level, message and the full line.
struct Row<'a> {
    time: &'a str,
    level: &'static str,
    message: &'a str,
    severity: Severity,
    full: &'a str,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Severity {
    Normal,
    Warn,
    Error,
}

/// Splits `YYYY-MM-DD HH:MM:SS,mmm LEVEL target: message` into what the list shows (time of
/// day without milliseconds, level, message); a continuation line keeps the severity of the
/// entry above and shows its text indented.
fn parse_rows<'a>(lines: &[&'a str]) -> Vec<Row<'a>> {
    let mut out: Vec<Row<'a>> = Vec::with_capacity(lines.len());
    let mut last = Severity::Normal;
    for &line in lines {
        let Some(level) = line_level(line) else {
            out.push(Row {
                time: "",
                level: "",
                message: line.trim_end(),
                severity: last,
                full: line,
            });
            continue;
        };
        let mut parts = line.splitn(4, ' ');
        let time = parts.nth(1).unwrap_or_default();
        let time = time.split(',').next().unwrap_or(time);
        let rest = parts.nth(1).unwrap_or_default();
        let message = rest.split_once(": ").map_or(rest, |(_, m)| m);
        let (word, severity) = match level {
            "WARNING" => ("WARN", Severity::Warn),
            "ERROR" => ("ERROR", Severity::Error),
            "CRITICAL" => ("CRIT", Severity::Error),
            "DEBUG" => ("DEBUG", Severity::Normal),
            _ => ("INFO", Severity::Normal),
        };
        last = severity;
        out.push(Row {
            time,
            level: word,
            message,
            severity,
            full: line,
        });
    }
    out
}

fn log_row(ui: &mut egui::Ui, row: &Row<'_>) {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), LOG_ROW), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let (bg, level_color, text_color) = match row.severity {
        Severity::Normal => (None, theme::FAINT, theme::TEXT_SOFT),
        Severity::Warn => (Some(theme::WARN_BG), theme::WARN_DOT, theme::WARN_TEXT),
        Severity::Error => (Some(theme::ERROR_BG), theme::ERROR_DOT, theme::ERROR_TEXT),
    };
    let painter = ui.painter();
    if let Some(bg) = bg {
        painter.rect_filled(rect, 0.0, bg);
    }
    let font = egui::FontId::new(12.0, egui::FontFamily::Monospace);
    let y = rect.center().y;
    let x = rect.left() + 12.0;
    painter.text(
        egui::pos2(x, y),
        egui::Align2::LEFT_CENTER,
        row.time,
        font.clone(),
        theme::DIM,
    );
    painter.text(
        egui::pos2(x + 70.0, y),
        egui::Align2::LEFT_CENTER,
        row.level,
        font.clone(),
        level_color,
    );
    let clip = egui::Rect::from_min_max(egui::pos2(x + 122.0, rect.top()), rect.max);
    painter.with_clip_rect(clip).text(
        egui::pos2(x + 122.0, y),
        egui::Align2::LEFT_CENTER,
        row.message,
        font,
        text_color,
    );
    response.on_hover_text(RichText::new(row.full).monospace());
}

/// The data folder as shown: without the user's profile folder when it is inside it (so the
/// user name is not displayed), the whole path otherwise.
pub fn shorten_path(path: &str, home: Option<&str>) -> String {
    if let Some(home) = home.filter(|h| !h.is_empty()) {
        let (p, h) = (path.to_lowercase(), home.to_lowercase());
        let h = h.trim_end_matches(['\\', '/']);
        if p.len() > h.len() + 1 && p.starts_with(h) && matches!(&p[h.len()..=h.len()], "\\" | "/")
        {
            return path[h.len() + 1..].to_string();
        }
    }
    path.to_string()
}

fn home_dir() -> Option<String> {
    std::env::var("USERPROFILE").ok()
}

impl eframe::App for Window {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ctx.request_repaint_after(TICK);
        if ctx.input(|i| i.viewport().close_requested())
            && !self.closing
            && let Some(link) = &self.link
        {
            self.closing = true;
            if self.announced {
                let _ = link.send_and_wait(Command::WindowVisible(false), Duration::from_secs(1));
            }
            link.close();
        }
        let Some(link) = &self.link else { return };
        let Some(snap) = link.snapshot() else {
            ui.label(self.s("status.starting"));
            return;
        };
        if !self.announced {
            self.announced = true;
            link.send(Command::WindowVisible(true));
        }
        self.show(ui, &snap);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_in_the_profile_loses_the_user_name() {
        let home = Some(r"C:\Users\ana");
        assert_eq!(
            shorten_path(r"C:\Users\ana\Documents\Stintmate\connectors\example", home),
            r"Documents\Stintmate\connectors\example"
        );
        assert_eq!(
            shorten_path(r"c:\users\ANA\Documents", home),
            "Documents",
            "case-insensitive, like the file system"
        );
        assert_eq!(
            shorten_path(r"D:\Stintmate\example", home),
            r"D:\Stintmate\example"
        );
        assert_eq!(
            shorten_path(r"C:\Users\anabel\Documents", home),
            r"C:\Users\anabel\Documents",
            "a longer user name is another user"
        );
        assert_eq!(shorten_path(r"C:\x", None), r"C:\x");
    }

    #[test]
    fn log_lines_show_time_level_and_message_only() {
        let lines = [
            "2026-10-25 01:11:19,008 WARNING example_recorder::link: Connection lost: retrying",
            "    continuation of the warning",
            "2026-10-25 01:11:20,000 INFO example_recorder: Back",
            "2026-10-25 01:11:21,000 CRITICAL example_recorder: Gone",
        ];
        let rows = parse_rows(&lines);
        assert_eq!(rows[0].time, "01:11:19");
        assert_eq!(rows[0].level, "WARN");
        assert_eq!(rows[0].message, "Connection lost: retrying");
        assert!(rows[0].severity == Severity::Warn);
        assert_eq!(
            rows[0].full, lines[0],
            "the full line stays for the tooltip"
        );
        assert!(
            rows[1].severity == Severity::Warn,
            "a continuation keeps the entry's severity"
        );
        assert_eq!(rows[1].level, "");
        assert!(rows[2].severity == Severity::Normal);
        assert_eq!(rows[3].level, "CRIT");
        assert!(rows[3].severity == Severity::Error);
    }

    #[test]
    fn each_state_has_its_pill_colour() {
        use crate::theme::pill_colors;
        assert_eq!(pill_colors("waiting"), pill_colors("not_linked"));
        assert_ne!(pill_colors("recording"), pill_colors("waiting"));
        assert_ne!(pill_colors("crashed"), pill_colors("off"));
        assert_ne!(pill_colors("garage"), pill_colors("recording"));
        assert_eq!(pill_colors("off"), pill_colors("anything else"));
    }
}
