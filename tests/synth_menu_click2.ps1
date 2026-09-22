# Findet Menu-Items mit ihren numerischen IDs, dann sendet WM_COMMAND.
$exe = 'D:\sysinfo\capslock_off_rs\target\release\capslock_off.exe'
$log = Join-Path $env:APPDATA 'capslock_off_rs\tray.log'

# Backup Log.
$ts = [int][double]::Parse((Get-Date -UFormat %s))
if (Test-Path $log) { Move-Item $log "$log.$ts" -Force }

# Tray starten.
$p = Start-Process -FilePath $exe -ArgumentList 'run' -PassThru -WindowStyle Hidden
Start-Sleep -Seconds 2
Write-Host "PID=$($p.Id)"

Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;

public class TrayT {
    public delegate bool EnumWindowsProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")]
    public static extern bool EnumWindows(EnumWindowsProc c, IntPtr l);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")]
    public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);

    [DllImport("user32.dll")]
    public static extern IntPtr FindWindowW(string cls, string title);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct MENUITEMINFOW {
        public uint cbSize;
        public uint fMask;
        public uint fType;
        public uint fState;
        public uint wID;
        public IntPtr hSubMenu;
        public IntPtr hbmpChecked;
        public IntPtr hbmpUnchecked;
        public uint dwItemData;
        public IntPtr dwTypeData;
        public uint cch;
        public IntPtr hbmpItem;
    }

    public const uint MIIM_ID = 0x00000002;
    public const uint MIIM_STRING = 0x00000040;
    public const uint MIIM_SUBMENU = 0x00000004;
    public const uint MIIM_FTYPE = 0x00000100;

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern bool GetMenuItemInfoW(IntPtr hMenu, uint uItem, bool fByPosition, ref MENUITEMINFOW lpmii);

    [DllImport("user32.dll")]
    public static extern IntPtr GetMenu(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern int GetMenuItemCount(IntPtr hMenu);
}
"@

# Tray-Window suchen.
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
$tray_hwnd = $found | Select-Object -First 1
Write-Host "Tray HWND: $tray_hwnd"

# Das Menu ist im tray-Window als Window-Menu attached.
# GetMenu(hwnd) gibt das Menu-Handle.
$hMenu = [TrayT]::GetMenu($tray_hwnd)
Write-Host "hMenu: $hMenu"
if ($hMenu -ne [IntPtr]::Zero) {
    $count = [TrayT]::GetMenuItemCount($hMenu)
    Write-Host "Menu items: $count"
    for ($i = 0; $i -lt $count; $i++) {
        $info = New-Object 'TrayT+MENUITEMINFOW'
        $info.cbSize = [System.Runtime.InteropServices.Marshal]::SizeOf([type][TrayT+MENUITEMINFOW])
        $info.fMask = [TrayT]::MIIM_ID -bor [TrayT]::MIIM_STRING -bor [TrayT]::MIIM_FTYPE
        $ok = [TrayT]::GetMenuItemInfoW($hMenu, $i, $true, [ref]$info)
        $txt = ""
        if ($info.dwTypeData -ne [IntPtr]::Zero) {
            $txt = [System.Runtime.InteropServices.Marshal]::PtrToStringUni($info.dwTypeData)
        }
        Write-Host "  [$i] id=$($info.wID) text='$txt'"
    }
}

# Schicke WM_COMMAND an das tray-Window.
# Item-IDs sind numerisch (1, 2, 3, ...). LOWORD(wparam) = id.
# mode_normal sollte Item 1 sein (nach Header).
if ($hMenu -ne [IntPtr]::Zero -and $count -gt 1) {
    Write-Host "Sende WM_COMMAND mit Item-ID 1 (mode_normal)..."
    $ret = [TrayT]::SendMessageW($tray_hwnd, 0x0111, [IntPtr]::new(1), [IntPtr]::Zero)
    Write-Host "SendMessage result: $ret"
}

Start-Sleep -Seconds 2
Write-Host "---tray.log:"
if (Test-Path $log) { Get-Content $log } else { Write-Host "(kein Log)" }

Stop-Process -Id $p.Id -Force
Start-Sleep -Seconds 1