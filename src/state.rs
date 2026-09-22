//! Persistierung des aktuellen Hook-Modus.
//!
//! Schema (Version 2):
//!   { "mode": "normal" | "block" | "shift" }
//!
//! Gespeichert in `%APPDATA%\capslock_off_rs\state.json` (Windows-Standard
//! fuer Standalone-Apps, kein Bezug auf das Repo-Verzeichnis).
//!
//! Atomar via tmp+rename.
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const VALID_MODES: [&str; 3] = ["normal", "block", "shift"];
pub const DEFAULT_MODE: &str = "block";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Normal,
    Block,
    Shift,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Normal => "normal",
            Mode::Block => "block",
            Mode::Shift => "shift",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "normal" => Some(Mode::Normal),
            "block" => Some(Mode::Block),
            "shift" => Some(Mode::Shift),
            _ => None,
        }
    }
}

impl Default for Mode {
    fn default() -> Self {
        Mode::Block
    }
}

/// Liefert den Pfad zur state.json im %APPDATA%\capslock_off_rs\.
pub fn state_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    let dir = Path::new(&appdata).join("capslock_off_rs");
    Some(dir.join("state.json"))
}

#[derive(Debug, Serialize, Deserialize)]
struct StateFile {
    mode: String,
}

/// Laedt den persistierten Mode. Fallback: DEFAULT_MODE.
pub fn load() -> Mode {
    let Some(path) = state_path() else {
        return Mode::default();
    };
    if !path.exists() {
        return Mode::default();
    }
    match fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<StateFile>(&text) {
            Ok(s) => Mode::parse(&s.mode).unwrap_or_default(),
            Err(e) => {
                log::warn!("state.json unparseable: {e}");
                Mode::default()
            }
        },
        Err(e) => {
            log::warn!("state.json lesen fehlgeschlagen: {e}");
            Mode::default()
        }
    }
}

/// Schreibt den Mode atomar via tmp+rename.
pub fn save(mode: Mode) {
    let Some(path) = state_path() else {
        log::warn!("kein APPDATA-Pfad gefunden, state nicht gespeichert");
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            log::warn!("state dir anlegen fehlgeschlagen: {e}");
            return;
        }
    }
    let json = serde_json::to_string_pretty(&StateFile {
        mode: mode.as_str().to_string(),
    })
    .unwrap_or_else(|_| {
        format!(
            "{{\"mode\": \"{}\"}}",
            mode.as_str().replace('"', "\\\"")
        )
    });
    // Atomar: in tmp schreiben, dann rename.
    let tmp = path.with_extension("tmp");
    let write_result = (|| -> std::io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(json.as_bytes())?;
        f.sync_all()?;
        Ok(())
    })();
    if let Err(e) = write_result {
        log::warn!("state tmp schreiben fehlgeschlagen: {e}");
        let _ = fs::remove_file(&tmp);
        return;
    }
    if let Err(e) = fs::rename(&tmp, &path) {
        log::warn!("state rename fehlgeschlagen: {e}");
        let _ = fs::remove_file(&tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_roundtrip() {
        for m in [Mode::Normal, Mode::Block, Mode::Shift] {
            assert_eq!(Mode::parse(m.as_str()), Some(m));
        }
        assert_eq!(Mode::parse("invalid"), None);
    }
}
