# Authenticode-sign a file with a PFX code-signing certificate.
#
# Inputs (environment):
#   CERT_PFX_BASE64  the .pfx file, base64-encoded (GitHub secret)
#   CERT_PASSWORD    the .pfx password            (GitHub secret)
#   TIMESTAMP_URL    RFC 3161 timestamp server (optional, default DigiCert)
#
# The timestamp keeps the signature valid after the certificate expires.
# Uses the built-in Set-AuthenticodeSignature (no Windows SDK / signtool needed).
param(
    [Parameter(Mandatory)] [string] $File,
    # Only for local testing with a self-signed cert: accept an untrusted root.
    [switch] $AllowUntrustedRoot
)
$ErrorActionPreference = 'Stop'

$tsUrl = if ($env:TIMESTAMP_URL) { $env:TIMESTAMP_URL } else { 'http://timestamp.digicert.com' }
$tmp = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$pfx = Join-Path $tmp ("sign-{0}.pfx" -f [guid]::NewGuid())

try {
    [IO.File]::WriteAllBytes($pfx, [Convert]::FromBase64String($env:CERT_PFX_BASE64))
    $cert = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2(
        $pfx, $env:CERT_PASSWORD,
        [System.Security.Cryptography.X509Certificates.X509KeyStorageFlags]::EphemeralKeySet)
    if (-not $cert.HasPrivateKey) { throw "certificate has no private key" }

    $sig = Set-AuthenticodeSignature -FilePath $File -Certificate $cert `
        -HashAlgorithm SHA256 -TimestampServer $tsUrl
    $check = Get-AuthenticodeSignature -FilePath $File

    "signer:    {0}" -f $check.SignerCertificate.Subject
    "timestamp: {0}" -f $(if ($check.TimeStamperCertificate) { $check.TimeStamperCertificate.Subject } else { 'NONE' })
    "status:    {0}" -f $check.Status

    if ($check.SignerCertificate.Thumbprint -ne $cert.Thumbprint) { throw "file is not signed by the given certificate" }
    if (-not $check.TimeStamperCertificate) { throw "no timestamp (server $tsUrl unreachable?)" }
    $ok = $check.Status -eq 'Valid' -or ($AllowUntrustedRoot -and $check.Status -eq 'UnknownError')
    if (-not $ok) { throw "signature not valid: $($check.Status) $($check.StatusMessage)" }
}
finally {
    # never leave the private key on disk
    if (Test-Path $pfx) { Remove-Item -Force $pfx }
}
