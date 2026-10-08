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

$session = (Get-Process -Id $PID).SessionId
$agent = Get-Process -Name deskhop-agent -ErrorAction SilentlyContinue |
    Where-Object SessionId -eq $session |
    Select-Object -First 1
if (-not $agent) {
    Write-Host "No session agent (deskhop-agent.exe) found in session $session. Is DeskHop running and are you logged on?"
    exit 1
}
Write-Host "Session agent PID $($agent.Id) in session $session."

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

$failures = 0
foreach ($name in $rights.Keys) {
    $h = [Probe]::OpenProcess($rights[$name], $false, [uint32]$agent.Id)
    if ($h -ne [IntPtr]::Zero) {
        [void][Probe]::CloseHandle($h)
        $verdict = 'allowed'
    } else {
        $verdict = 'denied'
    }
    '{0,-32} {1}' -f $name, $verdict
    if ($verdict -eq 'allowed' -and $name -ne 'PROCESS_TERMINATE') { $failures++ }
}

if ($failures -gt 0) {
    Write-Host "`nSHIELD NOT HOLDING: $failures tampering right(s) were allowed."
    exit 1
}
Write-Host "`nShield holds: only terminate is allowed."
