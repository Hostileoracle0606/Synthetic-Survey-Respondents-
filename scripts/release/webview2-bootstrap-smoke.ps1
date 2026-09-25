<#
  TEST_PLAN S14 `clean_install_win10_no_webview2`. This script deliberately refuses to run
  unless the VM snapshot starts without a WebView2 Runtime, then uses the normal installer
  smoke test. The Tauri downloadBootstrapper configuration must install the runtime before the
  application can launch.
#>
param(
  [Parameter(Mandatory = $true)][string]$Installer
)
$ErrorActionPreference = 'Stop'
$client = '{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
$locations = @(
  "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\$client",
  "HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\$client",
  "HKCU:\SOFTWARE\Microsoft\EdgeUpdate\Clients\$client"
)
$installed = @($locations | Where-Object { (Get-ItemProperty $_ -ErrorAction SilentlyContinue).pv })
if ($installed.Count) {
  throw 'the win10-clean snapshot already has WebView2; restore the no-WebView2 snapshot before this job'
}

& (Join-Path $PSScriptRoot 'install-smoke.ps1') -Installer $Installer
if ($LASTEXITCODE -ne 0) { throw 'the installer did not bootstrap WebView2 and launch successfully' }
Write-Host 'Windows 10 no-WebView2 bootstrap smoke passed'

