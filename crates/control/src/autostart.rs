//! Start the control app with Windows: the HKCU Run value named by the spec's `autostart_value`.
//!
//! The registry value itself is the toggle's source of truth. It stores the absolute path of
//! the control executable, so a moved install needs the option switched off and on again:
//! [`Autostart::is_enabled`] is false when the stored command points somewhere else.

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::Mutex;

/// The Run key under HKCU.
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// A registry-like store of string values (the real Run key, or a stand-in for tests).
pub trait RunKey: Send + Sync {
    /// The value, or `None` when it does not exist.
    fn get(&self, name: &str) -> io::Result<Option<String>>;
    /// Sets the value.
    fn set(&self, name: &str, value: &str) -> io::Result<()>;
    /// Deletes the value (no error when it does not exist).
    fn delete(&self, name: &str) -> io::Result<()>;
}

/// An in-memory store (tests, and a UI-less run that must not touch the registry).
#[derive(Debug, Default)]
pub struct MemRunKey(Mutex<HashMap<String, String>>);

impl RunKey for MemRunKey {
    fn get(&self, name: &str) -> io::Result<Option<String>> {
        Ok(self.0.lock().map_or(None, |m| m.get(name).cloned()))
    }
    fn set(&self, name: &str, value: &str) -> io::Result<()> {
        if let Ok(mut m) = self.0.lock() {
            m.insert(name.into(), value.into());
        }
        Ok(())
    }
    fn delete(&self, name: &str) -> io::Result<()> {
        if let Ok(mut m) = self.0.lock() {
            m.remove(name);
        }
        Ok(())
    }
}

/// An HKCU key through the registry API. The default is the real Run key; a test uses a
/// private subkey.
#[cfg(windows)]
#[derive(Debug, Clone)]
pub struct WinRunKey {
    subkey: String,
}

#[cfg(windows)]
impl Default for WinRunKey {
    fn default() -> Self {
        Self::at(RUN_KEY)
    }
}

#[cfg(windows)]
impl WinRunKey {
    /// A store at `HKCU\<subkey>`.
    pub fn at(subkey: &str) -> Self {
        Self {
            subkey: subkey.into(),
        }
    }

    /// Removes the whole subkey (a test's cleanup).
    pub fn remove_subkey(&self) -> io::Result<()> {
        use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteKeyW};
        let sub = wide(&self.subkey);
        // SAFETY: NUL-terminated string.
        let r = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, sub.as_ptr()) };
        match r {
            0 | 2 => Ok(()),
            code => Err(io::Error::from_raw_os_error(code as i32)),
        }
    }
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
impl RunKey for WinRunKey {
    fn get(&self, name: &str) -> io::Result<Option<String>> {
        use windows_sys::Win32::System::Registry::{
            HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW,
        };
        let (sub, val) = (wide(&self.subkey), wide(name));
        let mut size = 0u32;
        // SAFETY: valid strings; a null data pointer asks for the size.
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                sub.as_ptr(),
                val.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        match r {
            0 => {}
            2 => return Ok(None), // not found
            code => return Err(io::Error::from_raw_os_error(code as i32)),
        }
        let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: the buffer is `size` bytes long.
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                sub.as_ptr(),
                val.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut size,
            )
        };
        match r {
            0 => {
                let chars = (size as usize / 2).saturating_sub(1);
                Ok(Some(String::from_utf16_lossy(&buf[..chars])))
            }
            2 => Ok(None),
            code => Err(io::Error::from_raw_os_error(code as i32)),
        }
    }

    fn set(&self, name: &str, value: &str) -> io::Result<()> {
        use windows_sys::Win32::System::Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
            RegCreateKeyExW, RegSetValueExW,
        };
        let (sub, val, data) = (wide(&self.subkey), wide(name), wide(value));
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: valid strings and out pointer.
        let r = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                sub.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        };
        if r != 0 {
            return Err(io::Error::from_raw_os_error(r as i32));
        }
        // SAFETY: `key` is open; `data` holds `len * 2` bytes including the NUL.
        let r = unsafe {
            RegSetValueExW(
                key,
                val.as_ptr(),
                0,
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        };
        // SAFETY: `key` is open.
        unsafe { RegCloseKey(key) };
        if r != 0 {
            return Err(io::Error::from_raw_os_error(r as i32));
        }
        Ok(())
    }

    fn delete(&self, name: &str) -> io::Result<()> {
        use windows_sys::Win32::System::Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, RegCloseKey, RegDeleteValueW, RegOpenKeyExW,
        };
        let (sub, val) = (wide(&self.subkey), wide(name));
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: valid strings and out pointer.
        let r =
            unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, KEY_SET_VALUE, &mut key) };
        match r {
            0 => {}
            2 => return Ok(()),
            code => return Err(io::Error::from_raw_os_error(code as i32)),
        }
        // SAFETY: `key` is open.
        let r = unsafe { RegDeleteValueW(key, val.as_ptr()) };
        // SAFETY: `key` is open.
        unsafe { RegCloseKey(key) };
        match r {
            0 | 2 => Ok(()),
            code => Err(io::Error::from_raw_os_error(code as i32)),
        }
    }
}

/// The Run value: the control app hidden in the tray (it starts the services set to start).
pub fn command(ui_exe: &Path) -> String {
    format!("\"{}\" --tray", ui_exe.display())
}

/// The "start with Windows" switch.
pub struct Autostart {
    value_name: &'static str,
    run_key: Box<dyn RunKey>,
    command: String,
}

impl Autostart {
    /// A switch writing `command(ui_exe)` into `run_key` as the value `value_name`.
    pub fn new(value_name: &'static str, run_key: Box<dyn RunKey>, ui_exe: &Path) -> Self {
        Self {
            value_name,
            run_key,
            command: command(ui_exe),
        }
    }

    /// The value written when enabled.
    pub fn command(&self) -> &str {
        &self.command
    }

    /// The entry exists and points at this exe.
    pub fn is_enabled(&self) -> io::Result<bool> {
        Ok(self
            .run_key
            .get(self.value_name)?
            .is_some_and(|v| v.trim().eq_ignore_ascii_case(&self.command)))
    }

    /// Writes the entry.
    pub fn enable(&self) -> io::Result<()> {
        self.run_key.set(self.value_name, &self.command)
    }

    /// Removes the entry.
    pub fn disable(&self) -> io::Result<()> {
        self.run_key.delete(self.value_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn the_entry_is_the_source_of_truth_and_a_moved_install_reads_as_off() {
        let key = std::sync::Arc::new(MemRunKey::default());
        struct Shared(std::sync::Arc<MemRunKey>);
        impl RunKey for Shared {
            fn get(&self, n: &str) -> io::Result<Option<String>> {
                self.0.get(n)
            }
            fn set(&self, n: &str, v: &str) -> io::Result<()> {
                self.0.set(n, v)
            }
            fn delete(&self, n: &str) -> io::Result<()> {
                self.0.delete(n)
            }
        }
        let exe = PathBuf::from(r"C:\Stintmate\example-connector-ui.exe");
        let a = Autostart::new("Example connector", Box::new(Shared(key.clone())), &exe);
        assert!(!a.is_enabled().unwrap());
        a.enable().unwrap();
        assert_eq!(
            key.get("Example connector").unwrap().unwrap(),
            r#""C:\Stintmate\example-connector-ui.exe" --tray"#
        );
        assert!(a.is_enabled().unwrap());
        let moved = Autostart::new(
            "Example connector",
            Box::new(Shared(key.clone())),
            Path::new(r"D:\other\example-connector-ui.exe"),
        );
        assert!(!moved.is_enabled().unwrap(), "points somewhere else");
        a.disable().unwrap();
        a.disable().unwrap();
        assert!(!a.is_enabled().unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn the_registry_store_round_trips_in_a_test_subkey() {
        let sub = format!(
            r"Software\Stintmate\Test\ExampleConnectorAutostart{}",
            std::process::id()
        );
        let key = WinRunKey::at(&sub);
        assert_eq!(key.get("v").unwrap(), None);
        key.delete("v").unwrap();
        key.set("v", r#""C:\a b\x.exe" --tray"#).unwrap();
        assert_eq!(
            key.get("v").unwrap().as_deref(),
            Some(r#""C:\a b\x.exe" --tray"#)
        );
        key.set("v", "short").unwrap();
        assert_eq!(key.get("v").unwrap().as_deref(), Some("short"));
        key.delete("v").unwrap();
        assert_eq!(key.get("v").unwrap(), None);
        key.remove_subkey().unwrap();
    }
}
