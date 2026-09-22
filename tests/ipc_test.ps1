# Cross-Process-IPC-Test fuer capslock_off_rs.
# Pattern: Shared-Memory + AutoReset-Event + WM_COPYDATA.
#
# 1. Liest Discovery-Datei %APPDATA%\capslock_off_rs\ipc.json (hwnd + thread_id).
# 2. Erstellt benannten Shared-Memory (128 Bytes) + AutoReset-Event mit Random-GUID.
# 3. Schreibt IpcRequest {cmd, param, shmem_name, event_name} in lpData.
# 4. Sendet WM_COPYDATA an Tray-Hwnd (blockierend).
# 5. Wartet auf Event (Timeout 5s).
# 6. Liest IpcResponse aus Shared-Memory.

$ErrorActionPreference = 'Stop'

$discPath = Join-Path $env:APPDATA "capslock_off_rs\ipc.json"
if (-not (Test-Path $discPath)) {
    Write-Error "Discovery-Datei nicht gefunden: $discPath"
    exit 1
}
$disc = Get-Content $discPath -Raw | ConvertFrom-Json
Write-Host "Discovery: hwnd=$($disc.hwnd) thread_id=$($disc.thread_id) class=$($disc.class)"

Add-Type @"
using System;
using System.Runtime.InteropServices;

public class CapsIpc {
    [DllImport("user32.dll", CharSet = CharSet.Auto, SetLastError = true)]
    public static extern IntPtr SendMessage(IntPtr hWnd, uint Msg, IntPtr wParam, ref COPYDATASTRUCT lParam);

    public const uint WM_COPYDATA = 0x004A;
}

[StructLayout(LayoutKind.Sequential)]
public struct COPYDATASTRUCT {
    public IntPtr dwData;
    public int cbData;
    public IntPtr lpData;
}
"@

# IpcRequest layout (siehe Rust src/ipc.rs):
#   u32  cmd
#   u32  param
#   u16  shmem_name[64]  (128 bytes)
#   u16  event_name[64]  (128 bytes)
# Total: 4 + 4 + 128 + 128 = 264 bytes.
$reqSize = 264
$respSize = 128

function Send-IpcRequest {
    param(
        [IntPtr]$hwnd,
        [uint32]$cmd,
        [uint32]$param
    )

    $guid = [Guid]::NewGuid().ToString("N")
    $shmName = "Local\capslock_off_rs_resp_$guid"
    $evtName = "Local\capslock_off_rs_done_$guid"

    # Shared-Memory (Sender erstellt).
    $shm = [System.IO.MemoryMappedFiles.MemoryMappedFile]::CreateNew(
        $shmName, $respSize)
    $stream = $shm.CreateViewStream()

    # AutoReset-Event (Sender erstellt, initial non-signaled).
    $evt = [System.Threading.EventWaitHandle]::new(
        $false,
        [System.Threading.EventResetMode]::AutoReset,
        $evtName
    )

    # Request allokieren (gepinnt fuer WM_COPYDATA).
    $reqBuf = New-Object byte[] $reqSize
    $gch = [System.Runtime.InteropServices.GCHandle]::Alloc(
        $reqBuf, [System.Runtime.InteropServices.GCHandleType]::Pinned)
    $reqPtr = $gch.AddrOfPinnedObject()

    # cmd + param (little-endian, x64).
    [System.BitConverter]::GetBytes([uint32]$cmd).CopyTo($reqBuf, 0)
    [System.BitConverter]::GetBytes([uint32]$param).CopyTo($reqBuf, 4)
    # shmem_name als UTF-16LE bei Offset 8.
    $shmBytes = [System.Text.Encoding]::Unicode.GetBytes($shmName + "`0")
    [System.Buffer]::BlockCopy($shmBytes, 0, $reqBuf, 8,
        [Math]::Min($shmBytes.Length, 128))
    # event_name als UTF-16LE bei Offset 136.
    $evtBytes = [System.Text.Encoding]::Unicode.GetBytes($evtName + "`0")
    [System.Buffer]::BlockCopy($evtBytes, 0, $reqBuf, 136,
        [Math]::Min($evtBytes.Length, 128))

    # COPYDATASTRUCT (gepinnt). Layout x64:
    #   ULONG_PTR dwData   (Offset 0,  8 bytes)
    #   DWORD     cbData   (Offset 8,  4 bytes)
    #   <padding>          (Offset 12, 4 bytes)
    #   PVOID     lpData   (Offset 16, 8 bytes)
    # Total: 24 bytes.
    $cdsBuf = New-Object byte[] 24
    $cdsGch = [System.Runtime.InteropServices.GCHandle]::Alloc(
        $cdsBuf, [System.Runtime.InteropServices.GCHandleType]::Pinned)
    $cdsPtr = $cdsGch.AddrOfPinnedObject()

    [System.BitConverter]::GetBytes([IntPtr]::Zero).CopyTo($cdsBuf, 0)        # dwData = 0
    [System.BitConverter]::GetBytes([int32]$reqSize).CopyTo($cdsBuf, 8)        # cbData
    # 4 bytes padding bei Offset 12 bleiben 0
    [System.BitConverter]::GetBytes([Int64]$reqPtr.ToInt64()).CopyTo($cdsBuf, 16) # lpData

    $cds = [System.Runtime.InteropServices.Marshal]::PtrToStructure(
        $cdsPtr, [Type]([COPYDATASTRUCT]))

    # SendMessage (blockierend, wartet auf Rueckkehr).
    [void][CapsIpc]::SendMessage($hwnd, [CapsIpc]::WM_COPYDATA, [IntPtr]::Zero, [ref]$cds)

    # Auf Antwort-Event warten.
    if (-not $evt.WaitOne(5000)) {
        Write-Warning "Timeout bei cmd=$cmd"
        $gch.Free(); $cdsGch.Free()
        $shm.Dispose(); $evt.Dispose()
        return $null
    }

    # Response lesen.
    $respBuf = New-Object byte[] $respSize
    $stream.Read($respBuf, 0, $respSize) | Out-Null

    $gch.Free(); $cdsGch.Free()
    $shm.Dispose(); $evt.Dispose()

    return $respBuf
}

$hwnd = [IntPtr]::new([int64]$disc.hwnd)

# 1) GET_MODE
$resp = Send-IpcRequest $hwnd 1 0
$modeText = [System.Text.Encoding]::Unicode.GetString(
    $resp[4..($resp.Length - 1)]).TrimEnd([char]0).Split([char]0)[0]
Write-Host "GET_MODE -> $modeText"

# 2) GET_VERSION
$resp = Send-IpcRequest $hwnd 3 0
$versionText = [System.Text.Encoding]::Unicode.GetString(
    $resp[4..($resp.Length - 1)]).TrimEnd([char]0).Split([char]0)[0]
Write-Host "GET_VERSION -> $versionText"

# 3) GET_STATS -- payload ab Offset 4 ist IpcStats (32 bytes: 8 x u32).
$resp = Send-IpcRequest $hwnd 4 0
$statsOffset = 4
$modeVal     = [System.BitConverter]::ToUInt32($resp, $statsOffset + 0)
$seenVal     = [System.BitConverter]::ToUInt32($resp, $statsOffset + 4)
$blockedVal  = [System.BitConverter]::ToUInt32($resp, $statsOffset + 8)
$shiftInjVal = [System.BitConverter]::ToUInt32($resp, $statsOffset + 12)
$capsVal     = [System.BitConverter]::ToUInt32($resp, $statsOffset + 16)
Write-Host ("GET_STATS -> mode={0} seen={1} blocked={2} shift_inj={3} caps_toggle={4}" -f `
    $modeVal, $seenVal, $blockedVal, $shiftInjVal, $capsVal)

# 4) SET_MODE wparam=2 (shift)
$resp = Send-IpcRequest $hwnd 2 2
Write-Host "SET_MODE(2) -> status=$([System.BitConverter]::ToUInt32($resp, 0))"

Start-Sleep -Milliseconds 200

# 6) Verify GET_MODE
$resp = Send-IpcRequest $hwnd 1 0
$modeText2 = [System.Text.Encoding]::Unicode.GetString(
    $resp[4..($resp.Length - 1)]).TrimEnd([char]0).Split([char]0)[0]
Write-Host "GET_MODE after set -> $modeText2"

# 7) SET_MODE wparam=0 (normal) zurueck
$resp = Send-IpcRequest $hwnd 2 0
Write-Host "SET_MODE(0) -> status=$([System.BitConverter]::ToUInt32($resp, 0))"

Start-Sleep -Milliseconds 200

# 8) Verify final mode
$resp = Send-IpcRequest $hwnd 1 0
$modeText3 = [System.Text.Encoding]::Unicode.GetString(
    $resp[4..($resp.Length - 1)]).TrimEnd([char]0).Split([char]0)[0]
Write-Host "GET_MODE final -> $modeText3"

Write-Host "IPC_TEST_OK"