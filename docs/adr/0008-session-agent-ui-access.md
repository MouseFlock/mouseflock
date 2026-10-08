# ADR 0008 — The session agent runs with UI access

**Status:** Proposed
**Date:** 2026-10-08
**Refines:** ADR 0002 (Process topology)

## Context

ADR 0002 lists elevated windows on the user's desktop as one of the hard moments DeskHop must handle, and names a `uiAccess=true` manifest as a possible later layer, "requires signed install under Program Files".

The two-PC check on 2026-10-08 confirmed the gap. The session agent runs as the logged-on user at medium integrity. Windows' User Interface Privilege Isolation (UIPI) stops a medium-integrity process from injecting input into, or hooking, a higher-integrity window, and reports neither a failed return nor an error code when it does. So when the controlled PC's foreground window is elevated — an admin terminal, Task Manager, a window a UAC prompt opened — the controller's forwarded input silently vanishes, and `docs/known-issues.md` recorded the trap.

The manifest route needs an Authenticode signature chaining to a trusted root, in a secure folder. DeskHop is not yet code-signed, and a signed-only fix would break anyone building the auditable client from source.

## Decision

- **The registered service grants the session agent UI access through its token**, not a manifest. Before launching the agent it takes the user's token, copies it, raises the copy to Medium Plus integrity, and sets `TokenUIAccess`. Setting that flag needs SYSTEM's `SeTcbPrivilege`, which the service has, and skips the signature and install-path checks the shell's launch path would apply. No code signing is required, and a self-built agent keeps the fix.
  - The agent still runs **as the user**, not as an administrator, and gains no privilege. UI access is the same flag an on-screen keyboard or screen reader carries; it lets the process work with higher-integrity windows, nothing more.
- **The agent is shielded.** A process with UI access must not be hijackable by the user's other programs, or malware running as the user could drive admin windows through it. So the agent runs at Medium Plus integrity, above every unelevated program, behind a process and thread security descriptor that grants the user only synchronize, query-limited and terminate. Mandatory integrity and UIPI then block lower processes from writing its memory, creating threads in it, changing its security, or messaging its windows.
- **It fails safe.** If Windows refuses any step, the service launches the agent with the plain user token and no shield, as before, and logs once why. The agent then works with every window except elevated ones. Console mode cannot set the flag and logs that elevated windows need the registered service.

## Alternatives considered

- **A `uiAccess="true"` manifest, launched through the shell.** Needs signing and a secure folder; breaks source builds. Rejected for now; it remains available once a signing pipeline exists.
- **The user's linked (elevated) token.** Covers only administrators, and runs the whole agent — clipboard included — elevated. A larger blast radius for no gain over the token route. Rejected.
- **A SYSTEM helper on the user's desktop.** SYSTEM code taking window messages on the user's desktop is the shatter-attack exposure that session 0 isolation exists to prevent. Rejected.

## Consequences

- Typing, clicking and moving over the user's elevated windows works from another PC, closing the last everyday gap in ADR 0002's reliability promise.
- A member controlling a machine can now type into its elevated windows. That is what the user's own keyboard at that machine can do; the threat model records it. A member could already answer UAC prompts via the winlogon agent.
- The token work stays in `win32-session`, linked only by `bin/service`, with the launch still going through `launch::agent`. No new crate, no protocol change.
- The fix does not depend on code signing. A future Windows build that stops honoring the flag set this way falls back to the logged, plain launch, and the signed manifest route remains open.
