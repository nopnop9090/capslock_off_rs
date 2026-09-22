# Findet das tray-icon-Window (Klassenname "tray_icon_app") und schickt
# einen simulierten WM_RBUTTONDOWN. Wenn die Pump funktioniert, sollte das
# Fenster das Menue aufmachen -- wir koennen das nicht visuell pruefen, aber
# wir koennen pruefen, ob das Window reachable ist.
$ErrorActionPreference = 'Stop'

Add-Type @"
using System;
using System.Runtime.InteropServices;

public class TrayTest {
    [DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    public static extern IntPtr FindWindowW(string lpClassName, string lpWindowName);

    [DllImport("user32.dll")]
    public static extern IntPtr FindWindowEx(IntPtr parent, IntPtr after, string cls, string title);

    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")]
    public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr lParam);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetClassNameW(IntPtr hWnd, System.Text.StringBuilder s, int n);

    [DllImport("user32.dll")]
    public static extern IntPtr SendMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);

    public const uint WM_USER_TRAYICON = 6002;
    public const uint WM_RBUTTONDOWN = 0x0204;
    public const uint WM_RBUTTONUP = 0x0205;
}
"@

# Alle Top-Level-Windows enumerieren, das mit Klassenname "tray_icon_app" suchen.
$found = New-Object 'System.Collections.Generic.List[IntPtr]'
$cb = [TrayTest+EnumWindowsProc]{
    param([IntPtr]$h, [IntPtr]$l)
    $sb = New-Object System.Text.StringBuilder 256
    [TrayTest]::GetClassNameW($h, $sb, 256) | Out-Null
    $cls = $sb.ToString()
    if ($cls -eq 'tray_icon_app') {
        $script:found.Add($h)
    }
    return $true
}
[TrayTest]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null

if ($found.Count -eq 0) {
    Write-Error "Kein tray_icon_app-Fenster gefunden. Tray ist nicht gestartet?"
}
Write-Host "Gefundene tray-icon-Windows: $($found.Count)"
$hwnd = $found[0]
Write-Host "HWND: $hwnd"

# Test: Fenster ist sichtbar? Existiert?
$title = New-Object System.Text.StringBuilder 256
# (GetWindowText wuerde nichts liefern, da hidden)
Write-Host "Sending WM_USER_TRAYICON mit lParam=WM_RBUTTONDOWN..."
# Das wuerde der Explorer normalerweise senden. Wenn die Pump laeuft, oeffnet
# sich das Menu via TrackPopupMenu (visuell sichtbar fuer den User).
$result = [TrayTest]::SendMessageW($hwnd, [TrayTest]::WM_USER_TRAYICON, [IntPtr]::Zero, [IntPtr]::new([int64][TrayTest]::WM_RBUTTONDOWN))
Write-Host "SendMessage result: $result"

Write-Host "---"
Write-Host "Wenn das Menue nach diesem Befehl erscheint, funktioniert die Pump."