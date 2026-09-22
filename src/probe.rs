//! Smoke-Test: durchlaeuft alle 3 Hook-Modi und prueft Counter.
//!
//! Erwartung (identisch zur Python-Version):
//!   block:   Hook sieht Events, blockt sie, kein Shift-Inject
//!   shift:   Hook sieht Events, blockt sie, injiziert Shift
//!   normal:  Hook sieht Events, blockt nichts, injiziert nichts
//!
//! blocked_count zaehlt nur block-Modus-Schluckungen;
//! shift_injected_count zaehlt nur shift-Modus-Injektionen.
//!
//! Exit 0 wenn alle Phasen ok, sonst Exit 1.
use std::thread;
use std::time::Duration;

use crate::hook::{self, KeyboardHook};
use crate::state::Mode;

#[derive(Debug, Clone, Copy, Default)]
struct PhaseStats {
    seen: u32,
    blocked: u32,
    shift_inj: u32,
}

pub fn run() -> i32 {
    log::info!("Probe-Modus: Hook + Auto-Inject (alle 3 Modi).");
    let mut h = KeyboardHook::new();
    if let Err(e) = h.start() {
        log::error!("hook.start() fehlgeschlagen: {e:?}");
        return 1;
    }
    thread::sleep(Duration::from_secs(1));

    let block_s = run_phase(&h, Mode::Block);
    let shift_s = run_phase(&h, Mode::Shift);
    let normal_s = run_phase(&h, Mode::Normal);

    h.stop();

    let ok_block = block_s.seen >= 2 && block_s.blocked >= 2 && block_s.shift_inj == 0;
    let ok_shift = shift_s.seen >= block_s.seen + 2
        && shift_s.shift_inj >= 2
        && shift_s.blocked == block_s.blocked;
    let ok_normal = normal_s.seen >= shift_s.seen + 2
        && normal_s.blocked == shift_s.blocked
        && normal_s.shift_inj == shift_s.shift_inj;

    let ok = ok_block && ok_shift && ok_normal;

    println!(
        "probe_ergebnis: block=ok({}) shift=ok({}) normal=ok({}) \
         block[seen={},blocked={},inj={}] shift[seen={},blocked={},inj={}] \
         normal[seen={},blocked={},inj={}] OK={}",
        ok_block,
        ok_shift,
        ok_normal,
        block_s.seen,
        block_s.blocked,
        block_s.shift_inj,
        shift_s.seen,
        shift_s.blocked,
        shift_s.shift_inj,
        normal_s.seen,
        normal_s.blocked,
        normal_s.shift_inj,
        ok,
    );

    if ok {
        0
    } else {
        1
    }
}

fn run_phase(h: &KeyboardHook, mode: Mode) -> PhaseStats {
    h.set_mode(mode);
    thread::sleep(Duration::from_millis(100));
    let caps_before = hook::capslock_state();
    hook::inject_capslock();
    thread::sleep(Duration::from_millis(300));
    let after = h.stats();
    log::info!(
        "[{:?}] caps {}->{} seen={} blocked={} shift_inj={}",
        mode,
        caps_before,
        hook::capslock_state(),
        after.seen_count,
        after.blocked_count,
        after.shift_injected_count,
    );
    PhaseStats {
        seen: after.seen_count,
        blocked: after.blocked_count,
        shift_inj: after.shift_injected_count,
    }
}
