//! WH_KEYBOARD_LL Hook: CapsLock abfangen, blocken oder als Shift umlenken.
//!
//! Architektur (identisch zum Python-Vorgaenger):
//! - Eigener Thread mit `GetMessageW`-Loop (sonst wird der Hook nach
//!   ~5s von Windows still entfernt).
//! - Modus per `AtomicU8`: `0=normal`, `1=block`, `2=shift`.
//! - Im shift-Modus wird VK_CAPITAL KeyDown+KeyUp via `SendInput(VK_LSHIFT)`
//!   synthetisiert. Re-Entrancy-Schutz via `injecting`-Flag.
//! - Callback ist `extern "system"` und wird ueber `Box::leak` statisch
//!   verankert, damit der Function-Pointer stabil bleibt.
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::OnceLock;
use std::thread::{self, JoinHandle};

use windows::core::Result;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    VIRTUAL_KEY, VK_CAPITAL, VK_LSHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HOOKPROC, PostThreadMessageW, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL,
    WM_KEYDOWN, WM_KEYUP, WM_QUIT,
};

use crate::state::Mode;

const MODE_NORMAL_U8: u8 = 0;
const MODE_BLOCK_U8: u8 = 1;
const MODE_SHIFT_U8: u8 = 2;

fn mode_to_u8(m: Mode) -> u8 {
    match m {
        Mode::Normal => MODE_NORMAL_U8,
        Mode::Block => MODE_BLOCK_U8,
        Mode::Shift => MODE_SHIFT_U8,
    }
}
fn mode_from_u8(v: u8) -> Mode {
    match v {
        MODE_SHIFT_U8 => Mode::Shift,
        MODE_BLOCK_U8 => Mode::Block,
        _ => Mode::Normal,
    }
}

/// Globaler Hook-State. Wird einmalig beim Start initialisiert und beim
/// Stop aufgeraeumt. Atomic-Operationen, kein Mutex im Hot-Path.
static HOOK: OnceLock<HookShared> = OnceLock::new();

struct HookShared {
    mode: AtomicU8,
    injecting: AtomicBool,
    seen_count: AtomicU32,
    blocked_count: AtomicU32,
    shift_injected_count: AtomicU32,
    stop: AtomicBool,
}

impl HookShared {
    fn new() -> Self {
        Self {
            mode: AtomicU8::new(MODE_BLOCK_U8),
            injecting: AtomicBool::new(false),
            seen_count: AtomicU32::new(0),
            blocked_count: AtomicU32::new(0),
            shift_injected_count: AtomicU32::new(0),
            stop: AtomicU8_set(false),
        }
    }
}

fn AtomicU8_set(b: bool) -> AtomicBool {
    AtomicBool::new(b)
}

/// Public statistics snapshot.
#[derive(Debug, Clone, Copy)]
pub struct HookStats {
    pub seen_count: u32,
    pub blocked_count: u32,
    pub shift_injected_count: u32,
}

pub struct KeyboardHook {
    thread: Option<JoinHandle<()>>,
    thread_id: Option<u32>,
    hhook: HHOOK,
}

unsafe impl Send for KeyboardHook {}
unsafe impl Sync for KeyboardHook {}

impl KeyboardHook {
    pub fn new() -> Self {
        Self {
            thread: None,
            thread_id: None,
            hhook: HHOOK::default(),
        }
    }

    /// Startet den Hook-Thread. Idempotent.
    pub fn start(&mut self) -> Result<()> {
        if self.thread.is_some() {
            return Ok(());
        }
        // Globale State-Struktur einmalig initialisieren.
        let _ = HOOK.get_or_init(HookShared::new);
        // HHOOK ist intern *mut c_void und nicht Send. Daher transportieren
        // wir nur die rohe Pointer-Zahl als u64 ueber den Channel.
        let (tx, rx) = std::sync::mpsc::channel::<(u64, u32)>();
        let handle = thread::Builder::new()
            .name("capslock-hook".into())
            .spawn(move || {
                run_pump(tx);
            })
            .map_err(|e| windows::core::Error::new(windows::core::HRESULT(-1), e.to_string()))?;
        let (hhook_raw, thread_id) = rx
            .recv()
            .map_err(|_| windows::core::Error::new(windows::core::HRESULT(-2), "hook init failed"))?;
        let hhook = HHOOK(hhook_raw as *mut std::ffi::c_void);
        self.thread_id = Some(thread_id);
        self.thread = Some(handle);
        self.hhook = hhook;
        log::info!("WH_KEYBOARD_LL installiert (hHook={:?})", hhook);
        Ok(())
    }

    pub fn stop(&mut self) {
        let shared = match HOOK.get() {
            Some(s) => s,
            None => return,
        };
        shared.stop.store(true, Ordering::SeqCst);
        if let Some(tid) = self.thread_id {
            // WM_QUIT an den Hook-Thread senden.
            unsafe {
                let _ = PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
        if !self.hhook.is_invalid() {
            unsafe {
                let _ = UnhookWindowsHookEx(self.hhook);
            }
            self.hhook = HHOOK::default();
        }
    }

    pub fn set_mode(&self, m: Mode) {
        if let Some(shared) = HOOK.get() {
            shared.mode.store(mode_to_u8(m), Ordering::SeqCst);
            log::info!("Hook-Modus -> {:?}", m);
        }
    }

    pub fn get_mode(&self) -> Mode {
        HOOK.get()
            .map(|s| mode_from_u8(s.mode.load(Ordering::SeqCst)))
            .unwrap_or(Mode::Block)
    }

    pub fn stats(&self) -> HookStats {
        match HOOK.get() {
            Some(s) => HookStats {
                seen_count: s.seen_count.load(Ordering::SeqCst),
                blocked_count: s.blocked_count.load(Ordering::SeqCst),
                shift_injected_count: s.shift_injected_count.load(Ordering::SeqCst),
            },
            None => HookStats {
                seen_count: 0,
                blocked_count: 0,
                shift_injected_count: 0,
            },
        }
    }
}

impl Default for KeyboardHook {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for KeyboardHook {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Pump-Funktion, laeuft im Hook-Thread.
fn run_pump(hhook_tx: std::sync::mpsc::Sender<(u64, u32)>) {
    let shared = HOOK.get().expect("HOOK init");

    // SetWindowsHookExW mit stabilem Callback. Wir leaken den Function-Pointer
    // bewusst -- er lebt bis Programmende (Hook-Thread laeuft eh so lange).
    let callback: HOOKPROC = Some(low_level_proc);
    let hhook = unsafe {
        SetWindowsHookExW(WH_KEYBOARD_LL, callback, None, 0)
    }
    .unwrap_or_else(|e| {
        log::error!("SetWindowsHookExW fehlgeschlagen: {e:?}");
        panic!("hook install failed");
    });

    // Thread-ID via Windows-API (stable API), im selben Thread geholt.
    let thread_id = unsafe { GetCurrentThreadId() };

    if hhook_tx.send((hhook.0 as u64, thread_id)).is_err() {
        unsafe {
            let _ = UnhookWindowsHookEx(hhook);
        }
        return;
    }

    let mut msg = MSG::default();
    loop {
        // GetMessageW blockiert bis eine Message kommt oder WM_QUIT.
        let ret = unsafe { GetMessageW(&mut msg, HWND(std::ptr::null_mut()), 0, 0) };
        // 0 = WM_QUIT, -1 = Fehler
        if matches!(ret.0, 0 | -1) {
            break;
        }
        if shared.stop.load(Ordering::SeqCst) {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }
    }

    unsafe {
        let _ = UnhookWindowsHookEx(hhook);
    }
    log::info!("Hook-Pump beendet");
}

/// WH_KEYBOARD_LL callback. Wird im Context des Hook-Threads aufgerufen.
unsafe extern "system" fn low_level_proc(
    ncode: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let Some(shared) = HOOK.get() else {
        return unsafe { CallNextHookEx(None, ncode, wparam, lparam) };
    };
    // HC_ACTION = 0
    if ncode != 0 {
        return unsafe { CallNextHookEx(None, ncode, wparam, lparam) };
    }
    // Re-Entrancy: selbst-gesendete Shift-Events ignorieren.
    if shared.injecting.load(Ordering::SeqCst) {
        return unsafe { CallNextHookEx(None, ncode, wparam, lparam) };
    }
    let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
    let wmsg = wparam.0 as u32;
    if kb.vkCode == VK_CAPITAL.0 as u32
        && (wmsg == WM_KEYDOWN || wmsg == WM_KEYUP)
    {
        shared.seen_count.fetch_add(1, Ordering::SeqCst);
        let mode = shared.mode.load(Ordering::SeqCst);
        if mode == MODE_BLOCK_U8 {
            shared.blocked_count.fetch_add(1, Ordering::SeqCst);
            return LRESULT(1); // Event geschluckt
        }
        if mode == MODE_SHIFT_U8 {
            let is_up = wmsg == WM_KEYUP;
            send_lshift(is_up, shared);
            shared.shift_injected_count.fetch_add(1, Ordering::SeqCst);
            return LRESULT(1); // Caps geschluckt, Shift wurde gesendet
        }
        // MODE_NORMAL: durchlassen
    }
    unsafe { CallNextHookEx(None, ncode, wparam, lparam) }
}

fn send_lshift(is_up: bool, shared: &HookShared) {
    let mut input = INPUT::default();
    input.r#type = INPUT_KEYBOARD;
    let mut ki = KEYBDINPUT::default();
    ki.wVk = VIRTUAL_KEY(VK_LSHIFT.0 as u16);
    ki.wScan = 0;
    ki.dwFlags = if is_up { KEYEVENTF_KEYUP } else { Default::default() };
    ki.time = 0;
    ki.dwExtraInfo = 0;
    // Setze ki-Feld in der Union.
    unsafe {
        let ki_ptr = &mut input.Anonymous.ki as *mut _ as *mut KEYBDINPUT;
        *ki_ptr = ki;
    }
    shared.injecting.store(true, Ordering::SeqCst);
    let result = unsafe {
        SendInput(&[input], std::mem::size_of::<INPUT>() as i32)
    };
    shared.injecting.store(false, Ordering::SeqCst);
    if result == 0 {
        log::warn!("SendInput(VK_LSHIFT) fehlgeschlagen");
    }
}

/// Aktueller CapsLock-Toggle-State (1 = an, 0 = aus).
pub fn capslock_state() -> bool {
    unsafe { (GetAsyncKeyState(VK_CAPITAL.0 as i32) & 1) != 0 }
}

/// Smoke-Test-Helfer: sendet ein VK_CAPITAL KeyDown+KeyUp via SendInput.
/// Wir nutzen das gleiche Re-Entrancy-Pattern wie der Hook selbst.
pub fn inject_capslock() {
    send_capslock_press();
    std::thread::sleep(std::time::Duration::from_millis(20));
    send_capslock_release();
}

fn send_capslock_press() {
    let mut input = INPUT::default();
    input.r#type = INPUT_KEYBOARD;
    let mut ki = KEYBDINPUT::default();
    ki.wVk = VIRTUAL_KEY(VK_CAPITAL.0);
    ki.wScan = 0;
    ki.dwFlags = Default::default();
    unsafe {
        let ki_ptr = &mut input.Anonymous.ki as *mut _ as *mut KEYBDINPUT;
        *ki_ptr = ki;
        let _ = SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
}

fn send_capslock_release() {
    let mut input = INPUT::default();
    input.r#type = INPUT_KEYBOARD;
    let mut ki = KEYBDINPUT::default();
    ki.wVk = VIRTUAL_KEY(VK_CAPITAL.0);
    ki.wScan = 0;
    ki.dwFlags = KEYEVENTF_KEYUP;
    unsafe {
        let ki_ptr = &mut input.Anonymous.ki as *mut _ as *mut KEYBDINPUT;
        *ki_ptr = ki;
        let _ = SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
}
