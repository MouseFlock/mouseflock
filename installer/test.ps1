<#
.SYNOPSIS
Installs, upgrades and uninstalls DeskHop, checking each step.

.DESCRIPTION
Run from an elevated PowerShell (Run as administrator), on a PC where DeskHop
is not installed and the development service is not registered:

    installer\build.ps1 -Arch x64
    installer\build.ps1 -Arch x64 -PackageOnly -OutputSuffix -rebuild
    installer\test.ps1 -Msi installer\out\DeskHop-0.1.0-x64.msi `
        -UpgradeMsi installer\out\DeskHop-0.1.0-x64-rebuild.msi `
        -Setup installer\out\DeskHop-Setup-0.1.0-x64.exe

The MSI is installed, upgraded with the second package (the same build with a
new product code), and uninstalled. Then, with -Setup, the setup program is
installed and uninstalled (add-installer design D9, D11). All of it runs
silently. Every check is printed; the script stops with exit code 1 at the
first failure. Logs go next to the packages.
#>
param(
    [Parameter(Mandatory = $true)] [string]$Msi,
    [Parameter(Mandatory = $true)] [string]$UpgradeMsi,
    [string]$Setup = ''
)

$ErrorActionPreference = 'Continue'
$Folder = Join-Path $env:ProgramFiles 'DeskHop'
$Data = Join-Path $env:ProgramData 'DeskHop'
$Shortcut = Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs\DeskHop.lnk'
$Marker = 'HKLM:\SOFTWARE\DeskHop'
$SasKey = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System'
$SasValue = 'SoftwareSASGeneration'
$Rules = @{ 'DeskHop (UDP 47391)' = 47391; 'DeskHop (mDNS)' = 5353 }
$Programs = 'deskhop-service.exe', 'deskhop-agent.exe', 'DeskHop.exe'
$UninstallKeys = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*',
    'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*'

function Check([string]$What, [bool]$Ok, [string]$Detail = '') {
    if ($Ok) {
        Write-Host "  ok    $What"
    } else {
        Write-Host "  FAIL  $What $Detail" -ForegroundColor Red
        exit 1
    }
}

function Run([string]$File, [string]$Arguments) {
    $p = Start-Process -FilePath $File -ArgumentList $Arguments -Wait -PassThru
    $p.ExitCode
}

function Msiexec([string]$Arguments, [string]$Log) {
    Run msiexec.exe "$Arguments /qn /norestart /l*v `"$Log`""
}

function Sas { (Get-ItemProperty -Path $SasKey -Name $SasValue -ErrorAction SilentlyContinue).$SasValue }

function Service { Get-CimInstance Win32_Service -Filter "Name='DeskHop'" }

# The DeskHop entries Installed apps shows: named DeskHop, not system components.
function Entries {
    @(Get-ItemProperty -Path $UninstallKeys -ErrorAction SilentlyContinue |
        Where-Object { $_.DisplayName -eq 'DeskHop' -and $_.SystemComponent -ne 1 })
}

# Waits up to `seconds` for `test` to hold.
function Within([int]$Seconds, [scriptblock]$Test) {
    $end = (Get-Date).AddSeconds($Seconds)
    do {
        if (& $Test) { return $true }
        Start-Sleep -Milliseconds 250
    } while ((Get-Date) -lt $end)
    return [bool](& $Test)
}

function Assert-Installed {
    $svc = Service
    Check 'service registered' ($null -ne $svc)
    Check 'service running' (Within 10 { (Service).State -eq 'Running' }) "state $((Service).State)"
    Check 'service starts automatically' ($svc.StartMode -eq 'Auto') "start mode $($svc.StartMode)"
    Check 'service runs as LocalSystem' ($svc.StartName -eq 'LocalSystem') "account $($svc.StartName)"
    Check 'service runs from Program Files' ($svc.PathName -like "*$Folder\deskhop-service.exe*") $svc.PathName
    $failure = (sc.exe qfailure DeskHop) -join "`n"
    Check 'service restarts after a failure' ($failure -match 'RESTART -- Delay = 5000') $failure
    foreach ($p in $Programs) { Check "$p installed" (Test-Path (Join-Path $Folder $p)) }
    Check 'Start menu shortcut' (Test-Path $Shortcut)
    Check 'install folder recorded' ((Get-ItemProperty -Path $Marker -ErrorAction SilentlyContinue).InstallFolder -like "$Folder*")
    Check 'one DeskHop entry in Installed apps' (@(Entries).Count -eq 1) "found $(@(Entries).Count)"
    foreach ($name in $Rules.Keys) {
        $rule = Get-NetFirewallRule -DisplayName $name -ErrorAction SilentlyContinue
        Check "firewall rule '$name'" ($null -ne $rule)
        Check "  inbound, allowed, enabled" ($rule.Direction -eq 'Inbound' -and $rule.Action -eq 'Allow' -and $rule.Enabled -eq 'True')
        Check "  every network type" ($rule.Profile -eq 'Any') "profile $($rule.Profile)"
        $app = $rule | Get-NetFirewallApplicationFilter
        Check "  only the service" ($app.Program -like "*$Folder\deskhop-service.exe") $app.Program
        $port = $rule | Get-NetFirewallPortFilter
        Check "  UDP $($Rules[$name])" ($port.Protocol -eq 'UDP' -and "$($port.LocalPort)" -eq "$($Rules[$name])") "$($port.Protocol) $($port.LocalPort)"
    }
    Check 'data folder created by the service' (Within 10 { Test-Path $Data })
}

function Assert-Removed {
    Check 'service removed' ($null -eq (Service))
    foreach ($name in $Rules.Keys) {
        Check "firewall rule '$name' removed" ($null -eq (Get-NetFirewallRule -DisplayName $name -ErrorAction SilentlyContinue))
    }
    Check 'install folder removed' (-not (Test-Path $Folder))
    Check 'Start menu shortcut removed' (-not (Test-Path $Shortcut))
    Check 'HKLM\SOFTWARE\DeskHop removed' (-not (Test-Path $Marker))
    Check 'no DeskHop entry in Installed apps' (@(Entries).Count -eq 0) "found $(@(Entries).Count)"
    Check 'data folder removed' (-not (Test-Path $Data))
    Check "$SasValue back to $(if ($null -eq $sasBefore) { 'not set' } else { $sasBefore })" ((Sas) -eq $sasBefore) "found $(Sas)"
}

$me = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $me.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host 'Run this from an elevated PowerShell (Run as administrator). Nothing was changed.'
    exit 1
}
$Msi = (Resolve-Path $Msi).Path
$UpgradeMsi = (Resolve-Path $UpgradeMsi).Path
$logs = Split-Path -Parent $Msi

Write-Host 'before'
Check 'no DeskHop service registered (uninstall the dev service or DeskHop first)' ($null -eq (Service))
$sasBefore = Sas
Write-Host "  $SasValue is $(if ($null -eq $sasBefore) { 'not set' } else { $sasBefore })"
$expected = if ($null -eq $sasBefore) { 1 } else { $sasBefore }

Write-Host 'install (MSI)'
$code = Msiexec "/i `"$Msi`"" (Join-Path $logs 'test-install.log')
Check 'msiexec succeeded without a reboot' ($code -eq 0) "exit code $code"
Assert-Installed
Check "$SasValue is $expected" ((Sas) -eq $expected) "found $(Sas)"
$created = (Get-Item -Path $Data -Force).CreationTimeUtc

Write-Host 'upgrade (MSI)'
$code = Msiexec "/i `"$UpgradeMsi`"" (Join-Path $logs 'test-upgrade.log')
Check 'msiexec succeeded without a reboot' ($code -eq 0) "exit code $code"
Assert-Installed
Check 'data folder kept (identity kept)' ((Get-Item -Path $Data -Force).CreationTimeUtc -eq $created)
Check "$SasValue is still $expected" ((Sas) -eq $expected) "found $(Sas)"

Write-Host 'uninstall (MSI)'
$code = Msiexec "/x `"$UpgradeMsi`"" (Join-Path $logs 'test-uninstall.log')
Check 'msiexec succeeded without a reboot' ($code -eq 0) "exit code $code"
Assert-Removed

if ($Setup) {
    $Setup = (Resolve-Path $Setup).Path

    Write-Host 'install (setup program)'
    $code = Run $Setup "/quiet /norestart /log `"$(Join-Path $logs 'test-setup-install.log')`""
    Check 'setup succeeded without a reboot' ($code -eq 0) "exit code $code"
    Assert-Installed
    Check "$SasValue is $expected" ((Sas) -eq $expected) "found $(Sas)"

    Write-Host 'uninstall (setup program)'
    $code = Run $Setup "/uninstall /quiet /norestart /log `"$(Join-Path $logs 'test-setup-uninstall.log')`""
    Check 'setup uninstall succeeded without a reboot' ($code -eq 0) "exit code $code"
    Assert-Removed
}

Write-Host 'all checks passed'
