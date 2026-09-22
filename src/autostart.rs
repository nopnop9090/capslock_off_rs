//! Autostart via HKCU Run-Key.
//!
//! Setzt einen Eintrag in
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\capslock_off`
//! mit Inhalt: `"<exe-pfad>" --autostart`.
//!
//! Kein Admin noetig (HKCU = Current User).
//!
//! Beim Logon startet Windows automatisch die Binary. Die Binary erkennt
//! `--autostart` und unterdrueckt das Konsolenfenster.
use std::path::PathBuf;

use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ, REG_VALUE_TYPE,
};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "capslock_off";

/// Liefert den Pfad zur aktuellen Exe.
fn current_exe() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

/// Erwarteter Registry-Wert.
fn expected_value() -> Option<String> {
    let exe = current_exe()?;
    let exe_str = exe.to_string_lossy().into_owned();
    // Quotes um den Exe-Pfad, weil er Backslashes enthaelt (siehe Memory).
    Some(format!("\"{}\" --autostart", exe_str))
}

/// Prueft, ob der HKCU Run-Key den erwarteten Wert hat.
pub fn is_enabled() -> bool {
    let Some(expected) = expected_value() else {
        return false;
    };
    let Some(current) = read_run_value() else {
        return false;
    };
    current == expected
}

fn read_run_value() -> Option<String> {
    unsafe {
        let mut hkey: HKEY = HKEY(std::ptr::null_mut());
        let result = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            windows::core::PCWSTR(wide_null(RUN_KEY).as_ptr()),
            0,
            KEY_QUERY_VALUE,
            &mut hkey,
        );
        if result != ERROR_SUCCESS {
            return None;
        }
        let mut buffer = [0u16; 1024];
        let mut buffer_len = (buffer.len() * 2) as u32;
        let mut reg_type = REG_VALUE_TYPE(0);
        let query = RegQueryValueExW(
            hkey,
            windows::core::PCWSTR(wide_null(RUN_VALUE).as_ptr()),
            None,
            Some(&mut reg_type),
            Some(buffer.as_mut_ptr() as *mut u8),
            Some(&mut buffer_len),
        );
        let _ = RegCloseKey(hkey);
        if query != ERROR_SUCCESS || reg_type != REG_SZ {
            return None;
        }
        let len = (buffer_len / 2) as usize;
        let s = String::from_utf16_lossy(&buffer[..len]);
        Some(s.trim_end_matches('\0').to_string())
    }
}

fn wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Setzt den HKCU Run-Key. Idempotent.
pub fn enable() -> std::io::Result<()> {
    let Some(expected) = expected_value() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "current_exe nicht ermittelbar",
        ));
    };
    unsafe {
        let mut hkey: HKEY = HKEY(std::ptr::null_mut());
        let open = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            windows::core::PCWSTR(wide_null(RUN_KEY).as_ptr()),
            0,
            KEY_SET_VALUE | KEY_QUERY_VALUE,
            &mut hkey,
        );
        if open != ERROR_SUCCESS {
            return Err(std::io::Error::last_os_error());
        }
        // Wide-String fuer Wert.
        let value_w: Vec<u16> = expected.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = std::slice::from_raw_parts(
            value_w.as_ptr() as *const u8,
            value_w.len() * 2,
        );
        let result = RegSetValueExW(
            hkey,
            windows::core::PCWSTR(wide_null(RUN_VALUE).as_ptr()),
            0,
            REG_SZ,
            Some(bytes),
        );
        let _ = RegCloseKey(hkey);
        if result != ERROR_SUCCESS {
            return Err(std::io::Error::last_os_error());
        }
        log::info!("autostart enabled: {}", expected);
        Ok(())
    }
}

/// Loescht den HKCU Run-Key. No-op wenn nicht vorhanden.
pub fn disable() -> std::io::Result<()> {
    unsafe {
        let mut hkey: HKEY = HKEY(std::ptr::null_mut());
        let open = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            windows::core::PCWSTR(wide_null(RUN_KEY).as_ptr()),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        );
        if open != ERROR_SUCCESS {
            return Err(std::io::Error::last_os_error());
        }
        let result = RegDeleteValueW(
            hkey,
            windows::core::PCWSTR(wide_null(RUN_VALUE).as_ptr()),
        );
        let _ = RegCloseKey(hkey);
        if result != ERROR_SUCCESS && result != ERROR_FILE_NOT_FOUND {
            return Err(std::io::Error::last_os_error());
        }
        log::info!("autostart disabled");
        Ok(())
    }
}

/// Toggle: liefert neuen Zustand (true = jetzt aktiv).
pub fn toggle() -> std::io::Result<bool> {
    if is_enabled() {
        disable()?;
        Ok(false)
    } else {
        enable()?;
        Ok(true)
    }
}
