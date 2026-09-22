# Changelog

## v0.1.0 (2026-09-22)

Erste Version. Port der Python-`capslock_off`-Vorlage auf Rust als native
single-file exe.

### Features

- `WH_KEYBOARD_LL`-Hook im eigenen Thread mit Message-Pump.
- Drei Modi: Block (default), Shift Left, Normal.
- Re-Entrancy-Schutz für synthetisierte Shift-Events via `injecting`-Flag.
- Tray-Icon mit Radio-Auswahl der drei Modi.
- Modus-Persistierung in `%APPDATA%\capslock_off_rs\state.json`.
- Optionaler Autostart via HKCU Run-Key (kein Admin nötig).
- Cross-Process-IPC via `WM_COPYDATA` + Shared-Memory + Event
  (siehe README für Layout-Specs).
- Discovery-Datei in `%APPDATA%\capslock_off_rs\ipc.json` mit HWND +
  Thread-ID für externe Clients.
- `probe`-CLI-Mode: 3-Phasen-Smoke-Test (Block / Shift / Normal) mit
  Counter-Vergleich, exit 0/1.

### Architektur

- Hook + IPC als Send+Sync; TrayApp separat im Mainthread mit
  `Arc<Mutex<App>>`-Capture.
- HWND_MESSAGE für unsichtbares IPC-Window (nicht in Taskbar / Alt-Tab).
- HHOOK / HWND werden als raw u64-Pointer durch mpsc-Channel
  transportiert (HWND/HHOOK sind intern `*mut c_void` und nicht Send).

### Build

- `opt-level=3`, `lto=fat`, `codegen-units=1`, `strip=true`,
  `panic=abort` → 1.4 MB Binary.
- Crates: `windows 0.58`, `tray-icon 0.19`, `image 0.25`,
  `serde`, `serde_json`, `log`, `env_logger`, `once_cell`.

### Bekannte Limitierungen

- IPC nur mit Discovery-Datei nutzbar (HWND_MESSAGE ist unsichtbar für
  `FindWindow` / `EnumWindows`).
- Antworten via Shared-Memory + Event (WM_COPYDATA ist One-Way).
- Keine `SetCheckmark`-API in tray-icon 0.19 — Radio-Markierung und
  Autostart-Check sind Text-Prefixe (`(o)` / `[x]`).