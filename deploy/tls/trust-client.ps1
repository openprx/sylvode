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
    # A CA certificate from sylvode-tls.sh is under 1 KiB: cap the input before anything parses it.
    if ((Get-Item -LiteralPath $CertPath).Length -gt 65536) {
        Stop-Script 3 'Refusing a file larger than 64 KiB.'
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

    if ($cert.RawData.Length -gt 16384) {
        Stop-Script 3 'Refusing a certificate larger than 16 KiB (DER).'
    }

    $fp = Format-Sha256 $cert.RawData
    $thumbprint = $cert.Thumbprint   # SHA-1: a lookup key for the store only, never a trust decision

    # --- SD-17 checks and display -----------------------------------------------------------------

    # The extensions are read from their DER bytes, never from formatted text: the subject and the
    # names can hold any characters.
    function Read-Tlv([byte[]]$Data, [int]$Pos, [int]$End) {
        if ($Pos + 2 -gt $End) { return $null }
        $tag = [int]$Data[$Pos]
        $len = [int]$Data[$Pos + 1]
        $hl = 2
        if ($len -ge 128) {
            $k = $len - 128
            if ($k -lt 1 -or $k -gt 3 -or ($Pos + 2 + $k) -gt $End) { return $null }
            $len = 0
            for ($i = 0; $i -lt $k; $i++) { $len = ($len * 256) + [int]$Data[$Pos + 2 + $i] }
            $hl = 2 + $k
        }
        if (($Pos + $hl + $len) -gt $End) { return $null }
        return [pscustomobject]@{ Tag = $tag; Start = ($Pos + $hl); Length = $len; Next = ($Pos + $hl + $len) }
    }

    function Read-Children([byte[]]$Data, [int]$Start, [int]$End) {
        $items = New-Object System.Collections.ArrayList
        $pos = $Start
        while ($pos -lt $End) {
            $tlv = Read-Tlv $Data $pos $End
            if ($null -eq $tlv) { Stop-Script 3 'Cannot parse the structure of the certificate; refusing it.' }
            [void]$items.Add($tlv)
            $pos = $tlv.Next
        }
        return $items.ToArray()
    }

    function Format-SafeText([byte[]]$Data, [int]$Start, [int]$Length) {
        $sb = New-Object System.Text.StringBuilder
        for ($i = 0; $i -lt $Length; $i++) {
            $b = [int]$Data[$Start + $i]
            if ($b -ge 33 -and $b -le 126) { [void]$sb.Append([char]$b) } else { [void]$sb.Append('?') }
        }
        return $sb.ToString()
    }

    # The DNS rules of sylvode-tls.sh (at least two labels), or the placeholder "invalid".
    function Test-DnsName([string]$Name) {
        if ($Name -ceq 'invalid') { return $true }
        if ($Name.Length -lt 1 -or $Name.Length -gt 253) { return $false }
        if ($Name -notmatch '\A[A-Za-z0-9.-]+\z') { return $false }
        if ($Name -match '\A[0-9.]+\z') { return $false }
        $labels = $Name.Split('.')
        if ($labels.Count -lt 2) { return $false }
        if ($labels[$labels.Count - 1] -ieq 'invalid') { return $false }
        foreach ($label in $labels) {
            if ($label.Length -lt 1 -or $label.Length -gt 63) { return $false }
            if ($label.StartsWith('-') -or $label.EndsWith('-')) { return $false }
        }
        return $true
    }

    function Format-GeneralName([byte[]]$Data, $Tlv) {
        if ($Tlv.Tag -eq 0x82) {
            $dnsText = Format-SafeText $Data $Tlv.Start $Tlv.Length
            return @{ Kind = 'dns'; Text = ('DNS:' + $dnsText); Ok = (Test-DnsName $dnsText) }
        }
        if ($Tlv.Tag -eq 0x87) {
            if ($Tlv.Length -eq 8) {
                $a = (0..3 | ForEach-Object { [string][int]$Data[$Tlv.Start + $_] }) -join '.'
                $m = (4..7 | ForEach-Object { [string][int]$Data[$Tlv.Start + $_] }) -join '.'
                $okMask = $true
                foreach ($i in 4..7) { if ($Data[$Tlv.Start + $i] -ne 0xFF) { $okMask = $false } }
                return @{ Kind = 'ip'; Text = "IP:$a/$m"; Ok = $okMask }
            }
            if ($Tlv.Length -eq 32) {
                $a = (0..7 | ForEach-Object { '{0:x}' -f (([int]$Data[$Tlv.Start + 2 * $_] * 256) + [int]$Data[$Tlv.Start + 2 * $_ + 1]) }) -join ':'
                $m = (8..15 | ForEach-Object { '{0:x}' -f (([int]$Data[$Tlv.Start + 2 * $_] * 256) + [int]$Data[$Tlv.Start + 2 * $_ + 1]) }) -join ':'
                $okMask = $true
                foreach ($i in 16..31) { if ($Data[$Tlv.Start + $i] -ne 0xFF) { $okMask = $false } }
                return @{ Kind = 'ip'; Text = "IP:$a/$m"; Ok = $okMask }
            }
            return @{ Kind = 'other'; Text = 'IP:(invalid length)'; Ok = $false }
        }
        return @{ Kind = 'other'; Text = ('(other name type, tag 0x{0:x2})' -f $Tlv.Tag); Ok = $true }
    }

    function Get-CaInfo {
        $info = @{ IsCa = $false; NcCount = 0; NcCritical = $false; PermIp = 0; PermDns = 0; Bad = @(); Perm = @(); Excl = @() }
        foreach ($ext in $cert.Extensions) {
            $raw = [byte[]]$ext.RawData
            if ($ext.Oid.Value -eq '2.5.29.19') {
                $seq = Read-Tlv $raw 0 $raw.Length
                if ($null -eq $seq -or $seq.Tag -ne 0x30) { Stop-Script 3 'Cannot parse the structure of the certificate; refusing it.' }
                $kids = @(Read-Children $raw $seq.Start $seq.Next)
                if ($kids.Count -ge 1 -and $kids[0].Tag -eq 1 -and $kids[0].Length -eq 1 -and $raw[$kids[0].Start] -eq 0xFF) {
                    $info.IsCa = $true
                }
            }
            if ($ext.Oid.Value -eq '2.5.29.30') {
                $info.NcCount++
                $info.NcCritical = [bool]$ext.Critical
                $seq = Read-Tlv $raw 0 $raw.Length
                if ($null -eq $seq -or $seq.Tag -ne 0x30) { Stop-Script 3 'Cannot parse the structure of the certificate; refusing it.' }
                foreach ($group in @(Read-Children $raw $seq.Start $seq.Next)) {
                    if ($group.Tag -ne 0xA0 -and $group.Tag -ne 0xA1) { Stop-Script 3 'Cannot parse the structure of the certificate; refusing it.' }
                    foreach ($subtree in @(Read-Children $raw $group.Start $group.Next)) {
                        if ($subtree.Tag -ne 0x30) { Stop-Script 3 'Cannot parse the structure of the certificate; refusing it.' }
                        $base = @(Read-Children $raw $subtree.Start $subtree.Next)
                        if ($base.Count -lt 1) { Stop-Script 3 'Cannot parse the structure of the certificate; refusing it.' }
                        $name = Format-GeneralName $raw $base[0]
                        if ($group.Tag -eq 0xA0) {
                            $info.Perm += $name.Text
                            if (-not $name.Ok) { $info.Bad += $name.Text }
                            if ($name.Kind -eq 'ip') { $info.PermIp++ }
                            if ($name.Kind -eq 'dns') { $info.PermDns++ }
                        }
                        else {
                            $info.Excl += $name.Text
                        }
                    }
                }
            }
        }
        return $info
    }

    function Show-Certificate($info) {
        Write-Output "Certificate: $CertPath"
        Write-Output "Subject:     $($cert.Subject)"
        Write-Output "Valid from:  $($cert.NotBefore.ToUniversalTime().ToString('u'))"
        Write-Output "Valid until: $($cert.NotAfter.ToUniversalTime().ToString('u'))"
        Write-Output "SHA-256:     $fp"
        Write-Output 'This CA may vouch for these names only:'
        Write-Output '  Permitted:'
        foreach ($line in $info.Perm) { Write-Output "    $line" }
        if (@($info.Excl).Count -gt 0) {
            Write-Output '  Excluded:'
            foreach ($line in $info.Excl) { Write-Output "    $line" }
        }
    }

    # SD-17 (tightened 2026-11-27): a CA with a critical name constraint that permits at least
    # one IP subtree and one DNS subtree.
    function Assert-ConstrainedCa($info) {
        if (-not $info.IsCa) {
            Stop-Script 3 'Refusing the certificate: it is not a CA certificate (basicConstraints CA:TRUE is missing).'
        }
        if ($info.NcCount -ne 1) {
            Stop-Script 3 'Refusing the certificate: it has no single name constraint, so it could vouch for any site.'
        }
        if (-not $info.NcCritical) {
            Stop-Script 3 'Refusing the certificate: its name constraint is not critical.'
        }
        if (@($info.Bad).Count -gt 0) {
            Stop-Script 3 "Refusing the certificate: the permitted subtree $(@($info.Bad)[0]) is not a single host (IP masks must be all ones: /32 or /128) or not a valid DNS name (at least two labels, no leading dot, not empty)."
        }
        if ($info.PermIp -lt 1 -or $info.PermDns -lt 1) {
            Stop-Script 3 'Refusing the certificate: its name constraint must permit at least one IP address and one DNS name; a missing type leaves that kind of name open.'
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
        $caInfo = Get-CaInfo
        Assert-ConstrainedCa $caInfo
        Show-Certificate $caInfo
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
            $found = @(Find-InStore $store)
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
    try { $present = @(Find-InStore $store).Count -gt 0 } finally { $store.Close() }
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
