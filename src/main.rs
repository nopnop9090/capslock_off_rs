//! Entry-Point und CLI.
//!
//! Args:
//!   capslock_off                  Default: run mit Tray + Hook
//!   capslock_off run             explizit run
//!   capslock_off probe           3-Phasen-Smoke-Test, exit 0/1
//!   capslock_off --debug         debug logging
//!   capslock_off --autostart    versteckter Marker (vom Autostart-Eintrag gesetzt)
//!
//! `--autostart` signalisiert der Binary, dass sie vom Autostart gestartet
//! wurde. Aktuell nur ein Marker ohne Verhaltensaenderung (die Binary
//! startet immer minimiert via Tray).
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
                print_help();
                return ExitCode::from(0);
            }
            "--version" | "-V" => {
                println!("capslock_off v{}", version::VERSION);
                return ExitCode::from(0);
            }
            other => {
                eprintln!("unbekanntes Argument: {other}");
                print_help();
                return ExitCode::from(2);
            }
        }
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
        "probe" => match probe::run() {
            0 => ExitCode::from(0),
            _ => ExitCode::from(1),
        },
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
