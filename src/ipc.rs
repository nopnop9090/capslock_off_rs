//! Hidden Window + Cross-Process IPC via WM_COPYDATA + Shared-Memory + Event.
//!
//! Erstellt ein unsichtbares Message-Only-Window mit eindeutigem Class-Namen.
//! Externe Tools koennen via `WM_COPYDATA` Anfragen schicken, der Empfänger
//! schreibt die Antwort in einen benannten Shared-Memory und signalisiert
//! ein benanntes Event.
//!
//! ## Warum nicht `WM_USER + ptr`?
//!
//! `SendMessage` ueber Prozessgrenzen hinweg uebertraegt Pointer-Argumente
//! NICHT in den Empfaenger-Adressraum. Ein in PowerShell allokierter Buffer
//! wuerde im Rust-Prozess als zufaelliger Speicher interpretiert.
//! `WM_COPYDATA` kopiert das lpData automatisch Cross-Process, aber damit
//! ist nur One-Way (Request) machbar -- die Antwort muss ueber einen
//! Out-Of-Band-Mechanismus zurueckkommen. Hier: Shared-Memory + Event.
//!
//! ## API (vom Sender aus gesehen)
//!
//! 1. PowerShell erstellt:
//!    - Shared-Memory `Local\capslock_off_rs_resp_<random>` (128 Bytes)
//!    - AutoReset-Event `Local\capslock_off_rs_done_<random>`
//! 2. Schreibt `IpcRequest` (cmd, param, shmem_name, event_name) in den
//!    lpData-Buffer.
//! 3. `SendMessage(ipc_hwnd, WM_COPYDATA, 0, &CDS{...})`.
//! 4. Wartet auf Event (mit Timeout).
//! 5. Liest `IpcResponse` aus Shared-Memory.
//!
//! ## IPC-Discovery
//!
//! Der Empfänger schreibt `ipc.json` in `%APPDATA%\capslock_off_rs\` mit
//! HWND + Thread-ID des IPC-Windows. Externe Tools koennen den HWND
//! daraus lesen (HWND_MESSAGE-Windows tauchen weder in FindWindow noch
//! EnumWindows auf).
//!
//! Sicherheit: Single-User, kein Process-Token-Check. Wer die Class kennt
//! und einen HWND hat, kann den Mode aendern. Akzeptabel fuer ein
//! Single-User-Tool ohne Multi-Tenant-Ansprueche.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::thread::{self, JoinHandle};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::Memory::{
    MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
};
use windows::Win32::System::Threading::{
    GetCurrentThreadId, OpenEventW, SetEvent, SYNCHRONIZATION_SYNCHRONIZE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, PostMessageW,
    PostThreadMessageW, TranslateMessage, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, HWND_MESSAGE,
    MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WM_COPYDATA, WM_QUIT, WM_USER,
};

use crate::hook::HookStats;
use crate::state::Mode;

pub const WINDOW_CLASS: &str = "capslock_off_rs_msg_window_v1";

// Public WM_USER-Messages (In-Process, optional, ohne Shared-Memory).
pub const MSG_GET_MODE: u32 = WM_USER + 1;
pub const MSG_SET_MODE: u32 = WM_USER + 2;
pub const MSG_GET_VERSION: u32 = WM_USER + 3;
pub const MSG_GET_STATS: u32 = WM_USER + 4;
pub const MSG_MODE_CHANGED: u32 = WM_USER + 5;

pub const MODE_BUFFER_LEN: usize = 32;

// Cross-Process-IPC commands (ueber WM_COPYDATA).
pub const IPC_CMD_GET_MODE: u32 = 1;
pub const IPC_CMD_SET_MODE: u32 = 2;
pub const IPC_CMD_GET_VERSION: u32 = 3;
pub const IPC_CMD_GET_STATS: u32 = 4;

/// C-compatible Stats-Struct (32 Bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IpcStats {
    pub mode: u32,
    pub seen_count: u32,
    pub blocked_count: u32,
    pub shift_injected_count: u32,
    pub caps_toggle_state: u32,
    pub _reserved: [u32; 3],
}

impl IpcStats {
    pub const SIZE: usize = std::mem::size_of::<Self>();
}

/// Cross-Process-Request: steht in `WM_COPYDATA.cds.lpData`.
/// cbData = sizeof(IpcRequest). Strings als fixed-size UTF-16.
#[repr(C)]
pub struct IpcRequest {
    pub cmd: u32,                       // IPC_CMD_*
    pub param: u32,                     // fuer SET_MODE: 0=normal, 1=block, 2=shift
    pub shmem_name: [u16; 64],          // null-terminated UTF-16
    pub event_name: [u16; 64],          // null-terminated UTF-16
}

impl IpcRequest {
    pub const SIZE: usize = std::mem::size_of::<Self>();

    pub fn shmem_name_str(&self) -> String {
        u16_array_to_string(&self.shmem_name)
    }
    pub fn event_name_str(&self) -> String {
        u16_array_to_string(&self.event_name)
    }
}

/// Cross-Process-Response: steht im Shared-Memory (128 Bytes).
#[repr(C)]
pub struct IpcResponse {
    pub status: u32,         // 0 = ok, sonst Fehlercode
    pub payload: [u8; 124],  // cmd-spezifisch (mode: UTF-16, version: UTF-16, stats: IpcStats, set: leer)
}

impl IpcResponse {
    pub const SIZE: usize = std::mem::size_of::<Self>();

    pub fn mode_string(&self) -> Option<String> {
        if self.status != 0 {
            return None;
        }
        let wide: Vec<u16> = self.payload[..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&w| w != 0)
            .collect();
        Some(String::from_utf16_lossy(&wide))
    }
}

fn u16_array_to_string(arr: &[u16]) -> String {
    let len = arr.iter().position(|&w| w == 0).unwrap_or(arr.len());
    String::from_utf16_lossy(&arr[..len])
}

fn wide_to_array(s: &str) -> [u16; 64] {
    let mut arr = [0u16; 64];
    for (i, w) in s.encode_utf16().take(63).enumerate() {
        arr[i] = w;
    }
    arr
}

/// Liefert den Pfad zur IPC-Discovery-Datei (HWND + Thread-ID).
pub fn discovery_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(Path::new(&appdata).join("capslock_off_rs").join("ipc.json"))
}

/// Schreibt die Discovery-Datei (hwnd + thread_id).
pub fn write_discovery(hwnd_raw: u64, thread_id: u32) {
    let Some(path) = discovery_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let payload = format!(
        "{{\"hwnd\": {}, \"thread_id\": {}, \"class\": \"{}\"}}",
        hwnd_raw, thread_id, WINDOW_CLASS
    );
    if let Err(e) = std::fs::write(&path, payload) {
        log::warn!("discovery-file schreiben fehlgeschlagen: {e}");
    }
}

/// Loescht die Discovery-Datei (beim Beenden).
pub fn clear_discovery() {
    if let Some(path) = discovery_path() {
        let _ = std::fs::remove_file(&path);
    }
}

type ModeGetter = Arc<dyn Fn() -> Mode + Send + Sync>;
type ModeSetter = Arc<dyn Fn(Mode) + Send + Sync>;
type VersionGetter = Arc<dyn Fn() -> String + Send + Sync>;
type StatsGetter = Arc<dyn Fn() -> (Mode, HookStats) + Send + Sync>;

pub struct IpcServer {
    thread: Option<JoinHandle<()>>,
    thread_id: Option<u32>,
    hwnd_raw: u64,
    get_mode: ModeGetter,
    set_mode: ModeSetter,
    get_version: VersionGetter,
    get_stats: StatsGetter,
}

unsafe impl Send for IpcServer {}
unsafe impl Sync for IpcServer {}

impl IpcServer {
    pub fn new(
        get_mode: ModeGetter,
        set_mode: ModeSetter,
        get_version: VersionGetter,
        get_stats: StatsGetter,
    ) -> Self {
        Self {
            thread: None,
            thread_id: None,
            hwnd_raw: 0,
            get_mode,
            set_mode,
            get_version,
            get_stats,
        }
    }

    pub fn start(&mut self) -> windows::core::Result<()> {
        if self.thread.is_some() {
            return Ok(());
        }
        let (tx, rx) = mpsc::channel::<(u64, u32)>();
        let get_mode = self.get_mode.clone();
        let set_mode = self.set_mode.clone();
        let get_version = self.get_version.clone();
        let get_stats = self.get_stats.clone();

        let handle = thread::Builder::new()
            .name("capslock-ipc".into())
            .spawn(move || {
                run_message_window(tx, get_mode, set_mode, get_version, get_stats);
            })
            .map_err(|e| windows::core::Error::new(windows::core::HRESULT(-1), e.to_string()))?;
        let (hwnd_raw, thread_id) = rx.recv().map_err(|_| {
            windows::core::Error::new(windows::core::HRESULT(-2), "ipc init failed")
        })?;
        if hwnd_raw == 0 {
            return Err(windows::core::Error::new(
                windows::core::HRESULT(-3),
                "ipc window creation failed",
            ));
        }
        self.thread_id = Some(thread_id);
        self.thread = Some(handle);
        self.hwnd_raw = hwnd_raw;
        log::info!("IPC-Window erstellt: hwnd=0x{hwnd_raw:x}");
        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(tid) = self.thread_id {
            unsafe {
                let _ = PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
        self.thread_id = None;
        self.hwnd_raw = 0;
    }

    /// Postet MODE_CHANGED an unser eigenes Window (In-Process-Notification).
    pub fn broadcast_mode_change(&self, mode: Mode) {
        let hwnd_raw = STATE_HWND.load(Ordering::SeqCst);
        if hwnd_raw == 0 {
            return;
        }
        let hwnd = HWND(hwnd_raw as *mut std::ffi::c_void);
        let wparam = WPARAM(mode_to_u8(mode) as usize);
        unsafe {
            let _ = PostMessageW(hwnd, MSG_MODE_CHANGED, wparam, LPARAM(0));
        }
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn mode_to_u8(m: Mode) -> u8 {
    match m {
        Mode::Normal => 0,
        Mode::Block => 1,
        Mode::Shift => 2,
    }
}
fn mode_from_u8(v: u32) -> Mode {
    match v {
        2 => Mode::Shift,
        1 => Mode::Block,
        _ => Mode::Normal,
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

static CLASS_REGISTERED: AtomicBool = AtomicBool::new(false);
static STATE_HWND: AtomicU64 = AtomicU64::new(0);
static STATE: OnceLock<Arc<WindowState>> = OnceLock::new();

fn run_message_window(
    hwnd_tx: mpsc::Sender<(u64, u32)>,
    get_mode: ModeGetter,
    set_mode: ModeSetter,
    get_version: VersionGetter,
    get_stats: StatsGetter,
) {
    let class_name = wide(WINDOW_CLASS);
    let h_instance = unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }
        .unwrap_or_default();

    let proc_ptr = window_proc as usize;

    if !CLASS_REGISTERED.swap(true, Ordering::SeqCst) {
        unsafe {
            let wc = windows::Win32::UI::WindowsAndMessaging::WNDCLASSEXW {
                cbSize: std::mem::size_of::<windows::Win32::UI::WindowsAndMessaging::WNDCLASSEXW>()
                    as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(std::mem::transmute::<
                    usize,
                    unsafe extern "system" fn(
                        HWND,
                        u32,
                        WPARAM,
                        LPARAM,
                    ) -> LRESULT,
                >(proc_ptr)),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: h_instance.into(),
                hIcon: Default::default(),
                hCursor: Default::default(),
                hbrBackground: Default::default(),
                lpszMenuName: PCWSTR(std::ptr::null()),
                lpszClassName: PCWSTR(class_name.as_ptr()),
                hIconSm: Default::default(),
            };
            let atom = windows::Win32::UI::WindowsAndMessaging::RegisterClassExW(&wc);
            if atom == 0 {
                log::error!("RegisterClassExW fehlgeschlagen");
                let _ = hwnd_tx.send((0, 0));
                return;
            }
        }
    }

    let state = Arc::new(WindowState {
        get_mode,
        set_mode,
        get_version,
        get_stats,
    });
    let _ = STATE.set(state);

    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(wide("capslock_off_rs_msg_window").as_ptr()),
            WINDOW_STYLE::default(),
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            HWND_MESSAGE,
            None,
            h_instance,
            None,
        )
    };
    let Ok(hwnd) = hwnd else {
        log::error!("CreateWindowExW fehlgeschlagen");
        let _ = hwnd_tx.send((0, 0));
        return;
    };
    log::info!("IPC-Window HWND={:?}", hwnd);
    STATE_HWND.store(hwnd.0 as u64, Ordering::SeqCst);
    let thread_id = unsafe { GetCurrentThreadId() };
    write_discovery(hwnd.0 as u64, thread_id);
    let _ = hwnd_tx.send((hwnd.0 as u64, thread_id));

    let mut msg = MSG::default();
    loop {
        let ret = unsafe { GetMessageW(&mut msg, HWND(std::ptr::null_mut()), 0, 0) };
        if matches!(ret.0, 0 | -1) {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }
    }

    STATE_HWND.store(0, Ordering::SeqCst);
    clear_discovery();
    let _ = unsafe { DestroyWindow(hwnd) };
    log::info!("IPC-Message-Loop beendet");
}

struct WindowState {
    get_mode: ModeGetter,
    set_mode: ModeSetter,
    get_version: VersionGetter,
    get_stats: StatsGetter,
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let Some(state) = STATE.get() else {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };
    match msg {
        WM_COPYDATA => {
            log::debug!("WM_COPYDATA empfangen");
            let cds = unsafe { &*(lparam.0 as *const COPYDATASTRUCT) };
            log::debug!("WM_COPYDATA cbData={} expected={}", cds.cbData, IpcRequest::SIZE);
            if cds.cbData as usize != IpcRequest::SIZE {
                log::warn!("WM_COPYDATA mit falscher Groesse: {}", cds.cbData);
                return LRESULT(0);
            }
            let req = unsafe { &*(cds.lpData as *const IpcRequest) };
            log::debug!("WM_COPYDATA cmd={} param={} shmem={:?}", req.cmd, req.param, req.shmem_name_str());
            handle_ipc_request(state, req);
            LRESULT(0)
        }
        // In-Process-WM_USER-Messages (Pointer-Args, kein Cross-Process)
        x if x == MSG_GET_MODE => {
            let mode = (state.get_mode)();
            copy_mode_to_buffer(mode, lparam);
            LRESULT(0)
        }
        x if x == MSG_SET_MODE => {
            let new_mode = mode_from_u8(wparam.0 as u32);
            (state.set_mode)(new_mode);
            LRESULT(0)
        }
        x if x == MSG_GET_VERSION => {
            let v = (state.get_version)();
            copy_string_to_buffer(&v, lparam, MODE_BUFFER_LEN);
            LRESULT(0)
        }
        x if x == MSG_GET_STATS => {
            let (mode, stats) = (state.get_stats)();
            let caps_state = crate::hook::capslock_state() as u32;
            let ipc_stats = IpcStats {
                mode: mode_to_u8(mode) as u32,
                seen_count: stats.seen_count,
                blocked_count: stats.blocked_count,
                shift_injected_count: stats.shift_injected_count,
                caps_toggle_state: caps_state,
                _reserved: [0; 3],
            };
            unsafe {
                let dst = lparam.0 as *mut u8;
                std::ptr::copy_nonoverlapping(
                    &ipc_stats as *const IpcStats as *const u8,
                    dst,
                    IpcStats::SIZE,
                );
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Behandelt einen Cross-Process-IPC-Request: oeffnet das im Request
/// angegebene Shared-Memory + Event, schreibt die Antwort, signalisiert.
fn handle_ipc_request(state: &Arc<WindowState>, req: &IpcRequest) {
    let shmem_name = req.shmem_name_str();
    let event_name = req.event_name_str();

    // Response erstellen.
    let response = build_response(state, req.cmd, req.param);

    // Shared-Memory oeffnen und Antwort reinschreiben.
    let shmem_w: Vec<u16> = shmem_name
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let map_handle = unsafe {
        OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, PCWSTR(shmem_w.as_ptr()))
    }
    .unwrap_or(HANDLE(std::ptr::null_mut()));
    if map_handle.is_invalid() {
        log::warn!("OpenFileMappingW fehlgeschlagen fuer {}", shmem_name);
        return;
    }
    let view = unsafe {
        MapViewOfFile(map_handle, FILE_MAP_ALL_ACCESS, 0, 0, IpcResponse::SIZE)
    };
    if view.Value.is_null() {
        log::warn!("MapViewOfFile fehlgeschlagen");
        let _ = unsafe { CloseHandle(map_handle) };
        return;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(
            &response as *const IpcResponse as *const u8,
            view.Value as *mut u8,
            IpcResponse::SIZE,
        );
        let _ = UnmapViewOfFile(view);
    }
    let _ = unsafe { CloseHandle(map_handle) };

    // Event signalisieren.
    let event_w: Vec<u16> = event_name
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let event_handle = unsafe {
        OpenEventW(
            windows::Win32::System::Threading::EVENT_MODIFY_STATE
                | SYNCHRONIZATION_SYNCHRONIZE,
            false,
            PCWSTR(event_w.as_ptr()),
        )
    }
    .unwrap_or(HANDLE(std::ptr::null_mut()));
    if !event_handle.is_invalid() {
        unsafe {
            let _ = SetEvent(event_handle);
            let _ = CloseHandle(event_handle);
        }
    } else {
        log::warn!("OpenEventW fehlgeschlagen fuer {}", event_name);
    }
}

fn build_response(state: &WindowState, cmd: u32, param: u32) -> IpcResponse {
    let mut resp = IpcResponse {
        status: 0,
        payload: [0u8; 124],
    };
    // Max 62 UTF-16-LE Chars (124 Bytes) + Null-Terminator.
    const MAX_WIDE_CHARS: usize = 61;
    match cmd {
        IPC_CMD_GET_MODE => {
            let mode = (state.get_mode)();
            let wide: Vec<u16> = mode
                .as_str()
                .encode_utf16()
                .take(MAX_WIDE_CHARS)
                .chain(std::iter::once(0))
                .collect();
            let bytes: &[u8] =
                unsafe { std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2) };
            resp.payload[..bytes.len()].copy_from_slice(bytes);
        }
        IPC_CMD_SET_MODE => {
            let new_mode = mode_from_u8(param);
            (state.set_mode)(new_mode);
            // Kein Payload.
        }
        IPC_CMD_GET_VERSION => {
            let v = (state.get_version)();
            let wide: Vec<u16> = v
                .encode_utf16()
                .take(MAX_WIDE_CHARS)
                .chain(std::iter::once(0))
                .collect();
            let bytes: &[u8] =
                unsafe { std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2) };
            resp.payload[..bytes.len()].copy_from_slice(bytes);
        }
        IPC_CMD_GET_STATS => {
            let (mode, stats) = (state.get_stats)();
            let caps_state = crate::hook::capslock_state() as u32;
            let ipc_stats = IpcStats {
                mode: mode_to_u8(mode) as u32,
                seen_count: stats.seen_count,
                blocked_count: stats.blocked_count,
                shift_injected_count: stats.shift_injected_count,
                caps_toggle_state: caps_state,
                _reserved: [0; 3],
            };
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    &ipc_stats as *const IpcStats as *const u8,
                    IpcStats::SIZE,
                )
            };
            resp.payload[..bytes.len()].copy_from_slice(bytes);
        }
        _ => {
            resp.status = 1; // unknown command
        }
    }
    resp
}

fn copy_mode_to_buffer(mode: Mode, lparam: LPARAM) {
    copy_string_to_buffer(mode.as_str(), lparam, MODE_BUFFER_LEN);
}

fn copy_string_to_buffer(s: &str, lparam: LPARAM, max_chars: usize) {
    let wide: Vec<u16> = s
        .encode_utf16()
        .take(max_chars - 1)
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let dst = lparam.0 as *mut u16;
        std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
    }
}