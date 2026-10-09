<#
.SYNOPSIS
Checks that the session agent is shielded from the user's own programs
(add-elevated-windows D3).

.DESCRIPTION
Run from a NORMAL (unelevated) PowerShell, as the logged-on user, while
DeskHop is running. It finds the session agent (deskhop-agent.exe in this
session) and tries to open its process for the access an attacker would use
to hijack a UI-access process: writing its memory, creating a thread in it,
and rewriting its security. With the shield in place, Windows denies each of
those, and allows only terminate (which the service recovers from by
relaunching).

    tools\dev\probe-agent-shield.ps1

Expected, when the agent has UI access and the shield:

    PROCESS_VM_WRITE|VM_OPERATION   denied
    PROCESS_CREATE_THREAD           denied
    WRITE_DAC                       denied
    PROCESS_TERMINATE               allowed

If any of the first three is "allowed", the shield is not holding; report it.
If the agent has no UI access (the fallback launch), it is an ordinary
medium-integrity process and these may be allowed; check the service log for
"UI access: yes" first.
#>

$ErrorActionPreference = 'Stop'

$me = [Security.Principal.WindowsIdentity]::GetCurrent()
if ($me.Owner.IsWellKnown([Security.Principal.WellKnownSidType]::LocalSystemSid)) {
    Write-Host 'Run this as the logged-on user from a NORMAL PowerShell, not as SYSTEM or elevated.'
    exit 1
}

# The session agent runs as the user; the winlogon agent runs as SYSTEM.
# Probe the session agent: the shield's point is that the user's own programs
# cannot tamper with it even though it runs as the user. (The winlogon agent
# is SYSTEM, so everything is denied for a different reason, which would not
# test this shield.) Pick it by owner, since its command line is unreadable
# across the integrity boundary; GetOwner runs via the WMI service, so it is
# not.
$session = (Get-Process -Id $PID).SessionId
$me = [Security.Principal.WindowsIdentity]::GetCurrent().Name
$proc = Get-CimInstance Win32_Process -Filter "Name='deskhop-agent.exe'" -ErrorAction SilentlyContinue |
    Where-Object { $_.SessionId -eq $session } |
    Where-Object {
        $o = Invoke-CimMethod -InputObject $_ -MethodName GetOwner
        $o.User -and "$($o.Domain)\$($o.User)" -eq $me
    } |
    Select-Object -First 1
if (-not $proc) {
    Write-Host "No session agent (deskhop-agent.exe running as $me) found in session $session. Is DeskHop running and are you logged on?"
    exit 1
}
$agent = [pscustomobject]@{ Id = [int]$proc.ProcessId }
Write-Host "Session agent PID $($agent.Id) in session $session (runs as you)."

Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class Probe {
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern IntPtr OpenProcess(uint access, bool inherit, uint pid);
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool CloseHandle(IntPtr h);
}
'@

# Access rights to try: the ones that would let a program tamper with the
# agent, plus terminate (which the shield intentionally still allows).
$rights = [ordered]@{
    'PROCESS_VM_WRITE|VM_OPERATION' = 0x0020 -bor 0x0008
    'PROCESS_CREATE_THREAD'         = 0x0002
    'WRITE_DAC'                     = 0x00040000
    'PROCESS_TERMINATE'             = 0x0001
}

$allowed = @{}
foreach ($name in $rights.Keys) {
    $h = [Probe]::OpenProcess($rights[$name], $false, [uint32]$agent.Id)
    if ($h -ne [IntPtr]::Zero) {
        [void][Probe]::CloseHandle($h)
        $allowed[$name] = $true
    } else {
        $allowed[$name] = $false
    }
    '{0,-32} {1}' -f $name, $(if ($allowed[$name]) { 'allowed' } else { 'denied' })
}

$tampering = $rights.Keys | Where-Object { $_ -ne 'PROCESS_TERMINATE' -and $allowed[$_] }
if ($tampering) {
    Write-Host "`nSHIELD NOT HOLDING: these tampering rights were allowed: $($tampering -join ', ')"
    exit 1
}
if ($allowed['PROCESS_TERMINATE']) {
    Write-Host "`nShield holds: tampering is denied, and only terminate is allowed (the service relaunches)."
} else {
    Write-Host "`nTampering is denied. Terminate was also denied, which is unexpected for the session agent that runs as you - check this is the --session agent, not the SYSTEM winlogon agent."
}
