<#
  TEST_PLAN S8 `network_egress`: install the production package, enable the Windows Filtering
  Platform connection audit, make one real Gemini request through the UI, and fail if the app
  executable connects to any non-loopback address not currently resolved for
  generativelanguage.googleapis.com.

  This needs an elevated Windows runner, Node 22+, and GEMINI_API_KEY in the environment.
#>
param(
  [Parameter(Mandatory = $true)][string]$Installer,
  [string]$Driver = (Join-Path $PSScriptRoot 'network-egress.mjs'),
  [int]$CdpPort = 9223
)
$ErrorActionPreference = 'Stop'
$product = 'Synthetic Survey'
$hostName = 'generativelanguage.googleapis.com'

foreach ($path in $Installer, $Driver) {
  if (-not (Test-Path $path)) { throw "not found: $path" }
}
if ([string]::IsNullOrWhiteSpace($env:GEMINI_API_KEY)) {
  throw 'GEMINI_API_KEY is required for the release network-egress check'
}
$Installer = (Resolve-Path $Installer).Path
$Driver = (Resolve-Path $Driver).Path

function Install-Package([string]$path) {
  if ($path.EndsWith('.msi')) {
    $process = Start-Process msiexec -ArgumentList "/i `"$path`" /qn /norestart" -Wait -PassThru
  } else {
    $process = Start-Process $path -ArgumentList '/S' -Wait -PassThru
  }
  if ($process.ExitCode -ne 0) { throw "installer exited with $($process.ExitCode)" }
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

function Resolve-AllowedAddresses {
  @(
    foreach ($type in 'A', 'AAAA') {
      Resolve-DnsName $hostName -Type $type -ErrorAction Stop |
        Where-Object IPAddress |
        ForEach-Object { ([Net.IPAddress]::Parse($_.IPAddress)).ToString() }
    }
  )
}

function Event-Fields($event) {
  [xml]$xml = $event.ToXml()
  $fields = @{}
  foreach ($field in $xml.Event.EventData.Data) {
    $fields[[string]$field.Name] = [string]$field.'#text'
  }
  return $fields
}

function Is-RemoteAddress([string]$address) {
  $parsed = $null
  if (-not [Net.IPAddress]::TryParse($address, [ref]$parsed)) { return $false }
  if ([Net.IPAddress]::IsLoopback($parsed)) { return $false }
  if ($parsed.Equals([Net.IPAddress]::Any) -or $parsed.Equals([Net.IPAddress]::IPv6Any)) { return $false }
  return $true
}

$app = $null
$appExe = $null
$auditWasEnabled = (& auditpol.exe /get /subcategory:'Filtering Platform Connection' /r) -match 'Success'
try {
  Install-Package $Installer
  $appExe = Find-AppExe
  if (-not $appExe) { throw 'installed application not found' }

  $allowed = @(Resolve-AllowedAddresses | Sort-Object -Unique)
  if (-not $allowed.Count) { throw "DNS returned no addresses for $hostName" }
  Write-Host "Allowed Gemini addresses: $($allowed -join ', ')"

  & auditpol.exe /set /subcategory:'Filtering Platform Connection' /success:enable | Write-Host
  if ($LASTEXITCODE -ne 0) { throw 'could not enable Windows Filtering Platform connection auditing' }
  $started = (Get-Date).AddSeconds(-2)
  $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$CdpPort"
  $env:SURVEY_CDP_PORT = [string]$CdpPort
  $app = Start-Process $appExe.FullName -PassThru
  & node $Driver
  if ($LASTEXITCODE -ne 0) { throw 'the egress UI driver failed' }
  Start-Sleep -Seconds 2
  $app.Refresh()
  if ($app.HasExited) { throw "the app exited during the egress check (code $($app.ExitCode))" }
  Stop-Process -Id $app.Id -Force
  $app = $null

  $allowed = @($allowed + @(Resolve-AllowedAddresses) | Sort-Object -Unique)
  $leaf = $appExe.Name.ToLowerInvariant()
  $connections = @(
    Get-WinEvent -FilterHashtable @{ LogName = 'Security'; Id = 5156; StartTime = $started } -ErrorAction Stop |
      ForEach-Object { Event-Fields $_ } |
      Where-Object {
        $_['Application'] -and $_['Application'].ToLowerInvariant().EndsWith("\$leaf") -and
        (Is-RemoteAddress $_['DestAddress'])
      }
  )
  if (-not $connections.Count) {
    throw 'no outbound Windows Filtering Platform events were captured for the application'
  }

  $unexpected = @($connections | Where-Object { $allowed -notcontains ([Net.IPAddress]::Parse($_['DestAddress'])).ToString() })
  foreach ($connection in $connections) {
    Write-Host "Observed $($connection['DestAddress']):$($connection['DestPort'])"
  }
  if ($unexpected.Count) {
    $destinations = $unexpected | ForEach-Object { "$($_['DestAddress']):$($_['DestPort'])" } | Sort-Object -Unique
    throw "unexpected application egress: $($destinations -join ', ')"
  }
  Write-Host "Network egress limited to $hostName"
} finally {
  if ($app -and -not $app.HasExited) { Stop-Process -Id $app.Id -Force -ErrorAction SilentlyContinue }
  & cmdkey.exe /delete:SyntheticSurvey 2>&1 | Out-Null
  if ($appExe) {
    if ($Installer.EndsWith('.msi')) {
      Start-Process msiexec -ArgumentList "/x `"$Installer`" /qn /norestart" -Wait -ErrorAction SilentlyContinue | Out-Null
    } else {
      $uninstaller = Get-ChildItem $appExe.DirectoryName -Filter 'uninstall*.exe' -ErrorAction SilentlyContinue | Select-Object -First 1
      if ($uninstaller) { Start-Process $uninstaller.FullName -ArgumentList '/S' -Wait -ErrorAction SilentlyContinue | Out-Null }
    }
  }
  Remove-Item Env:\WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS, Env:\SURVEY_CDP_PORT -ErrorAction SilentlyContinue
  if (-not $auditWasEnabled) {
    & auditpol.exe /set /subcategory:'Filtering Platform Connection' /success:disable | Out-Null
  }
}
