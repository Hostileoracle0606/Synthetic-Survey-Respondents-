<#
  TEST_PLAN S7 `memory_1000` and S10 `stream_fps_1000` (BACKLOG B14): 1,000 respondents ×
  20 questions stream from a local mock Gemini server into a performance-harness build of the
  app while perf-1000.mjs records frames and browses every screen, and this script samples
  memory every 500 ms. Runs several times; the median of each figure is compared with the
  budget (performance on shared runners is noisy).

  Memory is enforced: app (Rust) process <= 50 MB, WebView2 renderer <= 100 MB, peak working
  set. Frames are warn-only unless -StrictFps, because GitHub's windows-latest has no GPU
  (TEST_PLAN §6): >= 95% of frames on time and no long task over 100 ms.

  Usage: perf-1000.ps1 -App <survey-app.exe> -Mock <mock-gemini.exe> -Seed <perf-seed.exe>
                       [-Runs 3] [-DelayMs 300] [-OutDir perf-results] [-StrictFps]
  The app must be built with `--features perf` and VITE_PERF_HARNESS=1.
#>
param(
  [Parameter(Mandatory = $true)][string]$App,
  [Parameter(Mandatory = $true)][string]$Mock,
  [Parameter(Mandatory = $true)][string]$Seed,
  [int]$Runs = 3,
  [int]$DelayMs = 300,
  [int]$MaxRustMB = 50,
  [int]$MaxRendererMB = 100,
  [string]$OutDir = 'perf-results',
  [switch]$StrictFps
)
$ErrorActionPreference = 'Stop'
foreach ($f in $App, $Mock, $Seed) {
  if (-not (Test-Path $f)) {
    Get-ChildItem (Split-Path $f) -Filter *.exe -ErrorAction SilentlyContinue | ForEach-Object { Write-Host "  found $($_.Name)" }
    throw "not found: $f"
  }
}
$App = (Resolve-Path $App).Path; $Mock = (Resolve-Path $Mock).Path; $Seed = (Resolve-Path $Seed).Path
$identifier = 'app.syntheticsurvey.desktop'
$dataDir = Join-Path $env:APPDATA $identifier
$cdpPort = 9222
New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path

function WebView2Processes {
  Get-CimInstance Win32_Process -Filter "Name = 'msedgewebview2.exe'" |
    Where-Object { $_.CommandLine -match [regex]::Escape($identifier) }
}

function Median([double[]]$xs) {
  $s = @($xs | Sort-Object)
  if ($s.Count % 2) { return $s[[int][math]::Floor($s.Count / 2)] }
  return ($s[$s.Count / 2 - 1] + $s[$s.Count / 2]) / 2
}

function Invoke-PerfRun([int]$n) {
  Write-Host "=== Run $n of $Runs ==="
  $mockPort = 8786 + $n
  if (Test-Path $dataDir) { Remove-Item -Recurse -Force $dataDir }
  New-Item -ItemType Directory -Force $dataDir | Out-Null
  $project = (& $Seed --db (Join-Path $dataDir 'data.db')) | Select-Object -Last 1
  if ($LASTEXITCODE -ne 0) { throw "perf-seed failed" }

  $mockProc = Start-Process $Mock -ArgumentList '--port', $mockPort, '--delay-ms', $DelayMs -PassThru -NoNewWindow `
    -RedirectStandardOutput (Join-Path $OutDir "mock-$n.log") -RedirectStandardError (Join-Path $OutDir "mock-$n.err")
  $env:SURVEY_PERF_GEMINI_URL = "http://127.0.0.1:$mockPort"
  # Keep rendering at full rate even if the runner's desktop hides the window.
  $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$cdpPort --disable-renderer-backgrounding --disable-background-timer-throttling --disable-backgrounding-occluded-windows"
  $appProc = Start-Process $App -PassThru
  $uiJson = Join-Path $OutDir "ui-$n.json"
  $driver = Start-Process node -ArgumentList (Join-Path $PSScriptRoot 'perf-1000.mjs'), '--project', $project, '--port', $cdpPort, '--out', $uiJson `
    -PassThru -NoNewWindow -RedirectStandardOutput (Join-Path $OutDir "driver-$n.log") -RedirectStandardError (Join-Path $OutDir "driver-$n.err")

  $samples = [System.Collections.Generic.List[object]]::new()
  $rendererIds = @()
  $others = @()
  $clock = [Diagnostics.Stopwatch]::StartNew()
  $lastScan = -10000
  try {
    while (-not $driver.HasExited) {
      if ($appProc.HasExited) { throw "the app exited during run $n (code $($appProc.ExitCode))" }
      if ($clock.ElapsedMilliseconds - $lastScan -ge 5000) {
        $wv = @(WebView2Processes)
        $rendererIds = @($wv | Where-Object { $_.CommandLine -match '--type=renderer' } | ForEach-Object { $_.ProcessId })
        $others = @($wv | Where-Object { $_.CommandLine -notmatch '--type=renderer' } | ForEach-Object { $_.ProcessId })
        $lastScan = $clock.ElapsedMilliseconds
      }
      $appMB = (Get-Process -Id $appProc.Id).WorkingSet64 / 1MB
      $rMB = ($rendererIds | ForEach-Object { (Get-Process -Id $_ -ErrorAction SilentlyContinue).WorkingSet64 } | Measure-Object -Maximum).Maximum / 1MB
      $oMB = ($others | ForEach-Object { (Get-Process -Id $_ -ErrorAction SilentlyContinue).WorkingSet64 } | Measure-Object -Sum).Sum / 1MB
      $samples.Add([pscustomobject]@{ ms = $clock.ElapsedMilliseconds; appMB = [math]::Round($appMB, 1); rendererMB = [math]::Round($rMB, 1); otherWebView2MB = [math]::Round($oMB, 1) })
      Start-Sleep -Milliseconds 500
    }
    $driver.WaitForExit()
    Get-Content (Join-Path $OutDir "driver-$n.log") | Write-Host
    if ($driver.ExitCode -ne 0) {
      Get-Content (Join-Path $OutDir "driver-$n.err") | Write-Host
      throw "the UI driver failed in run $n (exit $($driver.ExitCode))"
    }
    # The OS keeps each process's peak working set; use it too, in case a peak fell between samples.
    $appPeak = (Get-Process -Id $appProc.Id).PeakWorkingSet64 / 1MB
    $rendererPeak = ($rendererIds | ForEach-Object { (Get-Process -Id $_ -ErrorAction SilentlyContinue).PeakWorkingSet64 } | Measure-Object -Maximum).Maximum / 1MB
  } finally {
    foreach ($p in @($driver, $appProc, $mockProc)) {
      if ($p -and -not $p.HasExited) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }
    }
    # Wait for WebView2 to exit with the app, so the next run measures only its own processes.
    for ($i = 0; $i -lt 30 -and @(WebView2Processes).Count; $i++) { Start-Sleep -Milliseconds 500 }
    Remove-Item Env:\SURVEY_PERF_GEMINI_URL, Env:\WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS -ErrorAction SilentlyContinue
  }
  $samples | Export-Csv -NoTypeInformation (Join-Path $OutDir "memory-$n.csv")
  $ui = Get-Content $uiJson -Raw | ConvertFrom-Json
  $sampled = $samples | Measure-Object -Property appMB, rendererMB, otherWebView2MB -Maximum
  $result = [pscustomobject]@{
    run                = $n
    appPeakMB          = [math]::Round([math]::Max($appPeak, ($sampled | Where-Object Property -eq appMB).Maximum), 1)
    rendererPeakMB     = [math]::Round([math]::Max($rendererPeak, ($sampled | Where-Object Property -eq rendererMB).Maximum), 1)
    otherWebView2MB    = ($sampled | Where-Object Property -eq otherWebView2MB).Maximum
    samples            = $samples.Count
    runSeconds         = $ui.fps.runSeconds
    frames             = $ui.fps.frames
    averageFps         = $ui.fps.averageFps
    onTimePct          = $ui.fps.onTimePct
    p95FrameMs         = $ui.fps.p95FrameMs
    longTasksOver100ms = $ui.fps.longTasksOver100ms
    maxLongTaskMs      = $ui.fps.maxLongTaskMs
    crossTabs          = "$($ui.report.crossTabs) of $($ui.report.selects)"
  }
  $result | Format-List | Out-String | Write-Host
  return $result
}

$results = @(1..$Runs | ForEach-Object { Invoke-PerfRun $_ })
$median = [ordered]@{}
foreach ($k in 'appPeakMB', 'rendererPeakMB', 'otherWebView2MB', 'runSeconds', 'averageFps', 'onTimePct', 'p95FrameMs', 'longTasksOver100ms', 'maxLongTaskMs') {
  $median[$k] = Median ($results | ForEach-Object { [double]$_.$k })
}
$summary = [ordered]@{
  runner  = "$env:ImageOS $env:ImageVersion".Trim()
  os      = (Get-CimInstance Win32_OperatingSystem).Caption
  gpu     = ((Get-CimInstance Win32_VideoController).Name -join ', ')
  webview = (Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' -ErrorAction SilentlyContinue).pv
  budgets = [ordered]@{ appMB = $MaxRustMB; rendererMB = $MaxRendererMB; onTimePct = 95; longTasksOver100ms = 0 }
  median  = $median
  runs    = $results
}
$summary | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $OutDir 'perf.json')

$memoryOk = $median.appPeakMB -le $MaxRustMB -and $median.rendererPeakMB -le $MaxRendererMB
$fpsOk = $median.onTimePct -ge 95 -and $median.longTasksOver100ms -eq 0
$table = @(
  "### 1,000 respondents × 20 questions streaming (median of $Runs runs)",
  '',
  '| Check | Median | Budget | Result |',
  '|---|---|---|---|',
  "| App (Rust) peak working set | $($median.appPeakMB) MB | ≤ $MaxRustMB MB | $(if ($median.appPeakMB -le $MaxRustMB) { 'pass' } else { 'FAIL' }) |",
  "| WebView2 renderer peak working set | $($median.rendererPeakMB) MB | ≤ $MaxRendererMB MB | $(if ($median.rendererPeakMB -le $MaxRendererMB) { 'pass' } else { 'FAIL' }) |",
  "| Other WebView2 processes (not counted) | $($median.otherWebView2MB) MB | — | — |",
  "| Frames on time | $($median.onTimePct)% ($($median.averageFps) fps, p95 $($median.p95FrameMs) ms) | ≥ 95% | $(if ($median.onTimePct -ge 95) { 'pass' } elseif ($StrictFps) { 'FAIL' } else { 'warn' }) |",
  "| Long tasks over 100 ms | $($median.longTasksOver100ms) (longest $($median.maxLongTaskMs) ms) | 0 | $(if ($median.longTasksOver100ms -eq 0) { 'pass' } elseif ($StrictFps) { 'FAIL' } else { 'warn' }) |",
  "| Run time | $($median.runSeconds) s | — | — |",
  '',
  "Runner: $($summary.os), GPU: $($summary.gpu), WebView2 $($summary.webview)."
)
$table | Write-Host
if ($env:GITHUB_STEP_SUMMARY) { $table | Add-Content $env:GITHUB_STEP_SUMMARY }

if (-not $fpsOk) {
  $msg = "frame budget missed: $($median.onTimePct)% of frames on time, $($median.longTasksOver100ms) long tasks over 100 ms"
  if ($StrictFps) { throw $msg } else { Write-Host "::warning::$msg (warn-only without a GPU, TEST_PLAN S10)" }
}
if (-not $memoryOk) { throw "memory over budget: app $($median.appPeakMB) MB (≤ $MaxRustMB), renderer $($median.rendererPeakMB) MB (≤ $MaxRendererMB)" }
Write-Host "Memory within budget"
