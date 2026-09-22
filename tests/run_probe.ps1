$exe = 'D:\sysinfo\capslock_off_rs\target\release\capslock_off.exe'
$p = Start-Process -FilePath $exe -ArgumentList 'probe' -PassThru -Wait -NoNewWindow
Write-Host "exit=$($p.ExitCode)"
Write-Host "---probe.log---"
$log = Join-Path $env:APPDATA 'capslock_off_rs\probe.log'
if (Test-Path $log) { Get-Content $log } else { Write-Host "(nicht vorhanden)" }