//! System-Tray-Icon + Menue via `tray-icon` Crate (muda-Backend).
//!
//! Menuepunkte:
//!   - Header: "Caps = <mode>" (read-only)
//!   - 3 CheckMenuItems fuer die Modi (id=mode_normal/block/shift) -
//!     zeigen nativen Windows-Haken (`MF_CHECKED`).
//!   - CheckMenuItem fuer Autostart (id=autostart)
//!   - MenuItems fuer Test/About/Quit (statisch)
//!
//! muda nutzt Windows-native Checkbox-Style fuer alle CheckMenuItems.
//! Visuell zeigen die 3 Mode-Items einen Haken am aktiven Mode -- das ist
//! Standard-UX fuer mutually-exclusive Menue-Selection (siehe Windows
//! Explorer "View > Sort by"). Bei Bedarf kann man spaeter auf
//! MF_RADIOCHECK umsteigen, aber das braucht Zugriff auf muda's
//! platform_impl, was `pub(crate)` ist.
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
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

static QUIT_SIGNAL: AtomicBool = AtomicBool::new(false);

pub fn signal_quit_external() {
    QUIT_SIGNAL.store(true, Ordering::SeqCst);
}

pub fn run(app: Arc<Mutex<App>>, mode: Mode) -> windows::core::Result<()> {
    QUIT_SIGNAL.store(false, Ordering::SeqCst);
    let app_for_cmd = Arc::clone(&app);
    let cmd_handler: CmdHandler = Arc::new(move |cmd| {
        app::handle_tray_command(Arc::clone(&app_for_cmd), cmd);
    });
    let mut state = TrayState::new();
    state
        .build(mode)
        .map_err(|e| windows::core::Error::new(windows::core::HRESULT(-1), e.to_string()))?;

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

        while let Ok(cmd) = menu_rx.try_recv() {
            log::info!("TrayCommand: {:?}", cmd);
            if let Some(f) = log_file.as_mut() {
                let _ = writeln!(f, "[iter {iteration}] CMD {cmd:?}");
            }
            cmd_handler(cmd);

            // Mode-Wechsel: Haken auf den richtigen Mode setzen,
            // Tooltip + Icon aktualisieren.
            if let TrayCommand::SetMode(new_mode) = cmd {
                state.current_mode = new_mode;
                state.refresh_mode_checks();
                if let Some(icon) = state.icon.as_ref() {
                    let _ = icon.set_tooltip(Some(format!("CapsLock: {}", new_mode.as_str())));
                    let _ = icon.set_icon(Some(icons::for_mode(new_mode)));
                }
            }
            // Autostart-Toggle: Haken aus Registry frisch lesen.
            if matches!(cmd, TrayCommand::ToggleAutostart) {
                state.refresh_autostart_check();
            }
        }

        if QUIT_SIGNAL.load(Ordering::SeqCst) {
            if let Some(f) = log_file.as_mut() {
                let _ = writeln!(f, "[iter {iteration}] QUIT signal");
            }
            unsafe { PostQuitMessage(0) };
        }

        let ret = unsafe {
            GetMessageW(&mut msg, HWND(std::ptr::null_mut()), 0, 0)
        };
        if matches!(ret.0, 0 | -1) {
            if let Some(f) = log_file.as_mut() {
                let _ = writeln!(f, "[iter {iteration}] GetMessageW ret={} -> exit", ret.0);
            }
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }
    }

    MenuEvent::set_event_handler::<Box<dyn Fn(MenuEvent) + Send + Sync>>(None);
    state.icon = None;
    Ok(())
}

/// Hauptobjekt im Mainthread: haelt das TrayIcon + alle Menu-Items,
/// damit wir `set_checked` auf den Items aufrufen koennen (fuer Mode-
/// Wechsel und Autostart-Toggle).
struct TrayState {
    icon: Option<TrayIcon>,
    current_mode: Mode,
    normal_item: Option<CheckMenuItem>,
    block_item: Option<CheckMenuItem>,
    shift_item: Option<CheckMenuItem>,
    autostart_item: Option<CheckMenuItem>,
}

impl TrayState {
    fn new() -> Self {
        Self {
            icon: None,
            current_mode: Mode::Block, // wird in build() ueberschrieben
            normal_item: None,
            block_item: None,
            shift_item: None,
            autostart_item: None,
        }
    }

    fn build(&mut self, mode: Mode) -> tray_icon::Result<()> {
        self.current_mode = mode;

        let normal = CheckMenuItem::with_id(
            "mode_normal",
            "Normal (Caps ist normal)",
            true,
            mode == Mode::Normal,
            None,
        );
        let block = CheckMenuItem::with_id(
            "mode_block",
            "Blockiert (Caps tot)",
            true,
            mode == Mode::Block,
            None,
        );
        let shift = CheckMenuItem::with_id(
            "mode_shift",
            "Shift Left (Caps = Shift)",
            true,
            mode == Mode::Shift,
            None,
        );
        let autostart = CheckMenuItem::with_id(
            "autostart",
            "Mit Windows starten",
            true,
            autostart::is_enabled(),
            None,
        );
        let test = MenuItem::with_id("test_inject", "Test: Caps-Event senden", true, None);
        let about = MenuItem::with_id("about", "About...", true, None);
        let quit = MenuItem::with_id("quit", "Beenden", true, None);

        let menu = Menu::new();
        let _ = menu.append(&normal);
        let _ = menu.append(&block);
        let _ = menu.append(&shift);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&autostart);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&test);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&about);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&quit);

        let icon = TrayIconBuilder::new()
            .with_id("capslock_off")
            .with_tooltip(format!("CapsLock: {}", mode.as_str()))
            .with_icon(icons::for_mode(mode))
            .with_menu(Box::new(menu))
            .build()?;
        self.icon = Some(icon);

        self.normal_item = Some(normal);
        self.block_item = Some(block);
        self.shift_item = Some(shift);
        self.autostart_item = Some(autostart);

        Ok(())
    }

    fn refresh_mode_checks(&self) {
        let m = self.current_mode;
        if let Some(i) = self.normal_item.as_ref() {
            i.set_checked(m == Mode::Normal);
        }
        if let Some(i) = self.block_item.as_ref() {
            i.set_checked(m == Mode::Block);
        }
        if let Some(i) = self.shift_item.as_ref() {
            i.set_checked(m == Mode::Shift);
        }
    }

    fn refresh_autostart_check(&self) {
        if let Some(i) = self.autostart_item.as_ref() {
            i.set_checked(autostart::is_enabled());
        }
    }
}

fn tray_log_path() -> Option<std::path::PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    let dir = std::path::Path::new(&appdata).join("capslock_off_rs");
    Some(dir.join("tray.log"))
}