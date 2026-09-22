//! Tray-Icon Loader.
//!
//! Laedt PNG-Dateien aus `assets/` zur Runtime und konvertiert sie in
//! RGBA-Buffer (das Format, das `tray_icon::Icon` erwartet).
//!
//! 3 Icons:
//!   - `icon_normal.png`  gelber Akzent
//!   - `icon_block.png`   gruener Akzent + roter Slash
//!   - `icon-shift.png`   blauer Akzent + Aufwaerts-Pfeil
//!
//! Kein Caching auf Modulebene -- `tray_icon::Icon` ist nicht Sync (intern
//! `*mut c_void`). Jeder Aufrufer haelt sein eigenes Icon in seinem Struct.
use std::path::PathBuf;

use crate::state::Mode;

/// Liefert den Pfad zum assets/-Verzeichnis.
/// Suche relativ zur Exe, sonst relativ zum aktuellen Working-Dir.
fn assets_dir() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let candidates = [
                parent.join("assets"),
                parent.parent().map(|p| p.join("assets")).unwrap_or_default(),
                parent
                    .parent()
                    .and_then(|p| p.parent())
                    .map(|p| p.join("assets"))
                    .unwrap_or_default(),
            ];
            for c in &candidates {
                if c.exists() {
                    return Some(c.clone());
                }
            }
        }
    }
    let cwd = std::env::current_dir().ok()?.join("assets");
    if cwd.exists() {
        return Some(cwd);
    }
    None
}

fn load_icon(name: &str) -> Option<tray_icon::Icon> {
    let dir = assets_dir()?;
    let path = dir.join(name);
    let bytes = std::fs::read(&path)
        .map_err(|e| log::warn!("Icon {path:?} lesen fehlgeschlagen: {e}"))
        .ok()?;
    let img = image::load_from_memory(&bytes)
        .map_err(|e| log::warn!("Icon {path:?} dekodieren fehlgeschlagen: {e}"))
        .ok()?;
    let rgba = img.into_rgba8();
    let (w, h) = rgba.dimensions();
    tray_icon::Icon::from_rgba(rgba.into_raw(), w, h)
        .map_err(|e| log::warn!("Icon {name} in tray-icon::Icon konvertieren: {e}"))
        .ok()
}

fn load_for_mode(mode: Mode) -> Option<tray_icon::Icon> {
    let name = match mode {
        Mode::Normal => "icon_normal.png",
        Mode::Block => "icon_block.png",
        Mode::Shift => "icon_shift.png",
    };
    load_icon(name)
}

/// Liefert das passende Icon fuer den Mode.
/// Fallback: einfarbiges Rechteck (sollte nie greifen wenn assets/ stimmt).
pub fn for_mode(mode: Mode) -> tray_icon::Icon {
    if let Some(icon) = load_for_mode(mode) {
        return icon;
    }
    // Fallback: 1x1 graues Icon.
    tray_icon::Icon::from_rgba(vec![128, 128, 128, 255], 1, 1).unwrap()
}

/// Initialisiert (laedt einmal). Sollte beim Start aufgerufen werden,
/// damit Asset-Fehler frueh sichtbar werden.
pub fn preload() {
    for mode in [Mode::Normal, Mode::Block, Mode::Shift] {
        let _ = for_mode(mode);
    }
    log::info!("assets_dir = {:?}", assets_dir());
}
