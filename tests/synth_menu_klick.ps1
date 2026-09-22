# Synthetischer Tray-Klick + Menu-Klick zum Test der Event-Pipeline.
$exe = 'D:\sysinfo\capslock_off_rs\target\release\capslock_off.exe'
$log = Join-Path $env:APPDATA 'capslock_off_rs\tray.log'
$ipc_path = Join-Path $env:APPDATA 'capslock_off_rs\ipc.json'

# Cleanup alte Logs (via mavis-trash oder ueberschreiben).
$ts = [int][double]::Parse((Get-Date -UFormat %s))
$logBackup = "$log.$ts"
if (Test-Path $log) { Move-Item $log $logBackup -Force }

# Tray starten.
$p = Start-Process -FilePath $exe -ArgumentList 'run' -PassThru -WindowStyle Hidden
Start-Sleep -Seconds 2
Write-Host "PID=$($p.Id)"

# Tray-icon-Window suchen.
Add-Type @"
using System;
using System.Runtime.InteropServices;

public class TrayT {
    public delegate bool EnumWindowsProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")]
    public static extern bool EnumWindows(EnumWindowsProc c, IntPtr l);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetClassNameW(IntPtr h, System.Text.StringBuilder s, int n);
    [DllImport("user32.dll")]
    public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")]
    public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
}
"@

$found = New-Object 'System.Collections.Generic.List[IntPtr]'
$cb = [TrayT+EnumWindowsProc]{
    param([IntPtr]$h, [IntPtr]$l)
    $sb = New-Object System.Text.StringBuilder 256
    [TrayT]::GetClassNameW($h, $sb, 256) | Out-Null
    if ($sb.ToString() -eq 'tray_icon_app') {
        $script:found.Add($h)
    }
    return $true
}
[TrayT]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
Write-Host "tray_icon_app Windows: $($found.Count)"
$tray_hwnd = $found | Select-Object -First 1

if (-not $tray_hwnd) {
    Write-Error "Kein tray-icon-Window gefunden"
    Stop-Process -Id $p.Id
    exit 1
}

# Schritt 1: WM_USER_TRAYICON+WM_RBUTTONDOWN -> TrackPopupMenu oeffnet.
Write-Host "Sende WM_USER_TRAYICON+WM_RBUTTONDOWN..."
# Wir nutzen PostMessage statt SendMessage, damit es nicht blockiert.
$null = [TrayT]::PostMessageW($tray_hwnd, 6002, [IntPtr]::Zero, [IntPtr]::new(0x0204))
Start-Sleep -Milliseconds 500

# Schritt 2: WM_COMMAND mit item_id=2 (mode_normal z.B.) an das tray-Window.
# muda Subclass verarbeitet WM_COMMAND und ruft MenuEvent::send auf.
# menu item IDs sind als String definiert ("mode_normal" etc.). In der
# Subclass wird das via find_by_id(string) aufgeloest.
# Item-IDs haben keine numerische ID per Default in muda -- sie sind Strings.
# Daher koennen wir WM_COMMAND nicht synthetisch senden (HIWORD/LOWORD ist numeric).
#
# Alternative: wir koennen die muda-Subclass direkt testen, indem wir
# TrackPopupMenu schliessen und dann einen MenuEvent manuell erzeugen.
# Aber wir haben keinen Zugriff auf muda-interne Sender.

# Wir versuchen stattdessen: ein zweiter WM_USER_TRAYICON+WM_RBUTTONDOWN
# triggert nochmal TrackPopupMenu. Wenn das Popup vom ersten Mal noch offen
# ist, schliesst Windows es automatisch (idempotent).
Write-Host "Sende zweiten Trigger..."
$null = [TrayT]::PostMessageW($tray_hwnd, 6002, [IntPtr]::Zero, [IntPtr]::new(0x0204))
Start-Sleep -Seconds 2

# Schritt 3: tray.log pruefen.
Write-Host "---tray.log:"
if (Test-Path $log) {
    Get-Content $log
} else {
    Write-Host "(kein Log!)"
}

# Tray beenden.
Stop-Process -Id $p.Id -Force
Start-Sleep -Seconds 1