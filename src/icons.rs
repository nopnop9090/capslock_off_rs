//! Tray-Icon Loader.
//!
//! PNG-Icons werden zur Compile-Time via `include_bytes!` in die EXE
//! eingebettet. Es gibt keine Runtime-File-Reads aus `assets/`. Die
//! `assets/`-Dateien existieren nur noch fuer die README-Badges.
//!
//! 3 Icons:
//!   - `icon_normal.png`  gelber Akzent
//!   - `icon_block.png`   gruener Akzent + roter Slash
//!   - `icon-shift.png`   blauer Akzent + Aufwaerts-Pfeil
use crate::state::Mode;

// Compile-time in die Binary eingebettet. Pfade relativ zu src/.
const ICON_NORMAL_PNG: &[u8] = include_bytes!("../assets/icon_normal.png");
const ICON_BLOCK_PNG: &[u8] = include_bytes!("../assets/icon_block.png");
const ICON_SHIFT_PNG: &[u8] = include_bytes!("../assets/icon_shift.png");

fn load_icon(bytes: &[u8]) -> Option<tray_icon::Icon> {
    let img = image::load_from_memory(bytes)
        .map_err(|e| log::warn!("Icon-Dekodierung fehlgeschlagen: {e}"))
        .ok()?;
    let rgba = img.into_rgba8();
    let (w, h) = rgba.dimensions();
    tray_icon::Icon::from_rgba(rgba.into_raw(), w, h)
        .map_err(|e| log::warn!("tray_icon::Icon-Konvertierung: {e}"))
        .ok()
}

fn fallback() -> tray_icon::Icon {
    // 16x16 dunkelgrau. Sollte nie greifen wenn die PNGs korrekt
    // eingebettet sind.
    tray_icon::Icon::from_rgba(vec![80u8; 16 * 16 * 4], 16, 16).unwrap()
}

/// Liefert das passende Icon fuer den Mode (zur Compile-Time eingebettet).
pub fn for_mode(mode: Mode) -> tray_icon::Icon {
    let bytes = match mode {
        Mode::Normal => ICON_NORMAL_PNG,
        Mode::Block => ICON_BLOCK_PNG,
        Mode::Shift => ICON_SHIFT_PNG,
    };
    load_icon(bytes).unwrap_or_else(fallback)
}

/// Initialisiert (laedt einmal). Sollte beim Start aufgerufen werden,
/// damit Asset-Fehler frueh sichtbar werden (loggt nur, faellt sonst
/// auf fallback zurueck).
pub fn preload() {
    for bytes in [ICON_NORMAL_PNG, ICON_BLOCK_PNG, ICON_SHIFT_PNG] {
        if load_icon(bytes).is_none() {
            log::warn!("preload: Icon fehlgeschlagen ({} bytes)", bytes.len());
        }
    }
}