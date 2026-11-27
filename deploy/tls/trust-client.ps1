<#
.SYNOPSIS
Installs, removes or checks trust in a Sylvode intranet CA certificate on Windows.

.DESCRIPTION
Works with the CA certificate made by "sylvode-tls.sh init-ca". The script refuses a certificate
that is not a CA or that has no critical name constraint, prints the subject, validity, SHA-256
fingerprint and permitted names, and then needs either a matching -Fingerprint or an interactive
answer "yes" before it changes anything. -Yes is valid only together with -Fingerprint.

The certificate goes into the Trusted Root Certification Authorities store of the chosen scope.
LocalMachine (the default) needs an elevated session. CurrentUser makes Windows show its own
confirmation dialog.

The script is unsigned. Take it from the tagged repository and run it with:
  powershell -NoProfile -ExecutionPolicy Bypass -File .\trust-client.ps1 -Action Install -CertPath .\ca.crt -Fingerprint <hex>

Exit codes: 0 ok; 1 a command failed; 2 usage error; 3 refused (not a constrained CA, fingerprint
mismatch, not confirmed); 4 prerequisite missing (not elevated for LocalMachine).

.PARAMETER Action
Install, Remove or Check.

.PARAMETER CertPath
Path of the PEM file with the CA certificate (one certificate, no private key).

.PARAMETER Fingerprint
Expected SHA-256 of the DER certificate, as printed by "sylvode-tls.sh fingerprint". Colons and
case are ignored. The SHA-256 of the PEM file text does not match.

.PARAMETER Scope
LocalMachine (default) or CurrentUser.

.PARAMETER Yes
Do not ask for confirmation. Valid only together with -Fingerprint.

.PARAMETER Url
Check only: also send a HEAD request to this https URL with the system trust.

.PARAMETER DryRun
Print what would be done as "would run: ..." and change nothing.

.EXAMPLE
.\trust-client.ps1 -Action Install -CertPath .\ca.crt -Fingerprint 4C:CD:E4:09 -Yes

.EXAMPLE
.\trust-client.ps1 -Action Check -CertPath .\ca.crt -Url https://10.72.0.19/health
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Action,
    [Parameter(Mandatory = $true)][string]$CertPath,
    [string]$Fingerprint = '',
    [string]$Scope = 'LocalMachine',
    [switch]$Yes,
    [string]$Url = '',
    [switch]$DryRun
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Write-Err([string]$Message) {
    [Console]::Error.WriteLine("trust-client: error: $Message")
}

function Write-Info([string]$Message) {
    [Console]::Error.WriteLine("trust-client: $Message")
}

function Stop-Script([int]$Code, [string]$Message) {
    Write-Err $Message
    exit $Code
}

function Format-Sha256([byte[]]$Data) {
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $hash = $sha.ComputeHash($Data)
    }
    finally {
        $sha.Dispose()
    }
    return (($hash | ForEach-Object { $_.ToString('X2') }) -join ':')
}

# --- usage checks (exit 2) -----------------------------------------------------------------------

$actionName = $null
foreach ($candidate in @('Install', 'Remove', 'Check')) {
    if ($Action -ieq $candidate) { $actionName = $candidate }
}
if ($null -eq $actionName) { Stop-Script 2 'Action must be Install, Remove or Check.' }

$scopeName = $null
foreach ($candidate in @('LocalMachine', 'CurrentUser')) {
    if ($Scope -ieq $candidate) { $scopeName = $candidate }
}
if ($null -eq $scopeName) { Stop-Script 2 'Scope must be LocalMachine or CurrentUser.' }

if ($Yes -and [string]::IsNullOrEmpty($Fingerprint)) {
    Stop-Script 2 '-Yes is only valid together with -Fingerprint.'
}
if ($actionName -ne 'Install' -and (($Yes) -or -not [string]::IsNullOrEmpty($Fingerprint))) {
    Stop-Script 2 '-Fingerprint and -Yes are only valid for Install.'
}
if ($actionName -ne 'Check' -and -not [string]::IsNullOrEmpty($Url)) {
    Stop-Script 2 '-Url is only valid for Check.'
}
if ($actionName -eq 'Check' -and $DryRun) {
    Stop-Script 2 '-DryRun is not valid for Check.'
}

try {
    # --- load the certificate (exactly one PEM certificate, no key) -------------------------------

    if (-not (Test-Path -LiteralPath $CertPath -PathType Leaf)) {
        Stop-Script 2 "Not a file: $CertPath"
    }
    $pem = [System.IO.File]::ReadAllText((Resolve-Path -LiteralPath $CertPath).Path)
    if ($pem -match 'PRIVATE KEY') {
        Stop-Script 3 'Refusing a file that contains a private key; pass the CA certificate only.'
    }
    $certMatches = [regex]::Matches($pem, '-----BEGIN CERTIFICATE-----\s*([A-Za-z0-9+/=\s]+?)\s*-----END CERTIFICATE-----')
    if ($certMatches.Count -ne 1) {
        Stop-Script 3 "The file must hold exactly one PEM certificate (found $($certMatches.Count))."
    }
    $der = [Convert]::FromBase64String(($certMatches[0].Groups[1].Value -replace '\s', ''))
    $cert = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2 -ArgumentList (, $der)

    $fp = Format-Sha256 $cert.RawData
    $thumbprint = $cert.Thumbprint   # SHA-1: a lookup key for the store only, never a trust decision

    # --- SD-17 checks and display -----------------------------------------------------------------

    function Show-Certificate {
        Write-Output "Certificate: $CertPath"
        Write-Output "Subject:     $($cert.Subject)"
        Write-Output "Valid from:  $($cert.NotBefore.ToUniversalTime().ToString('u'))"
        Write-Output "Valid until: $($cert.NotAfter.ToUniversalTime().ToString('u'))"
        Write-Output "SHA-256:     $fp"
        Write-Output 'This CA may vouch for these names only:'
        foreach ($ext in $cert.Extensions) {
            if ($ext.Oid.Value -eq '2.5.29.30') {
                Write-Output ($ext.Format($true))
            }
        }
    }

    function Assert-ConstrainedCa {
        $isCa = $false
        $constrained = $false
        foreach ($ext in $cert.Extensions) {
            if ($ext.Oid.Value -eq '2.5.29.19') {
                $bc = New-Object System.Security.Cryptography.X509Certificates.X509BasicConstraintsExtension -ArgumentList $ext, $ext.Critical
                if ($bc.CertificateAuthority) { $isCa = $true }
            }
            if ($ext.Oid.Value -eq '2.5.29.30' -and $ext.Critical) {
                $constrained = $true
            }
        }
        if (-not $isCa) {
            Stop-Script 3 'Refusing the certificate: it is not a CA certificate.'
        }
        if (-not $constrained) {
            Stop-Script 3 'Refusing the certificate: it has no critical name constraint, so it could vouch for any site.'
        }
    }

    function Confirm-Trust {
        if (-not [string]::IsNullOrEmpty($Fingerprint)) {
            $want = ($Fingerprint -replace ':', '').ToUpperInvariant()
            $have = ($fp -replace ':', '')
            if ($want -ne $have) {
                Stop-Script 3 "The fingerprint does not match the certificate (expected $Fingerprint, computed $fp). Nothing was changed."
            }
            Write-Info 'fingerprint matches'
            return
        }
        if (-not [Environment]::UserInteractive) {
            Stop-Script 3 'No interactive session to confirm in; pass -Fingerprint (from sylvode-tls.sh fingerprint on the server).'
        }
        $answer = Read-Host 'Type yes to trust this CA for the names above'
        if ($answer -ne 'yes') {
            Stop-Script 3 'Not confirmed. Nothing was changed.'
        }
    }

    function Assert-Elevated {
        if ($scopeName -eq 'LocalMachine' -and -not $DryRun) {
            $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
            $principal = New-Object System.Security.Principal.WindowsPrincipal -ArgumentList $identity
            if (-not $principal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)) {
                Stop-Script 4 'LocalMachine needs an elevated session (Run as administrator), or use -Scope CurrentUser.'
            }
        }
    }

    function Open-RootStore([System.Security.Cryptography.X509Certificates.OpenFlags]$Flags) {
        $store = New-Object System.Security.Cryptography.X509Certificates.X509Store -ArgumentList 'Root', $scopeName
        $store.Open($Flags)
        return $store
    }

    function Find-InStore($store) {
        return @($store.Certificates.Find([System.Security.Cryptography.X509Certificates.X509FindType]::FindByThumbprint, $thumbprint, $false))
    }

    # --- actions ------------------------------------------------------------------------------------

    if ($actionName -eq 'Install') {
        Assert-ConstrainedCa
        Show-Certificate
        Confirm-Trust
        Assert-Elevated
        if ($DryRun) {
            Write-Output "would run: add certificate $thumbprint to the Root store of $scopeName"
        }
        else {
            $store = Open-RootStore ([System.Security.Cryptography.X509Certificates.OpenFlags]::ReadWrite)
            try { $store.Add($cert) } finally { $store.Close() }
            Write-Info "installed $fp in the Root store ($scopeName)"
        }
        Write-Output 'Hints:'
        Write-Output '  Firefox: set security.enterprise_roots.enabled to true in about:config if it is not already (the default in current releases).'
        Write-Output "  Node.js: setx NODE_EXTRA_CA_CERTS $((Resolve-Path -LiteralPath $CertPath).Path)   (read at start-up; open a new terminal afterwards)"
        exit 0
    }

    if ($actionName -eq 'Remove') {
        Assert-Elevated
        if ($DryRun) {
            Write-Output "would run: remove certificate $thumbprint from the Root store of $scopeName"
            exit 0
        }
        $store = Open-RootStore ([System.Security.Cryptography.X509Certificates.OpenFlags]::ReadWrite)
        try {
            $found = Find-InStore $store
            foreach ($c in $found) { $store.Remove($c) }
        }
        finally { $store.Close() }
        if ($found.Count -eq 0) {
            Write-Info "the certificate was not present in the Root store ($scopeName)"
        }
        else {
            Write-Info "removed $fp from the Root store ($scopeName)"
        }
        exit 0
    }

    # Check
    $rc = 0
    $store = Open-RootStore ([System.Security.Cryptography.X509Certificates.OpenFlags]::ReadOnly)
    try { $present = (Find-InStore $store).Count -gt 0 } finally { $store.Close() }
    if ($present) {
        Write-Output "present: $fp in the Root store ($scopeName)"
    }
    else {
        Write-Output "missing: $fp in the Root store ($scopeName)"
        $rc = 1
    }
    if (-not [string]::IsNullOrEmpty($Url)) {
        try {
            Invoke-WebRequest -UseBasicParsing -Method Head -Uri $Url | Out-Null
            Write-Output "url check passed: $Url"
        }
        catch {
            Write-Output "url check failed: $Url ($($_.Exception.Message))"
            $rc = 1
        }
    }
    exit $rc
}
catch {
    Write-Err "$($_.Exception.Message)"
    exit 1
}
