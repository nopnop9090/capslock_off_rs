//! Entry-Point und CLI.
//!
//! Args:
//!   capslock_off                  Default: run mit Tray + Hook
//!   capslock_off run             explizit run
//!   capslock_off probe           3-Phasen-Smoke-Test, exit 0/1
//!   capslock_off --debug         debug logging
//!   capslock_off --autostart    versteckter Marker (vom Autostart-Eintrag gesetzt)
//!
//! `#![windows_subsystem = "windows"]` deaktiviert das automatische
//! Console-Fenster (analog zu pythonw.exe). Logs laufen weiterhin via
//! env_logger auf stderr; ohne Konsole sieht man sie nicht, aber sie
//! koennen bei Bedarf mit `--debug` in ein File umgeleitet werden.
//! `probe` allokiert explizit eine Console, damit Output sichtbar ist.
#![windows_subsystem = "windows"]

mod app;
mod autostart;
mod hook;
mod icons;
mod ipc;
mod probe;
mod state;
mod tray;
mod version;

use std::process::ExitCode;

#[cfg(windows)]
fn ensure_console() {
    use windows::Win32::System::Console::{AllocConsole, AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        // Versuche erst, an die Parent-Console anzudocken (falls von
        // cmd/PowerShell gestartet). Falls keine existiert, eigene allokieren.
        if AttachConsole(ATTACH_PARENT_PROCESS).is_ok() {
            return;
        }
        let _ = AllocConsole();
    }
}

#[cfg(not(windows))]
fn ensure_console() {}

fn main() -> ExitCode {
    // CLI-Parse (manuell, kein clap um Dependency zu sparen).
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut debug = false;
    let mut autostart_marker = false;
    let mut cmd = "run".to_string();
    for arg in &args {
        match arg.as_str() {
            "--debug" => debug = true,
            "--autostart" => autostart_marker = true,
            "run" | "probe" => cmd = arg.clone(),
            "--help" | "-h" => {
                ensure_console();
                print_help();
                return ExitCode::from(0);
            }
            "--version" | "-V" => {
                ensure_console();
                println!("capslock_off v{}", version::VERSION);
                return ExitCode::from(0);
            }
            other => {
                ensure_console();
                eprintln!("unbekanntes Argument: {other}");
                print_help();
                return ExitCode::from(2);
            }
        }
    }

    // Bei probe eine Console allokieren (sonst sieht man nichts, weil
    // `#![windows_subsystem = "windows"]` die default-Console unterdrueckt).
    if cmd == "probe" {
        ensure_console();
    }

    // Logger init.
    let level = if debug {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };
    if let Err(e) = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(level.as_str()),
    )
    .format_timestamp_secs()
    .try_init()
    {
        // Doppelte init bei mehrfachen main-Aufrufen ist ok.
        eprintln!("logger init failed: {e}");
    }

    if autostart_marker {
        log::info!("autostart-Marker gesetzt (gestartet via HKCU Run-Key)");
    }

    match cmd.as_str() {
        "probe" => {
            ensure_console();
            // Output sowohl auf Console als auch in Log-File -- Console
            // koennte beim Exit sofort geschlossen werden, das File bleibt.
            let log_path = probe_log_path();
            let rc = probe::run_with_log(&log_path);
            // Letzte Zeile nochmal an Console (falls sichtbar).
            if let Some(text) = log_path.as_deref().and_then(read_log_tail) {
                println!("{text}");
            }
            if let Some(p) = log_path {
                println!("[probe-Log: {}]", p.display());
            }
            if rc == 0 { ExitCode::from(0) } else { ExitCode::from(1) }
        }
        "run" => {
            let app = app::App::new();
            if let Err(e) = app::App::run(app) {
                log::error!("App::run fehlgeschlagen: {e:?}");
                return ExitCode::from(1);
            }
            ExitCode::from(0)
        }
        _ => {
            eprintln!("unbekannter Befehl: {cmd}");
            print_help();
            ExitCode::from(2)
        }
    }
}

fn print_help() {
    println!(
        "capslock_off v{} -- CapsLock-Taste systemweit deaktivieren oder als Shift.\n\
         \n\
         Usage:\n  \
           capslock_off [run]           Tray + Hook starten (default).\n  \
           capslock_off probe          3-Phasen-Smoke-Test (block/shift/normal), exit 0/1.\n  \
         \n\
         Options:\n  \
           --debug                     DEBUG-Logging.\n  \
           --autostart                 Marker: via HKCU Run-Key gestartet.\n  \
           -h, --help                  Diese Hilfe.\n  \
           -V, --version               Version ausgeben.\n",
        version::VERSION
    );
}

fn probe_log_path() -> Option<std::path::PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    let dir = std::path::Path::new(&appdata).join("capslock_off_rs");
    Some(dir.join("probe.log"))
}

fn read_log_tail(path: &std::path::Path) -> Option<String> {
    // Ganzes File lesen -- bei ~500 Bytes unkritisch.
    let text = std::fs::read_to_string(path).ok()?;
    // Nur die letzten 3 Zeilen (sonst wuerde das grosse Log gespooolt).
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(3);
    Some(lines[start..].join("\n"))
}
