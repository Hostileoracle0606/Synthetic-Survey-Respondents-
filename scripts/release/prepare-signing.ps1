<#
  Imports the trusted Windows code-signing certificate for a release build and writes a
  temporary Tauri config overlay to GITHUB_OUTPUT. Version tags require signing; branch and
  manual workflow checks may build unsigned when the secrets are absent.

  Required GitHub secrets when signing:
    WINDOWS_CERTIFICATE          Base64-encoded PFX (raw base64 or certutil/PEM text)
    WINDOWS_CERTIFICATE_PASSWORD PFX export password
#>
$ErrorActionPreference = 'Stop'

$certificate = $env:WINDOWS_CERTIFICATE
$password = $env:WINDOWS_CERTIFICATE_PASSWORD
$required = $env:GITHUB_REF_TYPE -eq 'tag'
$hasCertificate = -not [string]::IsNullOrWhiteSpace($certificate)
$hasPassword = -not [string]::IsNullOrWhiteSpace($password)

if ($hasCertificate -ne $hasPassword) {
  throw 'WINDOWS_CERTIFICATE and WINDOWS_CERTIFICATE_PASSWORD must either both be set or both be absent'
}

if (-not $hasCertificate) {
  if ($required) {
    throw 'A version tag cannot be released unsigned. Configure WINDOWS_CERTIFICATE and WINDOWS_CERTIFICATE_PASSWORD.'
  }
  'enabled=false' | Add-Content $env:GITHUB_OUTPUT
  Write-Host 'No signing certificate configured; this non-tag build will be unsigned.'
  exit 0
}

$encodedPath = Join-Path $env:RUNNER_TEMP 'synthetic-survey-signing.txt'
$pfxPath = Join-Path $env:RUNNER_TEMP 'synthetic-survey-signing.pfx'
$configPath = Join-Path $env:RUNNER_TEMP 'signing.tauri.conf.json'

try {
  [IO.File]::WriteAllText($encodedPath, $certificate)
  & certutil.exe -f -decode $encodedPath $pfxPath | Out-Null
  if ($LASTEXITCODE -ne 0) { throw 'Could not decode WINDOWS_CERTIFICATE as a base64 PFX' }

  $securePassword = ConvertTo-SecureString -String $password -AsPlainText -Force
  $imported = Import-PfxCertificate -FilePath $pfxPath -CertStoreLocation 'Cert:\CurrentUser\My' -Password $securePassword
  if (-not $imported) { throw 'The code-signing certificate was not imported' }
  if (-not $imported.HasPrivateKey) { throw 'The imported code-signing certificate has no private key' }
  if ($imported.NotAfter -le (Get-Date)) { throw 'The imported code-signing certificate has expired' }

  $codeSigningEku = '1.3.6.1.5.5.7.3.3'
  if (-not ($imported.EnhancedKeyUsageList.ObjectId.Value -contains $codeSigningEku)) {
    throw 'The imported certificate is not valid for code signing'
  }

  $overlay = @{
    bundle = @{
      windows = @{
        certificateThumbprint = $imported.Thumbprint
        digestAlgorithm = 'sha256'
        timestampUrl = 'http://timestamp.digicert.com'
        tsp = $true
      }
    }
  }
  $overlay | ConvertTo-Json -Depth 4 | Set-Content -Path $configPath -Encoding utf8

  'enabled=true' | Add-Content $env:GITHUB_OUTPUT
  "config=$configPath" | Add-Content $env:GITHUB_OUTPUT
  "thumbprint=$($imported.Thumbprint)" | Add-Content $env:GITHUB_OUTPUT
  Write-Host "Imported a code-signing certificate valid until $($imported.NotAfter.ToString('u'))."
} finally {
  Remove-Item $encodedPath, $pfxPath -Force -ErrorAction SilentlyContinue
}
