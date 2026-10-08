# Design

## Context

- **Launching.** `launch::agent(role, target)` in `bin/service/src/launch.rs` is the one launch function.
  - For the session agent in a session, it takes the user's token from `win32_session::user_token` (`WTSQueryUserToken`). For an administrator, that is the filtered, medium-integrity token.
  - It then calls `win32_session::launch_as(token, exe, args, dir, desktop)`, which calls `CreateProcessAsUserW` with no process or thread security attributes. So the agent's process and threads get the token's default DACL, which gives the user full access.
  - The supervisor logs each launch, plus the winlogon agent's privileges (`supervisor.rs`).
- **Tokens.** `win32-session/src/token.rs` already builds the winlogon agent's token: SYSTEM, restricted, moved to a session with `SetTokenInformation(TokenSessionId)`, which needs the service's `SeTcbPrivilege`. Its `Token` type can read privileges and the user SID.
- **The agent's windows.**
  - The input thread has a hidden `STATIC` window for Raw Input.
  - The screen thread has a top-level hidden window that listens for `WM_DISPLAYCHANGE`, `WM_DPICHANGED`, `WM_SETTINGCHANGE` and `WM_DEVICECHANGE`.
  - The clipboard thread has a listener window for `WM_CLIPBOARDUPDATE` and `WM_APP`.
  - All are top-level, so they receive broadcasts.
- **What breaks today** (see proposal.md, Why): the session agent runs at medium integrity, so UIPI drops its `SendInput` into a high-integrity foreground window without an error, and its low-level hooks may not see input aimed there. ADR 0002 lists `uiAccess=true` as an option "may be layered later", assuming a signed install.
- **Settled with the user:**
  - the token route, without code signing, gated on a spike;
  - the spike runs as a throwaway program, leaving the installed DeskHop untouched;
  - the escape key (add-escape-key) is already a safety net; this change fixes the cause.

## Goals / Non-Goals

**Goals:**
- Every requirement in the deltas to `session-agents`, `input-capture` and `agent-link`.
- Prove the approach with a spike before any product code changes.
- Give the agent UI access only together with a shield against the user's unelevated programs.

**Non-Goals:**
- Changes to `win32-input`'s hooks or injection, unless the spike shows they are needed (D6).
- The secure desktop, the winlogon agent, or how the service routes between agents.
- Detecting blocked injection, which Windows does not report (`SendInput` documentation).

## Decisions

> **Decision changed (2026-10-08, with the user):** the separate throwaway spike is dropped. The product code (D2–D5) was small and is covered by fail-clean unit tests, so the approach is verified directly with a real 0.3.0 build in the two-PC check (D8) instead of a throwaway program run as SYSTEM. A small shield-probe script (`tools/dev/probe-agent-shield.ps1`) performs D1's tamper check against the real agent. The original spike design is kept below for the record.

### D1. Spike first, outside the workspace

A throwaway Rust program, `uiaccess-spike.exe`, lives in the session scratchpad. It has its own `Cargo.toml`, uses the workspace's `windows` crate version, and is not committed, so the `unsafe` rules in AGENTS.md don't apply to it. It has three modes:

- **`launch`**, run as SYSTEM:
  - takes the console session's user token, duplicates it as a primary token, and optionally sets `TokenIntegrityLevel` and `TokenUIAccess`;
  - launches `uiaccess-spike.exe agent` on `winsta0\default`, optionally with a process and thread security descriptor;
  - logs every call's result to `C:\Windows\Temp\uiaccess-spike.log`.
- **`agent`**, launched by `launch`, runs as the user:
  - logs its own token: UI access, integrity level and privileges, plus its PID, main thread ID and hidden window handle;
  - installs low-level keyboard and mouse hooks and counts the events they see, with the integrity level of the foreground window's process at each event;
  - after a 10-second countdown, so the user can focus an elevated Notepad, injects "uiaccess ok", moves the mouse in a square and clicks once;
  - sets the clipboard to a marker text and reads back what the user copies;
  - logs `WM_SETTINGCHANGE` and `WM_DISPLAYCHANGE` arrivals.

  It logs to the user's `%TEMP%`.
- **`probe <pid> <tid> <hwnd>`**, run as the user, unelevated: tries to open the agent's process for each of `PROCESS_VM_WRITE | PROCESS_VM_OPERATION`, `PROCESS_CREATE_THREAD`, `WRITE_DAC` and `PROCESS_TERMINATE`, and its thread for `THREAD_SET_CONTEXT`. It also posts `WM_APP` to the window, and reports each result.

**Running as SYSTEM.** A one-off scheduled task (`schtasks /create /ru SYSTEM`, `/run`, `/delete`) runs `launch` from `C:\Program Files\DeskHop Spike\`, which only administrators can write. The user runs these elevated steps. The installed DeskHop keeps running beside it. The spike agent links to nothing, and its hooks only count, passing everything through.

**The runs:**

| Run | UI access | Integrity | Security descriptor | Answers |
|---|---|---|---|---|
| A | no | medium | default | Control: reproduces today's block |
| B | yes | medium | default | Does the token route grant UI access at all? |
| C | yes | Medium Plus | default | Is UI access kept at the raised level? Do the clipboard and setting changes still arrive? |
| D | yes | Medium Plus | D3's descriptor | The shield: what `probe` gets |
| E | yes | Medium Plus | D3's descriptor | Run D with the agent half started from a user-writable folder: does the folder matter? |

Each run is checked:
- **With an elevated Notepad in front:** typing, motion, the click, and hook counts while the user types physically.
- **With a normal Notepad in front:** the same, as a control.
- **Afterwards:** Windows Security's protection history.

Windows 10 22H2 is run too, if a machine is available.

**Results** go in "Spike results" below, and the rest of the tasks follow them:
- **B fails:** the change stops. Its tasks are marked as not done, with the reason. The signed-manifest route waits for code signing.
- **C fails** (the raised level loses UI access, the clipboard, or setting changes): the change pauses for the user.
  - **If `probe` shows D3's descriptor alone is enough at medium integrity:** the shield could fall back to it. That changes the spec's "Shield cannot be applied" scenario, which is why the change pauses.
  - **Otherwise:** the change stops, since the spec forbids UI access without the shield.

### D2. UI access through the token, set by the service

`win32-session` gains a function that builds the session agent's token. In order:
1. `WTSQueryUserToken`.
2. `DuplicateTokenEx` to a primary token with the access the next steps need.
3. `SetTokenInformation(TokenIntegrityLevel, Medium Plus)` (D3).
4. `SetTokenInformation(TokenUIAccess, 1)`. This needs `SeTcbPrivilege`, which LocalSystem holds and has enabled.

Raising the level comes before UI access, so a failure leaves no half-shielded token in use.

Why this route:
- **No signing:** the signature and Program Files checks belong to the shell's launch path (AppInfo), which this route doesn't use.
- **Forks and self-builds keep the fix.** That matters for an auditable open-source client.
- **The service already builds tokens here** for the winlogon agent.

Alternatives considered:
- **A `uiAccess="true"` manifest, launched through the shell.** It needs an Authenticode signature chaining to a trusted root, and a secure folder. It breaks self-builds, and depends on a signing pipeline that doesn't exist. Rejected for now; possibly layered on later.
- **The user's linked (elevated) token.** It covers only administrators, and runs the whole agent, clipboard included, elevated. That's a larger blast radius. Rejected.
- **A SYSTEM helper on the user's desktop.** SYSTEM code taking window messages on the user's desktop is the classic "shatter" exposure that session 0 isolation exists to prevent. Rejected.

### D3. The shield: Medium Plus integrity, and a tight process descriptor

**Integrity: Medium Plus (`S-1-16-8448`).** The agent's token is raised to Medium Plus, so the process, its threads and its windows sit above every unelevated program:
- **Mandatory integrity** denies write-type access (memory, threads, `WRITE_DAC`) from a lower level, whatever the DACL or ownership says.
- **UIPI** blocks window messages from lower levels.

Medium Plus is the lowest level that does this. High would make an unelevated agent look elevated, for no gain.

**The process and thread descriptor**, passed to `CreateProcessAsUserW`:
- **SYSTEM:** full access.
- **The user:** `SYNCHRONIZE`, `PROCESS_QUERY_LIMITED_INFORMATION` and `PROCESS_TERMINATE`. Task Manager can still end it, and the service restarts it, as the spec allows.
- **An `OWNER RIGHTS` ACE** granting nothing more, which removes the owner's implicit `WRITE_DAC`.
- **A Medium Plus no-write-up label.**

The descriptor alone isn't enough: without the raised level, the user's processes could still try owner-based tricks. Together they're defense in depth. The spike (run D) checks what `probe` actually gets.

**Setting changes.** UIPI also drops `WM_SETTINGCHANGE` that unelevated programs broadcast, such as Settings after a scaling change. If run C confirms this, the screen window allows that one message through with `ChangeWindowMessageFilterEx`. That's the exception the spec states. Monitor and DPI changes sent by Windows itself (`WM_DISPLAYCHANGE`, `WM_DPICHANGED`, `WM_DEVICECHANGE`) are unaffected.

### D4. Fail safe, log once

`launch::agent` asks for the UI-access token for the session agent in a session. If any step of D2 fails, or launching with D3's descriptor fails:
- it launches with the plain user token and no descriptor, as today;
- the supervisor logs why, once per service run.

The launch log line gains `UI access: yes|no`, as the winlogon agent's line lists its privileges. Console mode can't set the flag. It launches as today, and the supervisor logs once that elevated windows need the registered service, as it already does for the winlogon agent.

### D5. Token readers for the log and tests

`Token` gains:
- `ui_access() -> io::Result<bool>`, which reads `TokenUIAccess`;
- `integrity() -> io::Result<u32>`, which reads the RID of `TokenIntegrityLevel`.

Unit tests read them on the test process's own token: no UI access, medium or high. Building the agent token fails cleanly without SYSTEM, in the existing "fails cleanly" test style. Everything that needs SYSTEM is checked by hand (ADR 0006).

### D6. The agent itself is expected not to change

UI access is a property of the token. Hooks, Raw Input, `SendInput` and `SetCursorPos` need no new calls. The only expected agent-side change is D3's message filter in `win32-input`'s screen thread, if run C shows it is needed. If the spike shows anything else must change in `win32-input` or `win32-clipboard`, that's recorded and added to the tasks before building.

### D7. ADR 0008

ADR 0008 (Proposed) refines ADR 0002's process table:
- the session agent runs with UI access, granted by the service through its token;
- it runs at Medium Plus behind a tight descriptor;
- the "requires signed install" assumption in ADR 0002's alternatives no longer holds for this route.

ADR 0002 stays as it is.

### D8. Checking the result

- **Unit tests:** D5.
- **The user, on two PCs.** Both get a build from `installer\build.ps1 -Version 0.3.0`, installed over the current version as an upgrade. That also covers add-installer 6.3's "an upgrade keeps the pairing". Then:
  - the trap from the proposal, now passing: an elevated Notepad and Task Manager on the controlled PC;
  - crossing out from, and taking control back over, an elevated window at a PC's own keyboard;
  - the `probe` from the spike against the real agent;
  - scaling changed in Settings;
  - the clipboard, text and images, still both ways.

## Risks / Trade-offs

- **[A member can now type into the user's elevated windows]** → That's what the user's own keyboard at that PC can do. It's the purpose of the change, and the threat model says so. A member could already answer UAC prompts.
- **[A hijacked agent would bypass UIPI]** → That's why UI access comes only with the shield (D3), checked by `probe`. Elevated processes of the user can still tamper, but they are already administrators.
- **[Security software flags `TokenUIAccess` set by a service]** → The spike checks Windows Security. Corporate EDR can't be checked beyond the user's two PCs. If it objects, the fallback (D4) keeps DeskHop working without elevated windows.
- **[A future Windows build stops honoring the flag set this way]** → D4's fallback, logged once. The signed manifest route remains available later.
- **[Medium Plus breaks something the spike didn't try]**, such as IME or accessibility tools reading the agent → The agent has no visible UI or text input. The final two-PC check exercises the clipboard and screen changes.
- **[The spike runs a SYSTEM task]** → It runs from an administrators-only folder and is deleted afterwards. Only the agent half is ever started from a user folder (run E), and that half runs as the user.

## Migration Plan

- **Rollout:** an upgrade replaces the agent, and the service launches it with UI access from then on. There's no data or protocol change.
- **Rollback:** uninstall and install the older version. The data folder is removed with the uninstall, so pairing is redone.

## Open Questions

- **Windows 10 22H2:** whether a machine is available for the spike's Windows 10 run. The approach doesn't depend on the answer.
- **ARM64:** the token calls are architecture-neutral, and no ARM64 PC is available. Checked in the deferred multi-PC pass.

## Spike results

*Filled in by task 1.*
