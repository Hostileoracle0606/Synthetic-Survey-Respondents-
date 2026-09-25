<#
  TEST_PLAN S14 signature_valid: the application executable and both distributed installers
  must have valid Authenticode signatures from the certificate imported for this build.
#>
param(
  [Parameter(Mandatory = $true)][string]$ExpectedThumbprint
)
$ErrorActionPreference = 'Stop'

$signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" |
  Sort-Object FullName |
  Select-Object -Last 1
if (-not $signtool) { throw 'signtool.exe was not found' }

$app = Get-Item 'target/release/survey-app.exe' -ErrorAction Stop
$installers = @(Get-ChildItem -Recurse 'target/release/bundle' -Include '*.msi', '*-setup.exe')
if ($installers.Count -ne 2) {
  throw "Expected one MSI and one NSIS installer, found $($installers.Count)"
}

$expected = $ExpectedThumbprint.Replace(' ', '').ToUpperInvariant()
foreach ($file in @($app) + $installers) {
  & $signtool.FullName verify /pa /all $file.FullName
  if ($LASTEXITCODE -ne 0) { throw "Authenticode verification failed for $($file.Name)" }

  $signature = Get-AuthenticodeSignature $file.FullName
  if ($signature.Status -ne 'Valid') {
    throw "Authenticode status for $($file.Name) is $($signature.Status): $($signature.StatusMessage)"
  }
  $actual = $signature.SignerCertificate.Thumbprint.Replace(' ', '').ToUpperInvariant()
  if ($actual -ne $expected) {
    throw "The signature on $($file.Name) used certificate $actual instead of the imported certificate"
  }
  Write-Host "Verified $($file.Name)"
}
