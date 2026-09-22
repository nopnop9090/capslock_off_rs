//! System-Tray-Icon + Menue via `tray-icon` Crate.
//!
//! Menuepunkte (statische IDs):
//!   - Header: Status (read-only)
//!   - "Normal"        (radio, id=mode_normal)
//!   - "Blockiert"     (radio, id=mode_block)
//!   - "Shift Left"    (radio, id=mode_shift)
//!   - Separator
//!   - "Mit Windows starten" (checkbox, id=autostart)
//!   - Separator
//!   - "Test: Caps-Event senden" (id=test_inject)
//!   - Separator
//!   - "Beenden" (id=quit)
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, PostQuitMessage, TranslateMessage, MSG,
};

use crate::app::{self, App};
use crate::autostart;
use crate::icons;
use crate::state::Mode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    SetMode(Mode),
    ToggleAutostart,
    TestInject,
    About,
    Quit,
}

impl From<MenuEvent> for TrayCommand {
    fn from(event: MenuEvent) -> Self {
        let id_str = event.id.0.as_str();
        match id_str {
            "quit" => TrayCommand::Quit,
            "test_inject" => TrayCommand::TestInject,
            "autostart" => TrayCommand::ToggleAutostart,
            "about" => TrayCommand::About,
            "mode_normal" => TrayCommand::SetMode(Mode::Normal),
            "mode_block" => TrayCommand::SetMode(Mode::Block),
            "mode_shift" => TrayCommand::SetMode(Mode::Shift),
            _ => TrayCommand::Quit, // Fallback: unbekannt -> Quit, damit nichts haengt.
        }
    }
}

pub type CmdHandler = Arc<dyn Fn(TrayCommand) + Send + Sync>;

/// Externes Quit-Signal (vom App-Handle genutzt).
static QUIT_SIGNAL: AtomicBool = AtomicBool::new(false);

pub fn signal_quit_external() {
    QUIT_SIGNAL.store(true, Ordering::SeqCst);
}

/// Bauen und Ausfuehren des Tray. Blockiert bis Quit signalisiert wird.
///
/// **Wichtig:** tray-icon 0.19 registriert ein hidden Window mit
/// `WndProc = tray_proc`, startet aber **keine eigene Message-Loop**.
/// Ohne aktive `GetMessageW/DispatchMessageW`-Loop im Mainthread kommen
/// die `WM_USER_TRAYICON`-Klicks vom Explorer-Shell nicht im WindowProc
/// an und das Menü-Popup geht nicht auf.
pub fn run(app: Arc<Mutex<App>>, mode: Mode) -> windows::core::Result<()> {
    QUIT_SIGNAL.store(false, Ordering::SeqCst);
    let app_for_cmd = Arc::clone(&app);
    let cmd_handler: CmdHandler = Arc::new(move |cmd| {
        app::handle_tray_command(Arc::clone(&app_for_cmd), cmd);
    });
    let mut state = TrayState {
        cmd: cmd_handler.clone(),
        icon: None,
        current_mode: mode,
    };
    state
        .build(mode)
        .map_err(|e| windows::core::Error::new(windows::core::HRESULT(-1), e.to_string()))?;

    // File-Logger fuer Diagnose.
    let log_path = tray_log_path();
    let mut log_file: Option<File> = match log_path.as_ref() {
        Some(p) => OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .ok(),
        None => None,
    };
    if let Some(f) = log_file.as_mut() {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "[ts={ts}] tray.run() gestartet, mode={mode:?}");
    }

    // Menu-Event-Channel: muda dispatchet Menu-Klicks via set_event_handler
    // (direkter Callback), wir forwarden an einen mpsc::Sender und lesen
    // in der Main-Message-Loop (non-blocking try_recv zwischen Messages).
    let (menu_tx, menu_rx) = mpsc::channel::<TrayCommand>();
    {
        let log_path_clone = log_path.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let cmd: TrayCommand = event.into();
            if let Some(p) = log_path_clone.as_ref() {
                if let Ok(mut f) = OpenOptions::new().append(true).open(p) {
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let _ = writeln!(f, "[ts={ts}] MENU event -> {cmd:?}");
                }
            }
            let _ = menu_tx.send(cmd);
        }));
    }

    let mut msg = MSG::default();
    let mut iteration: u64 = 0;
    loop {
        iteration += 1;

        // Drain pending menu events (non-blocking).
        while let Ok(cmd) = menu_rx.try_recv() {
            log::info!("TrayCommand: {:?}", cmd);
            if let Some(f) = log_file.as_mut() {
                let _ = writeln!(f, "[iter {iteration}] CMD {cmd:?}");
            }
            cmd_handler(cmd);
            // Nach ToggleAutostart das Menu neu bauen, damit der Haken
            // ([x] vs [ ]) sofort den neuen Status reflektiert.
            if matches!(cmd, TrayCommand::ToggleAutostart) {
                if let Err(e) = state.rebuild_menu() {
                    log::warn!("rebuild_menu fehlgeschlagen: {e:?}");
                }
            }
        }

        // Quit-Signal?
        if QUIT_SIGNAL.load(Ordering::SeqCst) {
            if let Some(f) = log_file.as_mut() {
                let _ = writeln!(f, "[iter {iteration}] QUIT signal");
            }
            unsafe { PostQuitMessage(0) };
        }

        // Auf naechste Message warten (blockierend). Dispatches das
        // tray-icon-Window automatisch via tray_proc.
        let ret = unsafe {
            GetMessageW(&mut msg, HWND(std::ptr::null_mut()), 0, 0)
        };
        if matches!(ret.0, 0 | -1) {
            if let Some(f) = log_file.as_mut() {
                let _ = writeln!(f, "[iter {iteration}] GetMessageW ret={} -> exit", ret.0);
            }
            break;
        }

        // WM_USER_TRAYICON vom Explorer ruft tray_proc auf, der das
        // Menue via TrackPopupMenu zeigt. Menu-Klicks erzeugen dann
        // MenuEvents, die oben gedraint werden.
        unsafe {
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }
    }

    // Cleanup: Handler abmelden + Tray-Icon drop -> Shell_NotifyIcon NIM_DELETE.
    MenuEvent::set_event_handler::<Box<dyn Fn(MenuEvent) + Send + Sync>>(None);
    state.icon = None;
    Ok(())
}

struct TrayState {
    cmd: CmdHandler,
    icon: Option<TrayIcon>,
    current_mode: Mode,
}

impl TrayState {
    fn build(&mut self, mode: Mode) -> tray_icon::Result<()> {
        self.current_mode = mode;
        let menu = build_menu(mode);
        let icon = TrayIconBuilder::new()
            .with_id("capslock_off")
            .with_tooltip(format!("CapsLock: {}", mode.as_str()))
            .with_icon(icons::for_mode(mode))
            .with_menu(Box::new(menu))
            .build()?;
        self.icon = Some(icon);
        Ok(())
    }

    /// Baut das Menu neu (liest `autostart::is_enabled()` frisch aus der
    /// Registry) und setzt es am Tray-Icon. Wird nach `ToggleAutostart`
    /// aufgerufen, damit der [x]/[ ]-Haken den neuen Status zeigt.
    fn rebuild_menu(&mut self) -> tray_icon::Result<()> {
        let menu = build_menu(self.current_mode);
        if let Some(icon) = self.icon.as_ref() {
            icon.set_menu(Some(Box::new(menu)));
        }
        Ok(())
    }
}

fn build_menu(mode: Mode) -> Menu {
    let menu = Menu::new();
    let header = MenuItem::new(
        format!("Status: Caps = {}", mode.as_str()),
        false,
        None,
    );
    let _ = menu.append(&header);
    let _ = menu.append(&PredefinedMenuItem::separator());

    let normal = MenuItem::with_id("mode_normal", "Normal (Caps ist normal)", true, None);
    let block = MenuItem::with_id("mode_block", "Blockiert (Caps tot)", true, None);
    let shift = MenuItem::with_id("mode_shift", "Shift Left (Caps = Shift)", true, None);
    let _ = menu.append(&normal);
    let _ = menu.append(&block);
    let _ = menu.append(&shift);
    let _ = menu.append(&PredefinedMenuItem::separator());

    let auto_item = MenuItem::with_id(
        "autostart",
        if autostart::is_enabled() {
            "[x] Mit Windows starten"
        } else {
            "[ ] Mit Windows starten"
        },
        true,
        None,
    );
    let _ = menu.append(&auto_item);
    let _ = menu.append(&PredefinedMenuItem::separator());

    let test = MenuItem::with_id("test_inject", "Test: Caps-Event senden", true, None);
    let _ = menu.append(&test);
    let _ = menu.append(&PredefinedMenuItem::separator());

    let about = MenuItem::with_id("about", "About...", true, None);
    let _ = menu.append(&about);
    let _ = menu.append(&PredefinedMenuItem::separator());

    let quit = MenuItem::with_id("quit", "Beenden", true, None);
    let _ = menu.append(&quit);
    menu
}

fn tray_log_path() -> Option<std::path::PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    let dir = std::path::Path::new(&appdata).join("capslock_off_rs");
    Some(dir.join("tray.log"))
}