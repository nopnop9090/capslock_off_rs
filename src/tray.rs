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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

use crate::app::{self, App};
use crate::autostart;
use crate::icons;
use crate::state::Mode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    SetMode(Mode),
    ToggleAutostart,
    TestInject,
    Quit,
}

pub type CmdHandler = Arc<dyn Fn(TrayCommand) + Send + Sync>;

/// Externes Quit-Signal (vom App-Handle genutzt).
static QUIT_SIGNAL: AtomicBool = AtomicBool::new(false);

pub fn signal_quit_external() {
    QUIT_SIGNAL.store(true, Ordering::SeqCst);
}

/// Bauen und Ausfuehren des Tray. Blockiert bis Quit signalisiert wird.
pub fn run(app: Arc<Mutex<App>>, mode: Mode) -> windows::core::Result<()> {
    QUIT_SIGNAL.store(false, Ordering::SeqCst);
    let app_for_cmd = Arc::clone(&app);
    let cmd_handler: CmdHandler = Arc::new(move |cmd| {
        app::handle_tray_command(Arc::clone(&app_for_cmd), cmd);
    });
    let mut state = TrayState {
        cmd: cmd_handler,
        icon: None,
    };
    state
        .build(mode)
        .map_err(|e| windows::core::Error::new(windows::core::HRESULT(-1), e.to_string()))?;

    let menu_rx = MenuEvent::receiver();
    loop {
        if let Ok(event) = menu_rx.try_recv() {
            handle_menu_event(&state, event);
        }
        if QUIT_SIGNAL.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Ok(())
}

struct TrayState {
    cmd: CmdHandler,
    icon: Option<TrayIcon>,
}

impl TrayState {
    fn build(&mut self, mode: Mode) -> tray_icon::Result<()> {
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

    fn refresh(&mut self, mode: Mode) {
        if let Some(icon) = self.icon.as_ref() {
            let _ = icon.set_icon(Some(icons::for_mode(mode)));
            let _ = icon.set_tooltip(Some(format!("CapsLock: {}", mode.as_str())));
        }
    }
}

fn handle_menu_event(state: &TrayState, event: MenuEvent) {
    let id_str = event.id.0.as_str();
    let cmd = match id_str {
        "quit" => Some(TrayCommand::Quit),
        "test_inject" => Some(TrayCommand::TestInject),
        "autostart" => Some(TrayCommand::ToggleAutostart),
        "mode_normal" => Some(TrayCommand::SetMode(Mode::Normal)),
        "mode_block" => Some(TrayCommand::SetMode(Mode::Block)),
        "mode_shift" => Some(TrayCommand::SetMode(Mode::Shift)),
        _ => None,
    };
    if let Some(c) = cmd {
        (state.cmd)(c);
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

    let quit = MenuItem::with_id("quit", "Beenden", true, None);
    let _ = menu.append(&quit);
    menu
}
