# Sendet einen synthetischen Rechtsklick an das tray-icon-Window und
# beobachtet das tray.log.
$exe = 'D:\sysinfo\capslock_off_rs\target\release\capslock_off.exe'
$log = Join-Path $env:APPDATA 'capslock_off_rs\tray.log'

# Cleanup alte Logs.
Remove-Item $log -ErrorAction SilentlyContinue

# Tray starten.
$p = Start-Process -FilePath $exe -ArgumentList 'run' -PassThru -WindowStyle Hidden
Start-Sleep -Seconds 2
Write-Host "PID=$($p.Id)"

# IPC-Discovery lesen.
$disc = Get-Content (Join-Path $env:APPDATA 'capslock_off_rs\ipc.json') | ConvertFrom-Json
Write-Host "IPC hwnd=$($disc.hwnd)"

# Tray-icon-Window via EnumWindows suchen.
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

# Synthetischen WM_USER_TRAYICON (6002) + WM_RBUTTONDOWN (0x0204) senden.
Write-Host "Sende WM_USER_TRAYICON+WM_RBUTTONDOWN an $tray_hwnd..."
$ret = [TrayT]::SendMessageW($tray_hwnd, 6002, [IntPtr]::Zero, [IntPtr]::new(0x0204))
Write-Host "SendMessage result: $ret"

# Wait + check log.
Start-Sleep -Seconds 2
Write-Host "---tray.log:"
Get-Content $log

# Tray beenden via WM_QUIT an Tray-Window.
[void][TrayT]::SendMessageW($tray_hwnd, 0x0012, [IntPtr]::Zero, [IntPtr]::Zero)
Start-Sleep -Seconds 1

# Cleanup: ggf. Kill.
if (Get-Process -Id $p.Id -ErrorAction SilentlyContinue) {
    Stop-Process -Id $p.Id -Force
}