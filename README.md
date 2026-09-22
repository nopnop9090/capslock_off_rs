<a href="https://notbyhumans.fyi">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://notbyhumans.fyi/badges/developed-paper.svg">
    <img src="https://notbyhumans.fyi/badges/developed-ink.svg" width="165" height="54" alt="Developed by AI, not by humans">
  </picture>
</a>

# capslock_off_rs

Systemweit (User-Session) die CapsLock-Taste deaktivieren oder als LeftShift
umbelegen — als native, single-file Windows-exe in Rust mit Tray-Icon und
programmatischer IPC-API für externe Tools.

## Was es tut

- `WH_KEYBOARD_LL`-Hook im eigenen Thread (Message-Pump, sonst entfernt
  Windows den Hook nach ~5 s). Drei Modi:
  - **Block** (default): CapsLock wird komplett tot gestellt.
  - **Shift Left**: CapsLock erzeugt VK_LSHIFT (Hold-Semantik, nicht Toggle).
  - **Normal**: Hook pass-through, CapsLock verhält sich normal.
- Tray-Icon mit Radio-Auswahl der drei Modi + persistenter Zustand in
  `%APPDATA%\capslock_off_rs\state.json`.
- Optionaler Autostart via HKCU `Run`-Key (kein Admin nötig).
- IPC-API für externe Tools: jede andere App kann den Modus abfragen oder
  setzen, ohne das Tray anzuklicken.
- Native exe (~1.4 MB), single-file, keine Python-Runtime / keine Subprozesse.

## Installation

ZIP aus dem [Release](#releases) entpacken, `capslock_off.exe` an einen
festen Pfad legen (z. B. `D:\Tools\`), optional als Autostart im Tray-Menü
anhaken.

Manuell:

```powershell
# Probe (3-Phasen-Smoke-Test) — kein Tray.
.\capslock_off.exe probe

# Tray + Hook.
.\capslock_off.exe run

# Version + Hilfe.
.\capslock_off.exe --version
.\capslock_off.exe --help
```

## Tray-Menü

Rechtsklick auf das Icon:

```
Status: Caps = block
─────────────────────
(o) Normal   (Caps ist normal)
(o) Blockiert  (Caps tot)            <-- default
(o) Shift Left (Caps = Shift)
─────────────────────
[x] Mit Windows starten
─────────────────────
Test: Caps-Event senden
─────────────────────
Beenden
```

Die Radio-Markierung `(o)` ist textuell als Prefix (tray-icon 0.19 hat keine
echte Checkmark-API). Der Autostart-Toggle zeigt `[x]` / `[ ]` aus dem
gleichen Grund.

## Persistenz

Modus wird in `%APPDATA%\capslock_off_rs\state.json` gespeichert
(atomar via tmp+rename). Beim Start automatisch geladen.

Discovery-Datei für externe IPC-Clients:

`%APPDATA%\capslock_off_rs\ipc.json`:

```json
{"hwnd": 1642770, "thread_id": 108256, "class": "capslock_off_rs_msg_window_v1"}
```

`hwnd` ist das unsichtbare `HWND_MESSAGE`-Window — taucht nicht in
`FindWindow` / `EnumWindows` auf. Externe Tools lesen HWND + Thread-ID
aus dieser Datei.

## IPC-API (cross-process, externe Tools)

PowerShell-Beispiel (siehe `tests/ipc_test.ps1` für eine voll lauffähige
Implementierung):

```powershell
# 1. Discovery: HWND laden.
$disc = Get-Content "$env:APPDATA\capslock_off_rs\ipc.json" | ConvertFrom-Json
$hwnd = [IntPtr]::new([int64]$disc.hwnd)

# 2. Shared-Memory + AutoReset-Event mit zufälligem Namen anlegen.
$guid = [Guid]::NewGuid().ToString("N")
$shm = [MemoryMappedFile]::CreateNew("Local\capslock_off_rs_resp_$guid", 128)
$evt = [EventWaitHandle]::new($false, "AutoReset", "Local\capslock_off_rs_done_$guid")

# 3. IpcRequest in einen Buffer schreiben (Layout siehe unten).
$req = New-Object byte[] 264
[BitConverter]::GetBytes([uint32]1).CopyTo($req, 0)         # cmd = GET_MODE
[BitConverter]::GetBytes([uint32]0).CopyTo($req, 4)         # param
[Text.Encoding]::Unicode.GetBytes("Local\capslock_off_rs_resp_$guid`0").CopyTo($req, 8)
[Text.Encoding]::Unicode.GetBytes("Local\capslock_off_rs_done_$guid`0").CopyTo($req, 136)

# 4. WM_COPYDATA an das IPC-Hwnd senden (blockierend).
# (Siehe tests/ipc_test.ps1 für die COPYDATASTRUCT-Konstruktion.)

# 5. Auf Event warten, dann IpcResponse aus Shared-Memory lesen.
$evt.WaitOne(5000) | Out-Null
# ... siehe ipc_test.ps1 ...
```

### Layouts

`IpcRequest` (264 Bytes, im lpData von WM_COPYDATA):

| Offset | Typ    | Feld          | Bedeutung                            |
|--------|--------|---------------|--------------------------------------|
| 0      | u32 LE | `cmd`         | 1=GET_MODE 2=SET_MODE 3=GET_VERSION 4=GET_STATS |
| 4      | u32 LE | `param`       | für SET_MODE: 0=normal 1=block 2=shift |
| 8      | u16×64 | `shmem_name`  | null-terminated UTF-16-LE             |
| 136    | u16×64 | `event_name`  | null-terminated UTF-16-LE             |

`IpcResponse` (128 Bytes, im Shared-Memory):

| Offset | Typ    | Feld       | Bedeutung                                |
|--------|--------|------------|------------------------------------------|
| 0      | u32 LE | `status`   | 0=ok, 1=unknown cmd                       |
| 4      | u8×124 | `payload`  | cmd-abhängig (siehe unten)                |

Payload-Inhalt:

- `GET_MODE`: UTF-16-LE String inkl. Null-Terminator (`"block"` = 12 Bytes)
- `GET_VERSION`: UTF-16-LE String (`"0.1.0"` = 12 Bytes)
- `SET_MODE`: leer
- `GET_STATS`: `IpcStats` struct (32 Bytes), Layout:
  - `u32 mode`, `u32 seen_count`, `u32 blocked_count`,
  - `u32 shift_injected_count`, `u32 caps_toggle_state`,
  - `u32 _reserved[3]`

### Warum nicht WM_USER + Pointer-Args?

`SendMessage` überträgt `lParam`-Pointer nicht in den Empfänger-Adressraum
hinüber — der Buffer eines PowerShell-Skripts würde im Rust-Prozess als
zufälliger Speicher interpretiert (siehe `tests/ipc_test.ps1` — der erste
Versuch lieferte Daten aus `PATH`-Umgebungsvariablen, weil die Adresse in
den falschen Prozess gemappt wurde).

`WM_COPYDATA` kopiert `lpData` automatisch Cross-Process. Die Antwort
kommt über Shared-Memory + Event zurück, weil `WM_COPYDATA` One-Way ist.

## CLI

```
capslock_off [run]                # Tray + Hook (default).
capslock_off probe                # 3-Phasen-Smoke-Test (alle Modi), exit 0/1.

--debug                           # DEBUG-Logging.
--autostart                       # Marker: via HKCU Run-Key gestartet.
-h, --help
-V, --version
```

`probe` durchläuft die drei Modi (Block → Shift → Normal), injiziert pro
Phase ein VK_CAPITAL-Event via `SendInput` und prüft die Counter im Hook.
Exit 0 wenn alle Phasen ok.

## Build (für Entwickler)

Voraussetzungen: Rust 1.75+, MSVC-Buildtools.

```powershell
git clone https://github.com/nopnop9090/capslock_off_rs
cd capslock_off_rs
cargo build --release
# -> target/release/capslock_off.exe  (~1.4 MB)
```

Profile-Setup: `opt-level=3`, `lto=fat`, `codegen-units=1`, `strip=true`,
`panic=abort` (kleinste Binary).

## Bekannte Limitierungen

- HWND_MESSAGE-Windows tauchen in `FindWindow` / `EnumWindows` nicht auf —
  externe Tools MÜSSEN die Discovery-Datei lesen.
- IPC-Discovery-Datei und Shared-Memory werden **nicht** durch Process-Tokens
  geschützt — wer den Pfad kennt, kann lesen / schreiben. Akzeptabel für
  ein Single-User-Tool.
- "Shift"-Modus setzt CapsLock-Toggle nicht zurück. Wer vorher CapsLock
  an hatte und dann Shift wählt, behält das Toggle (Shift ist nur eine
  semantische Umdeutung).
- Re-Entrancy-Schutz für `SendInput(VK_LSHIFT)` ist über ein
  `injecting`-Flag realisiert — funktioniert für die normale CapsLock-
  Hook-Sequenz, aber sehr schnelles CapsLock-Tippen während ein Shift
  gerade synthetisiert wird, kann in seltenen Fällen doppelt zünden.

## Lizenz

MIT — siehe [LICENSE](LICENSE).