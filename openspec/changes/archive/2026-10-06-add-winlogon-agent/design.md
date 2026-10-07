# Design

## Context

- **Supervision.** `bin/service/src/supervisor.rs` runs one agent.
  - `Plan<T>` is a pure, tested state machine for launch, deadline, backoff and ending.
  - `run` drives it, with `find_target` choosing the console session if a user is logged on there.
  - `main.rs` starts one `supervisor::run` task. `scm.rs` forwards session changes to it as `SupervisorCommand::SessionsChanged`.
- **Launching.** `launch::agent(target)` creates the pipe for the user's SID with `win32_session::create_agent_pipe`, then calls `win32_session::launch_as(token, exe, args, dir)`. That always uses `winsta0\default` and the user's token from `WTSQueryUserToken`. A test (`processes_start_only_here`) keeps every process start in `launch.rs`.
- **The host** (`host.rs`) holds one `link: Option<AgentLink>`.
  - It sends every `Decision::SetCapture`/`Inject`/`WarpCursor` to that link, and `SetClipboard` too.
  - It feeds the engine every `FromAgent::Input` and `Screen`.
  - `HostCommand::DropLink` ends the link, and the link ending sends `EngineEvent::LocalInputLost`.
- **The IPC link.**
  - `ServiceEnd::accept` is hard-wired to expect `Role::SessionAgent`, and `AgentEnd::connect` sends it.
  - `Role::WinlogonAgent` (code `2`) already exists in `ipc::message`, and `docs/protocol.md` already lists it.
  - `FromAgent` has `Input`, `Screen` and `Clipboard`.
- **The agent.**
  - `bin/agent/src/main.rs` parses `--winlogon` but exits at once.
  - `session.rs` is the link loop: capture, clipboard, commands.
  - `win32_input::start()` runs two threads, input hooks with Raw Input, and screen, on whatever desktop the process started on.
- **Versions:** `IPC_VERSION` is 1 and not shipped, so new types are added to version 1 without a bump, as the clipboard change did.
- **Settled with the user:**
  - the winlogon agent runs whether or not a user is logged on;
  - elevated windows on the user's desktop are a follow-up change.

## Goals / Non-Goals

**Goals:**
- Every requirement in `specs/secure-desktop`, and the deltas to `session-agents`, `agent-link` and `service-host`.
- Leave `engine` untouched. Choosing the agent is the host's job, and it feeds the engine only events the engine already handles.
- Give the new SYSTEM process the least power that still works.

**Non-Goals:**
- `uiAccess`, or an elevated session agent, for high-integrity windows on the user's desktop.
- Releasing keys held across a desktop switch beyond what the engine already does (see Risks).
- Any UI change. `Status.agent_linked` keeps meaning the session agent.

## Decisions

### D1. Spike first: hooks, Raw Input and injection on `Winlogon`

The whole design rests on these assumptions, which no unit test can check:
1. A SYSTEM process started on `winsta0\Winlogon` with privileges removed can install low-level hooks and register Raw Input there.
2. Those hooks and Raw Input see input while that desktop is the input desktop.
3. Its `SendInput` and `SetCursorPos` reach `consent.exe` and `LogonUI`.
4. The Windows 11 lock screen, UAC prompts and the logon screen all make `Winlogon` the input desktop.

The tasks build the token, the launch, the agent's mode and the supervisor first. The host then holds the winlogon agent's link and logs its desktop reports, with a count of the input events it reported while active (a count, not contents). Then the user checks (1), (2), (4) and the token through the registered service on this PC (`tools\dev\service.ps1`), before the host's routing is built on them.

(3) needs a second PC to inject from, so it joins the deferred two-PC pass. Until then, nothing checks that injection reaches the secure desktop.

If (1)–(3) fail with privileges removed, the fallback is to keep the default SYSTEM privileges and record that in the threat model. That fallback changes a spec requirement, so the change would pause for the user. If (2) fails for Raw Input alone, motion falls back to hook coordinates on that desktop. That is a `win32-input` detail, and no spec changes.

### D2. The winlogon agent's token: SYSTEM, restricted, moved to the console session

A new function in `win32-session`, `system_token(session)`, builds the token in three steps:
1. `OpenProcessToken` on the service itself (`TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT | TOKEN_ADJUST_SESSIONID`).
2. `CreateRestrictedToken(.., DISABLE_MAX_PRIVILEGE, ..)`, which removes every privilege except `SeChangeNotifyPrivilege` and gives a primary token.
3. `SetTokenInformation(TokenSessionId, session)`, which needs the service's `SeTcbPrivilege`.

The integrity level stays System. That is required: UIPI lets a process inject only into windows at or below its own level, and `consent.exe` and `LogonUI` run at System.

- **Alternative: a plain duplicate of the service token.** SeDebug, SeTcb, SeImpersonate and the rest would sit in a process in the user's session, a gift to anything that compromises it. Rejected; the agent needs none of them.
- **Alternative: LocalService, or the user's token.** Neither can open the `Winlogon` desktop, and both run below System integrity. Rejected.
- **Alternative: `DuplicateTokenEx`, then `AdjustTokenPrivileges` with `SE_PRIVILEGE_REMOVED` for each privilege.** Same result with more calls. `CreateRestrictedToken` does it in one.

### D3. One launch function, with the agent's kind

```rust
pub enum AgentKind { Session, Winlogon }
pub fn agent(kind: AgentKind, target: Target) -> Result<(Watch, NamedPipeServer), LaunchError>
```

| | Session | Winlogon |
|---|---|---|
| Token | `user_token(session)`, or none in console mode | `system_token(session)` |
| Pipe ACL | SYSTEM + the user's SID | SYSTEM only (`create_agent_pipe` with the SYSTEM SID, its SDDL with no second ACE) |
| Desktop | `winsta0\default` | `winsta0\Winlogon` |
| Arguments | `--session --pipe <name>` | `--winlogon --pipe <name>` |

- `launch_as` gains a `desktop` parameter, a small enum in `win32-session` (`Desktop::Default`, `Desktop::Winlogon`), not a free string.
- `Target::Console` with `AgentKind::Winlogon` is an error that never happens: console mode never asks for one (D4).
- `processes_start_only_here` keeps holding.

### D4. Two supervisor tasks, one `run` each

`supervisor::run` takes an `AgentKind`, and `main.rs` starts it twice. Each task has its own `Plan`, pipe, generation counter and backoff, which gives the independence the spec asks for with no change to `Plan`.

- **Fan-out.** `scm.rs` sends `SessionsChanged` to both tasks. `main.rs` stops both and waits for both.
- **Targets.** A pure function chooses where each agent runs:

  ```rust
  fn target(kind, console_mode, console_session: Option<SessionId>, user_logged_on: bool) -> Option<Target>
  ```

  - Session agent: as today.
  - Winlogon agent: `None` in console mode, otherwise `console_session.map(Target::Session)`.

  It gets a unit test per row, and `find_target` becomes a thin wrapper that reads Windows.
- **Reading the target again.** Each task also reads its target every second. Session notifications can arrive before Windows reports the new console session. The 5.4 check found this: after signing out, the logon screen moved to a new session, and the agent kept being launched into the dying one. `Plan::set_target` resets the backoff only when the target changes, so these rechecks never skip it.
- **Console mode.** The winlogon task logs once that the winlogon agent needs the registered service, then idles until stopped.
- **Handover.** `AgentLink` gains `kind`, and `HostCommand::DropLink` becomes `DropLink(AgentKind)`.

**Alternative: one task driving two `Plan`s.** It would double every arm of the `select!` loop for no gain. Rejected.

### D5. `ipc`: the role on each end, and messages per role

- **Choosing the role.** `ServiceEnd::accept(stream, AgentRole, clock)` and `AgentEnd::connect(stream, AgentRole, clock)`, where `AgentRole` is `Session` or `Winlogon` and maps to `Role` codes 1 and 2. The handshake checks the peer's role against it.
- **The new message.** `InputDesktop`, type `0x0019`, agent → service, with a 1-byte payload `active: u8`: `0` means another desktop has the input, `1` means this agent's desktop is the input desktop. Any other value is a protocol error. It surfaces as `ToService::InputDesktop(bool)` and `FromAgent::InputDesktop(bool)`.
- **Role checks.** The service reader gets one check after decoding:
  - `ClipboardText`, `ClipboardChunk` and `ClipboardNotShared` are allowed only from role 1;
  - `InputDesktop` only from role 2;
  - anything else is a new `ProtocolError::WrongRole { type, role }`, and the link closes.
- **Clipboard sends.** `AgentHandle::set_clipboard` on a winlogon link returns an error without sending.
- **Docs and fuzzing.** `docs/protocol.md` adds the row, notes "session agent → service" on the clipboard rows, and gives the per-role rule. The never-panic test and the `decode_ipc` fuzz target pick up the new type through the registry.

### D6. Reporting the input desktop: a 50 ms poll in `win32-input`

`win32_input::start` takes an option, `report_input_desktop: bool`. When it is set, the screen thread, which already runs a message loop with a hidden window, does three things:
- sets a 50 ms timer;
- on each tick, compares the name of its own desktop (`GetThreadDesktop`, then `GetUserObjectInformationW(UOI_NAME)`) with the input desktop's name (`OpenInputDesktop`, then the same call);
- emits `Captured::InputDesktop(bool)` on each change, plus once right after the first `Screen`.

If `OpenInputDesktop` fails, that means "not mine". It is what a process that cannot open the input desktop sees.

Putting it on the screen thread orders the first `InputDesktop` after the first `Screen`, as the spec requires, without the agent having to sequence them.

- **Comparing against the agent's own desktop**, not testing for the name `Winlogon`, keeps the agent testable on the user's desktop, where it reports `true`. It also makes no assumption about desktop names.
- **Alternative: `SetWinEventHook(EVENT_SYSTEM_DESKTOPSWITCH)`.** It would need no polling. But whether the event reaches a hook on the `Winlogon` desktop is undocumented, and two syscalls every 50 ms cost nothing measurable. Rejected for now; it can replace the timer later without a spec change.
- **Alternative: the session agent reports too.** That is redundant: the winlogon agent alone decides the active agent, and the session agent is active by default.

### D7. The agent binary: one link loop, two kinds

- **Shared loop.** `session.rs` becomes `link.rs` with `run(pipe, AgentRole)`.
  - The session kind starts capture without desktop reports and starts the clipboard, as today.
  - The winlogon kind starts capture with desktop reports and never touches `win32-clipboard`.
  - `Captured::InputDesktop` becomes `ToService::InputDesktop`. A session agent never receives it, because its option is off.
- **Arguments.** `parse_mode` accepts `--winlogon --pipe <name>`. Bare `--winlogon` becomes a usage error.
- **Unchanged:** the exit codes (3, 4, 5), and the rule that the link ending ends capture.

### D8. The host: two links and an active agent

`host.rs` replaces `link` with a small `Agents` value:

```rust
struct Agents {
    session: Option<AgentLink>,
    winlogon: Option<AgentLink>,
    secure: bool,                     // the winlogon agent's latest InputDesktop
    screens: [Option<Screen>; 2],     // last screen from each kind
    given: Option<Screen>,            // last screen given to the engine
}
```

- **Which agent is active.** `active()` is `Winlogon` when `secure` is set and a winlogon link exists, and `Session` otherwise.
- **Reading events.** `next_from` selects over both links and tags each event with its kind.

| Event | Effect |
|---|---|
| `Input` from the active kind | to the engine |
| `Input` from the other kind | dropped |
| `Screen` from kind k | stored for k. If k is active and the screen differs from `given`: `ScreenChanged`. |
| `InputDesktop(b)` (winlogon only) | `secure = b`. If the active kind changed: logged, and its stored screen is given if it differs. |
| `Clipboard` (session only, enforced by `ipc`) | as today |
| `SetCapture` decision | to every linked agent |
| `Inject`, `WarpCursor` decisions | to the active agent; dropped if it has no link |
| clipboard text to set | to the session agent only |
| link of kind k ends | if k was active: `LocalInputLost`. If k is winlogon: `secure = false`. Then logged and reported. |
| new link of kind k | gets the current capture mode first (existing rule), then is stored |

- **Status and console.** `Status.agent_linked` stays "session agent linked". The console's `Report::AgentLinked`/`AgentGone` carry the kind.
- **Tests.** Host tests in `host/tests.rs`, with two fake agent links per PC using the real `ipc` ends and the right roles, cover every secure-desktop scenario that does not need Windows. The routing table itself is unit-tested as a sequence of events in, commands out, like the engine's replay tests.

### D9. Threat model

`docs/threat-model.md` fills in the secure desktop. It replaces the status note and the "Service → winlogon agent" row, and adds a "Secure desktop (winlogon agent)" section. It covers:
- **The agent.** SYSTEM with no privileges but `SeChangeNotifyPrivilege`, at System integrity. Its pipe is SYSTEM-only, created fresh per launch, and the client's process ID is checked. It has no network, no clipboard and no files. Its hidden windows are on `Winlogon`, where only SYSTEM processes run. It is launched from the service's own folder, which must be writable only by administrators: the installer and the dev script use Program Files.
- **New power for a member.** A member that controls a machine can now:
  - answer its UAC prompts, including a consent prompt that asks an administrator for no password;
  - type at its lock and logon screens.
  This is what a physical keyboard can do. DeskHop never learns a password except as keystrokes in transit, encrypted on QUIC and never logged.
- **Local keystrokes on the secure desktop.** Passwords typed at the machine's own keyboard reach the service the same way desktop keystrokes do. They are never logged, and they are forwarded only while focus is on another machine.
- **Residual risks:**
  - a compromised member can approve a UAC consent prompt while it controls the machine and the prompt is up;
  - a crash of the winlogon agent returns focus to the machine, so the local user regains input.

## Risks / Trade-offs

- **[Hooks or Raw Input behave differently on `Winlogon`]** → D1 spikes them first, through the registered service, before the host and supervisor work.
- **[Removing privileges breaks something]** → D1 checks it on the same pass. The fallback pauses the change, because it alters a spec requirement.
- **[Keys held across a desktop switch]** For example, Win+L injected into B: the key-ups arrive just after the switch, and before the next 50 ms poll they go to the session agent, whose `SendInput` fails on a desktop that no longer has the input. A modifier can then look stuck on B. → The engine already releases what it injected when focus changes. The two-PC pass watches for stuck keys. If they appear, a follow-up can have the host release injected keys on the newly active agent when the active agent changes, without a spec change.
- **[Up to 50 ms of injection lost at a switch]** Keys typed in the instant a UAC prompt appears can go to the old desktop. → Accepted; the user sees the prompt before typing into it.
- **[A SYSTEM process in the user's session]** It is a larger target than the medium-integrity session agent. → D2 removes its privileges, and D9 bounds what it accepts: one SYSTEM-only pipe from the service, and window messages only from its desktop.
- **[Session switch gap]** When the console switches session, the winlogon agent is ended and relaunched, so input cannot reach the secure desktop for up to a second. → Accepted; it is the same as the session agent today.

## Migration Plan

Nothing to migrate. `tools\dev\service.ps1 install` already copies `deskhop-agent.exe` beside the service, so a reinstall brings the winlogon agent with it. Rolling back is reinstalling the previous build.

## Open Questions

- Windows 10 22H2 is not checked on hardware in this change. The user's machine runs Windows 11. The lock-screen and UAC checks are repeated on Windows 10 when the deferred two-PC pass runs. No spec or task changes depend on the answer.
