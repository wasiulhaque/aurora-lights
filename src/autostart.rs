//! "Restore at login": runs `<this executable> --restore` when the user logs in.
//!
//! Linux: an XDG autostart entry. Windows: a value under the current user's Run key.

pub use platform::{is_enabled, set};

/// Rewrite an enabled entry so it follows the executable if it was moved.
pub fn refresh() -> Result<(), String> {
    if is_enabled() { set(true) } else { Ok(()) }
}

fn current_exe() -> Result<std::path::PathBuf, String> {
    std::env::current_exe().map_err(|e| format!("cannot locate this program: {e}"))
}

#[cfg(target_os = "linux")]
mod platform {
    use std::fs;
    use std::io::ErrorKind;
    use std::path::PathBuf;

    fn entry_path() -> Option<PathBuf> {
        Some(
            crate::effects::config_home()?
                .join("autostart")
                .join("aurora-lights.desktop"),
        )
    }

    /// Quote an Exec argument per the Desktop Entry spec.
    fn quote_exec_arg(arg: &str) -> String {
        let mut quoted = String::from("\"");
        for c in arg.chars() {
            match c {
                '"' | '`' | '$' => quoted.push_str(&format!("\\{c}")),
                // Escaped once for the quoting rule and once for the string type.
                '\\' => quoted.push_str("\\\\\\\\"),
                _ => quoted.push(c),
            }
        }
        quoted.push('"');
        quoted
    }

    pub fn is_enabled() -> bool {
        entry_path().is_some_and(|path| path.exists())
    }

    pub fn set(enabled: bool) -> Result<(), String> {
        let path = entry_path().ok_or("cannot locate the config directory")?;
        if !enabled {
            return match fs::remove_file(&path) {
                Err(e) if e.kind() != ErrorKind::NotFound => {
                    Err(format!("remove {}: {e}", path.display()))
                }
                _ => Ok(()),
            };
        }

        let exe = super::current_exe()?;
        let entry = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Aurora Lights (restore)\n\
             Comment=Reapply the last Aurora Lights setting at login\n\
             Exec={} --restore\n\
             NoDisplay=true\n\
             X-GNOME-Autostart-enabled=true\n\
             X-GNOME-Autostart-Delay=5\n",
            quote_exec_arg(&exe.to_string_lossy())
        );
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        }
        fs::write(&path, entry).map_err(|e| format!("write {}: {e}", path.display()))
    }
}

#[cfg(windows)]
mod platform {
    use std::ptr::null_mut;

    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
    };

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "Aurora Lights";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn os_error(context: &str, code: u32) -> String {
        format!(
            "{context}: {}",
            std::io::Error::from_raw_os_error(code as i32)
        )
    }

    pub fn is_enabled() -> bool {
        let (key, name) = (wide(RUN_KEY), wide(VALUE_NAME));
        // SAFETY: NUL-terminated strings; null data pointers only query existence.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                null_mut(),
                null_mut(),
                null_mut(),
            )
        };
        status == ERROR_SUCCESS
    }

    pub fn set(enabled: bool) -> Result<(), String> {
        let (key, name) = (wide(RUN_KEY), wide(VALUE_NAME));
        if !enabled {
            // SAFETY: NUL-terminated strings.
            let status =
                unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) };
            return match status {
                ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
                code => Err(os_error("remove login entry", code)),
            };
        }

        let exe = super::current_exe()?;
        let command = wide(&format!("\"{}\" --restore", exe.display()));
        // SAFETY: `command` is a NUL-terminated UTF-16 string of the given byte length.
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                command.as_ptr().cast(),
                (command.len() * 2) as u32,
            )
        };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(os_error("add login entry", status))
        }
    }
}
