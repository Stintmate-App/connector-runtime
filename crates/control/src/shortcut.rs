//! `--from-shortcut`: what the Start menu shortcut does. The connector is meant to run under
//! Stintmate, so the shortcut never starts a connector of its own: it opens Stintmate when it is
//! installed, and otherwise says that Stintmate is needed and offers its website.
//!
//! Stintmate is found through the uninstall entry its Inno Setup installer writes
//! ([`uninstall_subkey`]); the entry also names the folder that holds the launcher to open. The
//! decision is made against the [`Env`] seam so it is tested with fakes.

use crate::texts::Texts;
use std::path::{Path, PathBuf};

/// Where to get Stintmate.
pub const STINTMATE_URL: &str = "https://stintmate.app";
/// The Inno Setup `AppId` of Stintmate's installer, which names its uninstall registry key
/// (`<AppId>_is1`).
pub const STINTMATE_APP_ID: &str = "{B5E7C0D2-4A1F-4E8B-9C36-5D2F7A91E4C8}";
/// Stintmate's launcher, relative to the `InstallLocation` of its uninstall entry.
pub const LAUNCHER_RELATIVE: &str = "hub\\.venv\\Scripts\\stintmate.exe";

/// The registry path (under `HKCU`, then `HKLM`) of Stintmate's uninstall entry.
pub fn uninstall_subkey() -> String {
    format!("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{STINTMATE_APP_ID}_is1")
}

/// What the shortcut does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Open Stintmate with this launcher; nothing else starts.
    OpenStintmate(PathBuf),
    /// Stintmate is not installed: say so and offer its website.
    OfferStintmate,
}

/// Everything the decision touches outside the process.
pub trait Env {
    /// The `InstallLocation` of Stintmate's uninstall entry, `None` when there is no entry.
    fn stintmate_install_location(&self) -> Option<PathBuf>;
    /// Whether the file exists.
    fn file_exists(&self, path: &Path) -> bool;
    /// Whether a connector of this machine already runs (under Stintmate or by hand).
    fn connector_running(&self) -> bool;
    /// Opens Stintmate; false when it could not be started.
    fn open_stintmate(&self, launcher: &Path) -> bool;
    /// The question with Yes and No: true for Yes.
    fn ask(&self, title: &str, text: &str) -> bool;
    /// Opens `url` in the default browser.
    fn open_url(&self, url: &str);
}

/// The launcher of the installed Stintmate: its uninstall entry names a folder that holds it.
/// An entry whose launcher is gone counts as not installed.
pub fn find_launcher(env: &dyn Env) -> Option<PathBuf> {
    let launcher = env.stintmate_install_location()?.join(LAUNCHER_RELATIVE);
    env.file_exists(&launcher).then_some(launcher)
}

/// The decision. A running connector changes nothing: the shortcut never starts one, so with
/// Stintmate installed it opens Stintmate (which shows itself when it already runs).
pub fn decide(env: &dyn Env) -> Decision {
    match find_launcher(env) {
        Some(l) => Decision::OpenStintmate(l),
        None => Decision::OfferStintmate,
    }
}

/// Runs `--from-shortcut`: the exit status (0 done, 1 Stintmate could not be started).
pub fn run(env: &dyn Env, texts: &Texts) -> i32 {
    match decide(env) {
        Decision::OpenStintmate(launcher) => {
            if env.connector_running() {
                tracing::info!("A connector already runs; opening Stintmate only");
            }
            if env.open_stintmate(&launcher) { 0 } else { 1 }
        }
        Decision::OfferStintmate => {
            if env.ask(
                &texts.s("app.name"),
                &texts.s("shortcut.needs_stintmate.text"),
            ) {
                env.open_url(STINTMATE_URL);
            }
            0
        }
    }
}

/// The real machine: the registry, the file system, the shell and a message box.
#[cfg(windows)]
pub struct StintmateProbe {
    /// The connector whose running is asked.
    pub spec: &'static stintmate_connector_platform::spec::ConnectorSpec,
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn install_location(root: windows_sys::Win32::System::Registry::HKEY) -> Option<PathBuf> {
    use windows_sys::Win32::System::Registry::{RRF_RT_REG_SZ, RegGetValueW};
    let (sub, val) = (wide(&uninstall_subkey()), wide("InstallLocation"));
    let mut size = 0u32;
    // SAFETY: valid strings; a null data pointer asks for the size.
    let r = unsafe {
        RegGetValueW(
            root,
            sub.as_ptr(),
            val.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if r != 0 || size < 2 {
        return None;
    }
    let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: the buffer is `size` bytes long.
    let r = unsafe {
        RegGetValueW(
            root,
            sub.as_ptr(),
            val.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if r != 0 {
        return None;
    }
    let text = String::from_utf16_lossy(&buf[..(size as usize / 2).saturating_sub(1)]);
    let text = text.trim();
    (!text.is_empty()).then(|| PathBuf::from(text))
}

#[cfg(windows)]
fn shell_open(target: &str, dir: Option<&Path>) -> bool {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let (verb, target) = (wide("open"), wide(target));
    let dir = dir.map(|d| wide(&d.display().to_string()));
    // SAFETY: NUL-terminated strings; the result is a status code above 32 on success.
    let r = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            dir.as_ref().map_or(std::ptr::null(), |d| d.as_ptr()),
            SW_SHOWNORMAL,
        )
    };
    r as usize > 32
}

#[cfg(windows)]
impl Env for StintmateProbe {
    fn stintmate_install_location(&self) -> Option<PathBuf> {
        use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
        install_location(HKEY_CURRENT_USER).or_else(|| install_location(HKEY_LOCAL_MACHINE))
    }

    fn file_exists(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn connector_running(&self) -> bool {
        stintmate_connector_platform::single::is_running(
            &stintmate_connector_platform::single::Names::for_spec(self.spec).ui,
        )
    }

    fn open_stintmate(&self, launcher: &Path) -> bool {
        // Stintmate's own shortcut starts it in its `hub` folder: four levels up from the exe.
        shell_open(&launcher.display().to_string(), launcher.ancestors().nth(4))
    }

    fn ask(&self, title: &str, text: &str) -> bool {
        crate::dialog::ask(title, text)
    }

    fn open_url(&self, url: &str) {
        shell_open(url, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Fake {
        location: Option<PathBuf>,
        file_there: bool,
        running: bool,
        answer_yes: bool,
        log: RefCell<Vec<String>>,
    }

    impl Env for Fake {
        fn stintmate_install_location(&self) -> Option<PathBuf> {
            self.location.clone()
        }
        fn file_exists(&self, _: &Path) -> bool {
            self.file_there
        }
        fn connector_running(&self) -> bool {
            self.running
        }
        fn open_stintmate(&self, launcher: &Path) -> bool {
            self.log
                .borrow_mut()
                .push(format!("stintmate {}", launcher.display()));
            true
        }
        fn ask(&self, _: &str, text: &str) -> bool {
            self.log.borrow_mut().push(format!("ask {text}"));
            self.answer_yes
        }
        fn open_url(&self, url: &str) {
            self.log.borrow_mut().push(format!("url {url}"));
        }
    }

    fn installed() -> Fake {
        Fake {
            location: Some(PathBuf::from("C:\\S")),
            file_there: true,
            ..Fake::default()
        }
    }

    fn launcher() -> PathBuf {
        PathBuf::from("C:\\S").join(LAUNCHER_RELATIVE)
    }

    #[test]
    fn installed_opens_stintmate_and_nothing_else() {
        let f = installed();
        assert_eq!(decide(&f), Decision::OpenStintmate(launcher()));
        assert_eq!(run(&f, &Texts::english(&crate::testspec::SPEC)), 0);
        assert_eq!(
            *f.log.borrow(),
            vec![format!("stintmate {}", launcher().display())]
        );
    }

    #[test]
    fn already_running_still_only_opens_stintmate() {
        let f = Fake {
            running: true,
            ..installed()
        };
        assert_eq!(run(&f, &Texts::english(&crate::testspec::SPEC)), 0);
        assert_eq!(f.log.borrow().len(), 1);
        assert!(f.log.borrow()[0].starts_with("stintmate "));
    }

    #[test]
    fn an_entry_whose_file_is_gone_is_not_installed() {
        let f = Fake {
            file_there: false,
            ..installed()
        };
        assert_eq!(decide(&f), Decision::OfferStintmate);
    }

    #[test]
    fn not_installed_asks_and_yes_opens_the_website() {
        let f = Fake {
            answer_yes: true,
            ..Fake::default()
        };
        assert_eq!(decide(&f), Decision::OfferStintmate);
        assert_eq!(run(&f, &Texts::english(&crate::testspec::SPEC)), 0);
        let log = f.log.borrow();
        assert_eq!(log.len(), 2);
        assert!(
            log[0].starts_with("ask ") && log[0].contains("Stintmate"),
            "{}",
            log[0]
        );
        assert_eq!(log[1], format!("url {STINTMATE_URL}"));
    }

    #[test]
    fn not_installed_and_no_opens_nothing() {
        let f = Fake::default();
        assert_eq!(run(&f, &Texts::english(&crate::testspec::SPEC)), 0);
        assert_eq!(f.log.borrow().len(), 1);
        assert!(f.log.borrow()[0].starts_with("ask "));
    }

    #[test]
    fn the_uninstall_key_is_the_installers_app_id() {
        assert!(
            uninstall_subkey().ends_with("\\Uninstall\\{B5E7C0D2-4A1F-4E8B-9C36-5D2F7A91E4C8}_is1")
        );
    }

    fn parse(args: &[&str]) -> Result<crate::cli::Args, String> {
        crate::cli::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn the_option_parses_alone_and_starts_nothing_else() {
        assert!(parse(&["--from-shortcut"]).unwrap().from_shortcut);
        assert!(parse(&["--from-shortcut", "--tray"]).is_err());
        assert!(parse(&["--from-shortcut", "--quit"]).is_err());
        assert!(parse(&["--from-shortcut", "--workspace", "x"]).is_err());
        assert!(!parse(&[]).unwrap().from_shortcut);
    }
}
