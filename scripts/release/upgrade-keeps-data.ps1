<#
  TEST_PLAN S14 `upgrade_keeps_data`: install a baseline package, put a schema-v1 project in
  the real application data directory, install the candidate over it, launch the packaged app
  so its normal startup migrations run, and verify the marker data and SQLite integrity.

  Pass an older release as -PreviousInstaller for a real vN -> vN+1 test. If no older release
  exists yet, pass the candidate for both parameters; that is an explicit first-release
  reinstall rehearsal, not evidence of compatibility with a prior shipped version.
#>
param(
  [Parameter(Mandatory = $true)][string]$PreviousInstaller,
  [Parameter(Mandatory = $true)][string]$CurrentInstaller,
  [Parameter(Mandatory = $true)][string]$DbTool,
  [int]$SettleSeconds = 10
)
$ErrorActionPreference = 'Stop'
$product = 'Synthetic Survey'
$identifier = 'app.syntheticsurvey.desktop'
$dataDir = Join-Path $env:APPDATA $identifier
$database = Join-Path $dataDir 'data.db'
$marker = "upgrade-marker-$([Guid]::NewGuid().ToString('N'))"

foreach ($path in $PreviousInstaller, $CurrentInstaller, $DbTool) {
  if (-not (Test-Path $path)) { throw "not found: $path" }
}
$PreviousInstaller = (Resolve-Path $PreviousInstaller).Path
$CurrentInstaller = (Resolve-Path $CurrentInstaller).Path
$DbTool = (Resolve-Path $DbTool).Path

function Install-Package([string]$path) {
  Write-Host "Installing $path"
  if ($path.EndsWith('.msi')) {
    $process = Start-Process msiexec -ArgumentList "/i `"$path`" /qn /norestart" -Wait -PassThru
  } else {
    $process = Start-Process $path -ArgumentList '/S' -Wait -PassThru
  }
  if ($process.ExitCode -ne 0) {
    throw "installer exited with $($process.ExitCode)"
  }
}

function Find-AppExe {
  $roots = @(
    "$env:LOCALAPPDATA\$product",
    "$env:ProgramFiles\$product",
    "${env:ProgramFiles(x86)}\$product",
    "$env:LOCALAPPDATA\Programs\$product"
  )
  foreach ($root in $roots) {
    if (Test-Path $root) {
      $exe = Get-ChildItem $root -Filter '*.exe' -Recurse |
        Where-Object { $_.Name -notmatch 'uninstall' } |
        Select-Object -First 1
      if ($exe) { return $exe }
    }
  }
  return $null
}

function Uninstall-Package([System.IO.FileInfo]$exe, [string]$installer) {
  if ($installer.EndsWith('.msi')) {
    $process = Start-Process msiexec -ArgumentList "/x `"$installer`" /qn /norestart" -Wait -PassThru
  } else {
    $uninstaller = Get-ChildItem $exe.DirectoryName -Filter 'uninstall*.exe' | Select-Object -First 1
    if (-not $uninstaller) { throw 'uninstaller not found' }
    $process = Start-Process $uninstaller.FullName -ArgumentList '/S' -Wait -PassThru
    Start-Sleep -Seconds 5
  }
  if ($process.ExitCode -ne 0) { throw "uninstaller exited with $($process.ExitCode)" }
}

if (Test-Path $dataDir) { Remove-Item -Recurse -Force $dataDir }
$app = $null
try {
  Install-Package $PreviousInstaller
  New-Item -ItemType Directory -Force $dataDir | Out-Null
  & $DbTool seed-v1 --db $database --marker $marker
  if ($LASTEXITCODE -ne 0) { throw 'could not seed the old-schema database' }

  Install-Package $CurrentInstaller
  $appExe = Find-AppExe
  if (-not $appExe) { throw 'installed candidate application not found' }
  $app = Start-Process $appExe.FullName -PassThru
  Start-Sleep -Seconds $SettleSeconds
  $app.Refresh()
  if ($app.HasExited) { throw "candidate exited during migration (code $($app.ExitCode))" }
  if (-not $app.CloseMainWindow() -or -not $app.WaitForExit(10000)) {
    Stop-Process -Id $app.Id -Force
  }
  $app = $null

  & $DbTool verify --db $database --marker $marker
  if ($LASTEXITCODE -ne 0) { throw 'upgrade data verification failed' }

  Uninstall-Package $appExe $CurrentInstaller
  if (-not (Test-Path $database)) { throw 'uninstall removed the user database' }
  & $DbTool verify --db $database --marker $marker
  if ($LASTEXITCODE -ne 0) { throw 'user data did not survive uninstall' }
  Write-Host 'Upgrade, migration, and user-data preservation OK'
} finally {
  if ($app -and -not $app.HasExited) { Stop-Process -Id $app.Id -Force -ErrorAction SilentlyContinue }
  if (Test-Path $dataDir) { Remove-Item -Recurse -Force $dataDir }
}
