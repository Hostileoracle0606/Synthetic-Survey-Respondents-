<#
  TEST_PLAN S14 install smoke: install silently, launch, check the app stays up and within
  its idle memory budget, close, uninstall, and check the program files are gone.

  Usage: install-smoke.ps1 -Installer <path to *-setup.exe or *.msi> [-MaxRustMB 50]
  Runs on GitHub's windows-latest (Windows Server) for every release, and on the Windows 11
  VMs (runner labels win11-clean / win11-perf) when they are enabled (TEST_PLAN §6).
#>
param(
  [Parameter(Mandatory = $true)][string]$Installer,
  [int]$MaxRustMB = 50,
  [int]$SettleSeconds = 15
)
$ErrorActionPreference = 'Stop'
$product = 'Synthetic Survey'
$isMsi = $Installer.EndsWith('.msi')

function Find-AppExe {
  $roots = @("$env:LOCALAPPDATA\$product", "$env:ProgramFiles\$product", "${env:ProgramFiles(x86)}\$product", "$env:LOCALAPPDATA\Programs\$product")
  foreach ($r in $roots) {
    if (Test-Path $r) {
      $exe = Get-ChildItem $r -Filter *.exe -Recurse | Where-Object { $_.Name -notmatch 'uninstall' } | Select-Object -First 1
      if ($exe) { return $exe }
    }
  }
  return $null
}

Write-Host "Installing $Installer"
if ($isMsi) {
  $p = Start-Process msiexec -ArgumentList "/i `"$Installer`" /qn /norestart" -Wait -PassThru
} else {
  $p = Start-Process $Installer -ArgumentList '/S' -Wait -PassThru
}
if ($p.ExitCode -ne 0) { throw "installer exited with $($p.ExitCode)" }

$exe = Find-AppExe
if (-not $exe) { throw "installed app not found" }
Write-Host "Installed at $($exe.FullName)"

$app = Start-Process $exe.FullName -PassThru
Start-Sleep -Seconds $SettleSeconds
$app.Refresh()
if ($app.HasExited) { throw "the app exited within $SettleSeconds s (code $($app.ExitCode))" }

$rustMB = [math]::Round($app.WorkingSet64 / 1MB, 1)
$webviews = Get-CimInstance Win32_Process -Filter "Name = 'msedgewebview2.exe'" |
  Where-Object { $_.CommandLine -match 'app.syntheticsurvey.desktop' }
$renderer = $webviews | Where-Object { $_.CommandLine -match '--type=renderer' } |
  ForEach-Object { (Get-Process -Id $_.ProcessId -ErrorAction SilentlyContinue).WorkingSet64 } |
  Measure-Object -Maximum
$rendererMB = if ($renderer.Maximum) { [math]::Round($renderer.Maximum / 1MB, 1) } else { 'n/a' }
Write-Host "Idle working set: app $rustMB MB, WebView2 renderer $rendererMB MB, $($webviews.Count) WebView2 processes"
if ($rustMB -gt $MaxRustMB) { throw "app process uses $rustMB MB at idle, over $MaxRustMB MB" }

Stop-Process -Id $app.Id -Force
Start-Sleep -Seconds 3

Write-Host "Uninstalling"
if ($isMsi) {
  $u = Start-Process msiexec -ArgumentList "/x `"$Installer`" /qn /norestart" -Wait -PassThru
} else {
  $uninstaller = Get-ChildItem $exe.DirectoryName -Filter 'uninstall*.exe' | Select-Object -First 1
  if (-not $uninstaller) { throw "uninstaller not found" }
  $u = Start-Process $uninstaller.FullName -ArgumentList '/S' -Wait -PassThru
  Start-Sleep -Seconds 5  # NSIS finishes removing files in a child process
}
if ($u.ExitCode -ne 0) { throw "uninstaller exited with $($u.ExitCode)" }
if (Test-Path $exe.FullName) { throw "program files remain after uninstall: $($exe.FullName)" }
Write-Host "Install, launch and uninstall OK"
