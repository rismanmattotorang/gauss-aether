# Sign Windows .msi / .exe bundle artefacts with Authenticode.
#
# Usage:
#   $env:WINDOWS_CERTIFICATE = (Get-Content key.pfx.b64)
#   $env:WINDOWS_CERTIFICATE_PASSWORD = "..."
#   pwsh ./scripts/desktop/sign-windows.ps1 -BundleDir target/release/bundle
#
# Required environment:
#   WINDOWS_CERTIFICATE           Authenticode .pfx, base64 encoded.
#   WINDOWS_CERTIFICATE_PASSWORD  .pfx password.
#
# Behaviour:
#   - Decodes the .pfx into a temporary file on the runner.
#   - Locates signtool.exe via the Windows 10/11 SDK path.
#   - Signs every .msi and .exe under -BundleDir with SHA-256 +
#     RFC 3161 timestamping (DigiCert by default).
#
# Exit codes:
#   0  every artefact signed cleanly.
#   1  certificate decode / signtool failure.
#   2  bundle dir does not exist.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$BundleDir,

    [string]$TimestampUrl = "http://timestamp.digicert.com",

    [string]$DigestAlgo = "sha256"
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path $BundleDir)) {
    Write-Error "bundle dir does not exist: $BundleDir"
    exit 2
}

if (-not $env:WINDOWS_CERTIFICATE) {
    Write-Error "WINDOWS_CERTIFICATE env var is empty; refusing to sign."
    exit 1
}

# Decode the .pfx to a temp file.
$tempPfx = Join-Path $env:RUNNER_TEMP "code-sign.pfx"
[System.IO.File]::WriteAllBytes(
    $tempPfx,
    [System.Convert]::FromBase64String($env:WINDOWS_CERTIFICATE)
)

# Locate signtool.exe in the latest Windows SDK.
$signtool = Get-ChildItem `
    -Path "C:\Program Files (x86)\Windows Kits\10\bin\*\x64\signtool.exe" `
    -ErrorAction SilentlyContinue |
    Sort-Object -Property FullName -Descending |
    Select-Object -First 1 -ExpandProperty FullName

if (-not $signtool) {
    Write-Error "signtool.exe not found under the Windows 10/11 SDK path."
    exit 1
}
Write-Host "Using signtool: $signtool"

$artefacts = Get-ChildItem -Path $BundleDir -Recurse -Include *.msi, *.exe
if ($artefacts.Count -eq 0) {
    Write-Warning "no .msi / .exe artefacts found under $BundleDir"
    exit 0
}

foreach ($a in $artefacts) {
    Write-Host "signing: $($a.FullName)"
    & $signtool sign `
        /fd $DigestAlgo `
        /td $DigestAlgo `
        /tr $TimestampUrl `
        /f  $tempPfx `
        /p  $env:WINDOWS_CERTIFICATE_PASSWORD `
        /v `
        $a.FullName

    if ($LASTEXITCODE -ne 0) {
        Write-Error "signtool failed for $($a.FullName) with exit code $LASTEXITCODE"
        exit 1
    }
}

# Best-effort: remove the temp .pfx.
Remove-Item -Path $tempPfx -ErrorAction SilentlyContinue

Write-Host ("signed {0} artefact(s)" -f $artefacts.Count)
