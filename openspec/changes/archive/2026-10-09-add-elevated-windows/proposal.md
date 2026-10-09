# Proposal

## Why

The two-PC check on 2026-10-08 found a trap, now in `docs/known-issues.md`. When the controlled PC's foreground window runs elevated, such as an admin Notepad or Task Manager, the controlling PC's keyboard and mouse stop working:

- **Input is dropped.** Windows' User Interface Privilege Isolation (UIPI) silently drops everything the medium-integrity session agent injects into that window, cursor motion included.
- **The controller is stuck.** It is forwarding and withholds its own input, and the controlled PC's cursor cannot move to the edge that leads back.
- **Recovery is awkward.** The controlled PC's own mouse, moved twice outside the window, recovers; so now does Scroll Lock twice (add-escape-key), but input still never reaches the elevated window.

The user reproduced this on two PCs: an admin Notepad traps the controller, and a normal Notepad does not. ADR 0002 names elevated windows as one of the hard moments the product is for. The winlogon agent fixed UAC prompts and the lock screen, but the elevated window a UAC prompt opens still breaks DeskHop.

## What Changes

- **A spike comes first, and gates the rest.** A throwaway test program, run once as SYSTEM through a scheduled task, launches a test agent in the user's session with UI access. The installed DeskHop is left untouched. The spike answers:
  - whether injected keys, clicks and motion reach an elevated window;
  - whether the agent's hooks see physical input while an elevated window has focus;
  - whether other unelevated programs of the same user can tamper with the agent, and which integrity label and security descriptor stop them while the clipboard still works;
  - whether Windows Security reacts;
  - whether the agent's folder matters, and how Windows 10 22H2 behaves.

  The results go in `design.md`. If the token route fails, the change stops there and records why. The fallback would be the signed `uiAccess` manifest route, which waits for code signing.
- **The session agent runs with UI access.**
  - The registered service sets `TokenUIAccess` on the session user's token before launching the agent. Setting it needs `SeTcbPrivilege`, which only SYSTEM has, and no code signing.
  - With UI access, Windows lets the agent inject into, and hook, the user's elevated windows.
  - The agent still runs as the user, not as an administrator, and its token gains no privileges.
- **The agent is shielded from the user's other programs.** A process with UI access can drive admin windows, so it must not be hijackable.
  - The agent runs at an integrity level above medium, so unelevated programs of the same user cannot write its memory, start threads in it, change its security, or send messages to its windows.
  - Its process gets a security descriptor that doesn't hand the user's programs those rights back. The spike picks the exact label and descriptor.
- **Fails safe.** If Windows refuses UI access, the service launches the agent as today, without it, and logs that once. Console mode never has UI access, since it cannot set the flag, and logs that once.
- **What the user sees:**
  - With the cursor on a PC, they can type into, click in and move over its elevated windows, and cross back out of them.
  - At a PC whose own foreground window is elevated, the cursor crosses to another PC, and local input takes control back, as it does over any other window.
- **Docs:**
  - **ADR 0008** (Proposed) refines ADR 0002: the session agent runs with UI access granted by the service, and the signed manifest route is no longer required.
  - `docs/threat-model.md` covers the shielded UI-access agent and what a member can now do: type into the user's elevated windows.
  - AGENTS.md's process topology line for the session agent, and `docs/known-issues.md` once the trap is fixed.
- **Out of scope:**
  - Code signing and the manifest route.
  - Reporting injection that Windows blocks.
  - RDP sessions.
  - Any change to the winlogon agent or the secure desktop.

## Capabilities

### New Capabilities

None.

### Modified Capabilities
- `session-agents`: the session agent is launched with UI access at a raised integrity level, shielded from the user's unelevated processes. It falls back to a plain launch, logged once, if Windows refuses. Console mode has no UI access.
- `input-capture`: injected input reaches the user's elevated foreground windows, and physical input aimed at them is reported and withheld by capture mode like any other, when the agent runs with UI access.
- `agent-link`: the blocked-injection example now refers to console mode, where the agent has no UI access.

## Impact

- **Crates touched:**
  - **`win32-session`** (service only):
    - a session user token with UI access and a raised integrity level;
    - reading those back from a token, for the log and tests;
    - launching with a process and thread security descriptor.
  - **`bin/service`:** `launch::agent` asks for UI access for the session agent and logs the outcome. The supervisor logs a fallback once.
  - **Expected no change:** `win32-input`, `bin/agent`. The spike confirms that hooks and `SendInput` need no new calls.
  - **No change:** `engine`, `model`, `proto`, `ipc`, `transport`, `pairing`, `policy`, `win32-clipboard`, `win32-shell`, `apps/desktop`, `installer`. No protocol version bump.
- **Dependency rules hold:**
  - Only `deskhop-service` links `win32-session`, where the token and launch calls live. Every launch still goes through `launch::agent`.
  - No new `unsafe` crate, and no new dependency in `proto`, `model` or `engine`.
  - The spike program is not part of the workspace and is not committed. Its findings are.
- **No grid, no server/client toggle, no IP address field.**
- **Testing:**
  - Unit tests read a token's UI access flag and integrity level. Setting them needs SYSTEM, so the parts that need SYSTEM are checked by hand.
  - The user runs the spike and the final checks on two PCs: a build from `installer\build.ps1` installed as an upgrade, which also covers add-installer 6.3's "an upgrade keeps the pairing".
