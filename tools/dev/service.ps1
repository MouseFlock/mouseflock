<#
.SYNOPSIS
Registers DeskHop's service on this PC for development, and removes it again.

.DESCRIPTION
Run from an elevated PowerShell (Run as administrator), from anywhere:

    tools\dev\service.ps1 install     build, copy to Program Files, register, open the firewall
    tools\dev\service.ps1 start       start the service
    tools\dev\service.ps1 status      service state and DeskHop processes
    tools\dev\service.ps1 log         the last 50 log lines
    tools\dev\service.ps1 stop        stop the service
    tools\dev\service.ps1 uninstall   undo everything install did, except the data folder

A SYSTEM service must never run from a folder users can write, such as this
repository, so install copies the binaries to "Program Files\DeskHop Dev".
Every change to the system is printed.

This is for development only; users install DeskHop's package
(installer\build.ps1). The two register the same service name, so each
refuses to install while the other is installed.
#>
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [ValidateSet('install', 'uninstall', 'start', 'stop', 'status', 'log')]
    [string]$Action
)

$ErrorActionPreference = 'Stop'
$Name = 'DeskHop'
$Repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$Dest = Join-Path $env:ProgramFiles 'DeskHop Dev'
$Exe = Join-Path $Dest 'deskhop-service.exe'
$Data = Join-Path $env:ProgramData 'DeskHop'
$Rules = @(
    @{ Name = 'DeskHop Dev (UDP 47391)'; Port = 47391 },
    @{ Name = 'DeskHop Dev (mDNS)'; Port = 5353 }
)
$SasKey = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System'
$SasValue = 'SoftwareSASGeneration'

function Say([string]$Message) { Write-Host "  $Message" }

function Assert-Elevated {
    $me = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    if (-not $me.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        Write-Host 'Run this from an elevated PowerShell (Run as administrator). Nothing was changed.'
        exit 1
    }
}

function Get-DeskHopService { Get-Service -Name $Name -ErrorAction SilentlyContinue }

# The package registers the same service name; the two never coexist, and this
# script never touches the package's service (add-installer design D8).
function Assert-NotInstalled {
    $installed = (Get-ItemProperty -Path 'HKLM:\SOFTWARE\DeskHop' -Name InstallFolder -ErrorAction SilentlyContinue).InstallFolder
    if ($installed) {
        Write-Host "DeskHop is installed in $installed by its setup program. Use Settings > Apps > DeskHop instead. Nothing was changed."
        exit 1
    }
}

function Stop-DeskHop {
    $svc = Get-DeskHopService
    if ($svc -and $svc.Status -ne 'Stopped') {
        Stop-Service -Name $Name
        Say "stopped service $Name"
    }
}

switch ($Action) {
    'install' {
        Assert-Elevated
        Assert-NotInstalled
        Push-Location $Repo
        try {
            cargo build -p deskhop-service -p deskhop-agent
            if ($LASTEXITCODE -ne 0) { throw 'cargo build failed; nothing was changed' }
        } finally { Pop-Location }

        Stop-DeskHop
        if (-not (Test-Path $Dest)) {
            New-Item -ItemType Directory -Path $Dest | Out-Null
            Say "created $Dest"
        }
        foreach ($bin in 'deskhop-service.exe', 'deskhop-agent.exe') {
            Copy-Item -Path (Join-Path $Repo "target\debug\$bin") -Destination $Dest -Force
            Say "copied $bin to $Dest"
        }

        if (-not (Get-DeskHopService)) {
            New-Service -Name $Name -BinaryPathName "`"$Exe`"" -DisplayName 'DeskHop' `
                -Description 'DeskHop software KVM (development build).' -StartupType Manual | Out-Null
            Say "registered service $Name (manual start, runs as SYSTEM)"
        } else {
            Say "service $Name was already registered; binaries updated"
        }

        foreach ($rule in $Rules) {
            if (-not (Get-NetFirewallRule -DisplayName $rule.Name -ErrorAction SilentlyContinue)) {
                New-NetFirewallRule -DisplayName $rule.Name -Direction Inbound -Protocol UDP `
                    -LocalPort $rule.Port -Program $Exe -Action Allow | Out-Null
                Say "added firewall rule '$($rule.Name)': inbound UDP $($rule.Port) for $Exe"
            }
        }

        $sas = (Get-ItemProperty -Path $SasKey -Name $SasValue -ErrorAction SilentlyContinue).$SasValue
        if ($null -eq $sas) {
            Set-ItemProperty -Path $SasKey -Name $SasValue -Value 1 -Type DWord
            Say "set $SasKey\$SasValue = 1 (services may show Ctrl-Alt-Del for a controlling peer)"
        } else {
            Say "$SasValue is already $sas; left as it is (1 or 3 lets services show Ctrl-Alt-Del)"
        }
        Write-Host "Installed. Start it with: tools\dev\service.ps1 start"
    }

    'uninstall' {
        Assert-Elevated
        # The service of the same name may be the package's; never remove that.
        Assert-NotInstalled
        Stop-DeskHop
        if (Get-DeskHopService) {
            sc.exe delete $Name | Out-Null
            Say "deleted service $Name"
        }
        foreach ($rule in $Rules) {
            if (Get-NetFirewallRule -DisplayName $rule.Name -ErrorAction SilentlyContinue) {
                Remove-NetFirewallRule -DisplayName $rule.Name
                Say "removed firewall rule '$($rule.Name)'"
            }
        }
        if (Test-Path $Dest) {
            Remove-Item -Recurse -Force $Dest
            Say "removed $Dest"
        }
        $sas = (Get-ItemProperty -Path $SasKey -Name $SasValue -ErrorAction SilentlyContinue).$SasValue
        if ($null -ne $sas) {
            Say "left $SasValue = $sas; remove it with: Remove-ItemProperty '$SasKey' $SasValue"
        }
        if (Test-Path $Data) {
            Say "left the data folder $Data (identity, desk, layouts, log). Only SYSTEM can open it; to remove it:"
            Say "  takeown /F `"$Data`" /R /D Y; icacls `"$Data`" /grant Administrators:F /T; Remove-Item -Recurse `"$Data`""
        }
        Write-Host 'Uninstalled.'
    }

    'start' {
        Assert-Elevated
        Start-Service -Name $Name
        Get-DeskHopService | Format-Table -AutoSize Name, Status
    }

    'stop' {
        Assert-Elevated
        Stop-DeskHop
    }

    'status' {
        $svc = Get-DeskHopService
        if ($svc) { $svc | Format-Table -AutoSize Name, Status, StartType } else { Write-Host "service $Name is not registered" }
        Get-Process -Name deskhop-service, deskhop-agent -ErrorAction SilentlyContinue |
            Format-Table -AutoSize Id, ProcessName, SessionId, StartTime
    }

    'log' {
        Assert-Elevated
        # The folder is SYSTEM-only; the log file itself lets Administrators
        # read it, so open it by path rather than listing the folder.
        $log = Join-Path $Data 'service.log'
        try { Get-Content -Path $log -Tail 50 -ErrorAction Stop }
        catch { Write-Host "cannot read ${log}: $($_.Exception.Message)" }
    }
}
