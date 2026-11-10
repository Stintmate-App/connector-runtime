//! The tray: the control app's only UI in its own process. `tray-icon` and its
//! menu on a plain Win32 message loop: no GPU, no egui. The window is another process
//! ([`crate::window_host`]).
//!
//! One thread forwards the control app's [`UiEvent`]s into a queue and posts a thread message
//! to wake the loop; menu clicks arrive through the menu crate's channel, drained after each
//! message. A 1 s thread timer notices when the window process has ended.

use crate::api::{Command, Notice, Snapshot, UiEvent};
use crate::app::ControlApp;
use crate::texts::Texts;
use crate::tray_model::{self, Entry, TrayItem};
use crate::window_host::{Outcome, WindowHost};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use stintmate_connector_platform::spec::ConnectorSpec;
use stintmate_connector_resources::{TRAY_SIZE, render_rgba};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Shell::{
    NIF_INFO, NIIF_ERROR, NIIF_INFO, NIIF_WARNING, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW,
    ShellExecuteW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, FindWindowW, GetMessageW, KillTimer, MSG, PostThreadMessageW, SW_SHOWNORMAL,
    SetTimer, TranslateMessage, WM_APP, WM_TIMER,
};

/// The thread message that wakes the loop for queued control app events.
const WM_EVENTS: u32 = WM_APP + 1;
/// The tray-icon crate numbers its icons from 1; this process has exactly one.
const TRAY_UID: u32 = 1;
const POLL_MS: u32 = 1000;

/// What [`run`] needs.
pub struct TrayRun {
    /// The control app.
    pub app: Arc<ControlApp>,
    /// Its events (taken before `serve`).
    pub events: Receiver<UiEvent>,
    /// The texts.
    pub texts: Texts,
    /// The window process.
    pub host: Arc<WindowHost>,
    /// Open the window at start.
    pub show_at_start: bool,
}

/// Whether the shell has a notification area.
pub fn tray_available() -> bool {
    let class: Vec<u16> = "Shell_TrayWnd\0".encode_utf16().collect();
    // SAFETY: NUL-terminated class name; a null title matches any.
    unsafe { !FindWindowW(class.as_ptr(), std::ptr::null()).is_null() }
}

fn icon_for(spec: &ConnectorSpec, level: &str) -> Icon {
    let rgba = match spec.icon {
        Some(art) => art.badged_rgba(level, TRAY_SIZE).unwrap_or_else(|e| {
            tracing::warn!("{e}; the tray draws the glyph instead");
            render_rgba(spec.glyph, level, TRAY_SIZE)
        }),
        None => render_rgba(spec.glyph, level, TRAY_SIZE),
    };
    Icon::from_rgba(rgba, TRAY_SIZE, TRAY_SIZE).expect("a square RGBA icon")
}

struct Menus {
    items: Vec<(TrayItem, MenuItem)>,
}

fn build_menu(entries: &[Entry]) -> (Menu, Menus) {
    let menu = Menu::new();
    let mut items = Vec::new();
    for e in entries {
        match e {
            Entry::Separator => {
                let _ = menu.append(&PredefinedMenuItem::separator());
            }
            Entry::Item {
                item,
                label,
                enabled,
            } => {
                let mi = MenuItem::with_id(MenuId::new(item.id()), label, *enabled, None);
                let _ = menu.append(&mi);
                items.push((item.clone(), mi));
            }
        }
    }
    (menu, Menus { items })
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn copy_wide<const N: usize>(dst: &mut [u16; N], s: &str) {
    for (d, c) in dst.iter_mut().zip(s.encode_utf16().take(N - 1)) {
        *d = c;
    }
}

/// A tray balloon for a notice.
fn balloon(hwnd: HWND, n: &Notice) {
    // SAFETY: a zeroed NOTIFYICONDATAW is valid; the strings are copied and NUL-terminated.
    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = TRAY_UID;
        nid.uFlags = NIF_INFO;
        nid.dwInfoFlags = match n.level.as_str() {
            "error" => NIIF_ERROR,
            "warn" => NIIF_WARNING,
            _ => NIIF_INFO,
        };
        copy_wide(&mut nid.szInfoTitle, &n.title);
        copy_wide(&mut nid.szInfo, &n.message);
        if Shell_NotifyIconW(NIM_MODIFY, &nid) == 0 {
            tracing::warn!("The tray balloon was not shown");
        }
    }
}

/// Opens a folder in the file manager.
fn open_folder(path: &Path) {
    let (verb, target) = (wide("open"), wide(&path.display().to_string()));
    // SAFETY: NUL-terminated strings; the result is only a status code.
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}

struct Ui {
    app: Arc<ControlApp>,
    texts: Texts,
    host: Arc<WindowHost>,
    tray: Option<TrayIcon>,
    menus: Menus,
    level: String,
    tooltip: String,
    last: Option<Snapshot>,
}

impl Ui {
    fn show(&self, page: Option<&str>) {
        match self.host.ensure(page) {
            Ok(Outcome::Started(pid)) => tracing::info!("Window process started (pid {pid})"),
            Ok(Outcome::Running(pid)) => tracing::info!("Window process {pid} brought up"),
            Err(e) => tracing::error!("Cannot start the window process: {e}"),
        }
    }

    fn refresh(&mut self) {
        let snap = self.app.latest_snapshot();
        if self.last.as_ref() == Some(&snap) {
            return;
        }
        self.last = Some(snap.clone());
        if let Some(tray) = &self.tray {
            if snap.level() != self.level {
                self.level = snap.level().to_string();
                let _ = tray.set_icon(Some(icon_for(self.texts.spec(), &self.level)));
            }
            let tip = tray_model::tooltip(&snap, &self.texts);
            if tip != self.tooltip {
                let _ = tray.set_tooltip(Some(&tip));
                self.tooltip = tip;
            }
        }
        for entry in tray_model::menu(&snap, &self.texts) {
            if let Entry::Item {
                item,
                label,
                enabled,
            } = entry
                && let Some((_, mi)) = self.menus.items.iter().find(|(i, _)| *i == item)
            {
                if mi.text() != label {
                    mi.set_text(label);
                }
                if mi.is_enabled() != enabled {
                    mi.set_enabled(enabled);
                }
            }
        }
    }

    fn handle(&mut self, ev: UiEvent) -> bool {
        match ev {
            UiEvent::ShowWindow => self.show(None),
            UiEvent::ShowLogs => self.show(Some("logs")),
            UiEvent::HideWindow => {}
            UiEvent::OpenFolder(p) => open_folder(&p),
            UiEvent::Notice(n) => {
                if let Some(t) = &self.tray {
                    balloon(t.window_handle(), &n);
                }
            }
            UiEvent::ManagedChanged(managed) => {
                if let Some(t) = &self.tray {
                    let _ = t.set_visible(!managed);
                }
            }
            UiEvent::Changed => self.refresh(),
            UiEvent::Quit => return false,
        }
        true
    }

    fn click(&self, item: &TrayItem) {
        let snap: Snapshot = self.app.latest_snapshot();
        let cmd: Command = tray_model::command(item, &snap);
        if let Err(e) = self.app.dispatch(cmd) {
            tracing::warn!("Tray command refused: {e}");
        }
    }
}

/// Runs the tray on this thread until the control app quits.
pub fn run(run: TrayRun) {
    let TrayRun {
        app,
        events,
        texts,
        host,
        show_at_start,
    } = run;
    let snap = app.snapshot();
    let (menu, menus) = build_menu(&tray_model::menu(&snap, &texts));
    let tooltip = tray_model::tooltip(&snap, &texts);
    let built = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip(&tooltip)
        .with_icon(icon_for(texts.spec(), snap.level()))
        .build();
    let tray = match built {
        Ok(t) => Some(t),
        Err(e) => {
            tracing::error!("No tray icon ({e}); the window opens instead");
            None
        }
    };
    if let (Some(t), true) = (&tray, snap.managed()) {
        let _ = t.set_visible(false);
    }
    let tray_ok = tray.is_some();
    let mut ui = Ui {
        app: app.clone(),
        texts,
        host,
        tray,
        menus,
        level: snap.level().to_string(),
        tooltip,
        last: None,
    };
    let queue: Arc<Mutex<VecDeque<UiEvent>>> = Arc::default();
    // SAFETY: no arguments.
    let main_tid = unsafe { GetCurrentThreadId() };
    let q = queue.clone();
    std::thread::Builder::new()
        .name("tray-events".into())
        .spawn(move || {
            for ev in events {
                let quit = ev == UiEvent::Quit;
                q.lock().unwrap_or_else(|e| e.into_inner()).push_back(ev);
                // SAFETY: posts a thread message to a live thread id.
                unsafe { PostThreadMessageW(main_tid, WM_EVENTS, 0, 0) };
                if quit {
                    break;
                }
            }
        })
        .expect("a thread starts");
    // SAFETY: a thread timer (no window): WM_TIMER comes to this thread's queue.
    let timer = unsafe { SetTimer(std::ptr::null_mut(), 0, POLL_MS, None) };
    if (show_at_start || !tray_ok) && !snap.managed() {
        let _ = app.dispatch(Command::ShowWindow);
    }
    let mut alive = true;
    // SAFETY: a zeroed MSG is valid; the standard message loop of this thread.
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while alive && GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if msg.hwnd.is_null() && msg.message == WM_EVENTS {
                let drained: Vec<UiEvent> = queue
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .drain(..)
                    .collect();
                for ev in drained {
                    alive &= ui.handle(ev);
                }
            } else if msg.hwnd.is_null() && msg.message == WM_TIMER {
                if ui.host.poll_exit() {
                    let _ = app.dispatch(Command::WindowVisible(false));
                    if !tray_ok {
                        app.exit("window closed (no tray)");
                    }
                }
            } else {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            while let Ok(ev) = MenuEvent::receiver().try_recv() {
                if let Some(item) = TrayItem::from_id(&ev.id.0) {
                    ui.click(&item);
                }
            }
            while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
                if let TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                } = ev
                {
                    ui.click(&TrayItem::Open);
                }
            }
        }
        KillTimer(std::ptr::null_mut(), timer);
    }
    ui.host.close();
    drop(ui);
}

#[cfg(test)]
mod tests {
    use super::*;
    use stintmate_connector_resources::ConnectorIcon;
    use stintmate_connector_testing::EXAMPLE_SPEC;

    #[test]
    fn a_spec_without_artwork_or_with_a_bad_one_gets_the_glyph_icon() {
        // Icon has no accessor for its pixels; building it must simply not panic.
        let _ = icon_for(&EXAMPLE_SPEC, "ok");
        let bad = ConnectorSpec {
            icon: Some(ConnectorIcon {
                png_32: b"nope",
                png_64: b"",
            }),
            ..EXAMPLE_SPEC
        };
        let _ = icon_for(&bad, "warn");
    }
}
