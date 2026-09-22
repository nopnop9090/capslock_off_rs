//! App-Orchestrator: verbindet Hook + Tray + IPC + State.
//!
//! Reihenfolge beim Start:
//!   1. State laden (Mode aus %APPDATA%\capslock_off_rs\state.json).
//!   2. Hook starten (WH_KEYBOARD_LL im eigenen Thread).
//!   3. IPC-Server starten (Hidden Window + WM_USER-Handler).
//!   4. Icons vorladen.
//!   5. Tray bauen + Cmd-Handler installieren (Mainthread).
//!   6. Tray.run() blockiert.
//!   7. on quit: Hook + IPC stoppen, State speichern.
//!
//! `App` selbst enthaelt KEIN TrayApp, weil TrayIcon intern `Rc<RefCell>`
//! ist und nicht Sync. TrayApp lebt separat im Mainthread und bekommt
//! `Arc<Mutex<App>>` zur Kommunikation.
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use crate::autostart;
use crate::hook::{self, KeyboardHook};
use crate::ipc::IpcServer;
use crate::state::{self, Mode};
use crate::version::VERSION;

/// Single Source of Truth fuer den aktuellen Mode.
static CURRENT_MODE: AtomicU8 = AtomicU8::new(1);

pub struct App {
    hook: KeyboardHook,
    ipc: Option<IpcServer>,
}

impl App {
    pub fn new() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            hook: KeyboardHook::new(),
            ipc: None,
        }))
    }

    pub fn run(app: Arc<Mutex<Self>>) -> windows::core::Result<()> {
        let initial = state::load();
        CURRENT_MODE.store(mode_to_u8(initial), Ordering::SeqCst);
        log::info!("Start: persisted mode={}, version={}", initial.as_str(), VERSION);

        // Hook starten.
        {
            let mut me = app.lock().unwrap();
            me.hook.set_mode(initial);
            me.hook.start()?;
        }

        // Icons vorladen.
        crate::icons::preload();

        // IPC-Server starten.
        // Zwei separate Arc-Clones, damit die zwei Closures unabhaengig
        // je ein eigenes Arc-Borrow-Lifetime haben.
        let app_for_ipc_setter = Arc::clone(&app);
        let app_for_ipc_stats = Arc::clone(&app);
        let ipc = IpcServer::new(
            Arc::new(|| current_mode()),
            Arc::new(move |m: Mode| {
                apply_mode(Arc::clone(&app_for_ipc_setter), m);
            }),
            Arc::new(|| VERSION.to_string()),
            Arc::new(move || {
                let me = app_for_ipc_stats.lock().unwrap();
                let m = current_mode();
                let s = me.hook.stats();
                (m, s)
            }),
        );
        let mut ipc = ipc;
        if let Err(e) = ipc.start() {
            log::error!("IPC start fehlgeschlagen: {e:?}");
        }
        app.lock().unwrap().ipc = Some(ipc);

        // Tray bauen und laufen lassen (Mainthread).
        let app_for_tray = Arc::clone(&app);
        crate::tray::run(app_for_tray, initial)?;

        // Cleanup.
        log::info!("Beende...");
        state::save(current_mode());
        let mut me = app.lock().unwrap();
        me.hook.stop();
        if let Some(mut ipc) = me.ipc.take() {
            ipc.stop();
        }
        Ok(())
    }
}

/// Mode setzen: Hook + State + IPC-Broadcast.
fn apply_mode(app: Arc<Mutex<App>>, mode: Mode) {
    CURRENT_MODE.store(mode_to_u8(mode), Ordering::SeqCst);
    state::save(mode);
    {
        let me = app.lock().unwrap();
        me.hook.set_mode(mode);
    }
    // IPC-Broadcast an externe Listener.
    if let Ok(me) = app.lock() {
        if let Some(ipc) = me.ipc.as_ref() {
            ipc.broadcast_mode_change(mode);
        }
    }
    log::info!("Mode angewendet: {:?}", mode);
}

pub fn handle_tray_command(app: Arc<Mutex<App>>, cmd: crate::tray::TrayCommand) {
    use crate::tray::TrayCommand;
    match cmd {
        TrayCommand::SetMode(mode) => apply_mode(app, mode),
        TrayCommand::ToggleAutostart => match autostart::toggle() {
            Ok(active) => log::info!("autostart toggled -> {active}"),
            Err(e) => log::error!("autostart toggle failed: {e}"),
        },
        TrayCommand::TestInject => {
            let stats_before = {
                let me = app.lock().unwrap();
                me.hook.stats()
            };
            log::info!(
                "Test-Inject (caps vorher={}, mode={})",
                hook::capslock_state(),
                current_mode().as_str()
            );
            hook::inject_capslock();
            let stats_after = {
                let me = app.lock().unwrap();
                me.hook.stats()
            };
            log::info!(
                "Test-Inject: caps nachher={}, delta_seen={}, delta_blocked={}, delta_shift_inj={}",
                hook::capslock_state(),
                stats_after.seen_count - stats_before.seen_count,
                stats_after.blocked_count - stats_before.blocked_count,
                stats_after.shift_injected_count - stats_before.shift_injected_count,
            );
        }
        TrayCommand::Quit => {
            // Tray.run() beendet sich, wenn die uebergebene Quit-Funktion aufgerufen wird.
            // Wir nutzen einen Channel, um das zu signalisieren.
            crate::tray::signal_quit_external();
        }
    }
}

fn current_mode() -> Mode {
    mode_from_u8(CURRENT_MODE.load(Ordering::SeqCst))
}

fn mode_to_u8(m: Mode) -> u8 {
    match m {
        Mode::Normal => 0,
        Mode::Block => 1,
        Mode::Shift => 2,
    }
}
fn mode_from_u8(v: u8) -> Mode {
    match v {
        2 => Mode::Shift,
        1 => Mode::Block,
        _ => Mode::Normal,
    }
}
