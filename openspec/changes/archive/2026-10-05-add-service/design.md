# Design

## Context

- **`bin/service`:** an empty `main` with `#![forbid(unsafe_code)]`. It depends on `ipc`, `pairing`, `policy` and `transport`.
- **`transport::node`:**
  - `start(Config)` returns a `NodeHandle` and an event receiver.
  - `Config` takes the `Machine` with its trust store loaded, plus `addresses`, `port`, `discovery` and a test `network`.
  - Events: `Paired`, `JoinFailed`, `PairingClosed`, `PeerUp`, `PeerDown`, `TrustStore(bytes)`, `Addresses(map)`, and `Message { peer, msg }`.
  - Commands: `open_pairing`, `close_pairing`, `join`, `remove`, `goodbye`, `resume`, `send` and `shutdown`.
  - `transport::node::Backoff` is the 0.5 s doubling to 30 s policy.
- **`engine::Engine`:** `handle(Event) -> Vec<Decision>`.
  - It has no clock, and `Undo` carries a time.
  - Decisions: `SetCapture`, `Send`, `Inject`, `WarpCursor`, `RequestSecureAttention`, `FocusChanged`, `LayoutsChanged` and `OfferUndo`.
  - Peer loss is `Event::PeerLost`. Nothing tells it that local input went away.
- **`ipc::link::ServiceEnd::accept(stream, clock)`:** returns an `AgentHandle`, which never blocks, and a receiver of `FromAgent::{Input, Screen}`. Times are already on the clock it was given. Dropping the handle ends the link, and the agent then passes all and exits 0.
- **`pairing`:** `Identity` (37-byte file), `Desk::from_file`/`to_file` (`DHTS`), and `MachineName` (1–63 bytes). `model::LayoutBook` and the address map have no file format yet.
- **Settled with the user:**
  - pairing happens through console commands until `add-onboarding`;
  - console mode keeps its own data folder;
  - the new `win32-session` crate comes with ADR 0006;
  - the winlogon agent comes later.

## Goals / Non-Goals

**Goals:**
- Every requirement in `specs/service-host`, `specs/session-agents` and the `input-focus` delta.
- The service core (host loop, store, console commands) testable in-process, without SYSTEM: two hosts on loopback with fake agents over `tokio::io::duplex`.
- All Win32 calls behind small safe functions in `win32-session`.

**Non-Goals:**
- UI pipe and UI pairing (`add-onboarding`).
- Winlogon agent and the secure desktop.
- RDP and other non-console sessions.
- Managed configuration: `policy` stays a stub.
- Edge-sensitivity settings: the engine default applies.
- The installer and firewall rules. The dev script adds a rule only for the dev copy.

## Decisions

### D1. ADR 0006 and `win32-session`

ADR 0006 adds a third crate where `unsafe` is allowed, `crates/win32-session`, linked only by `bin/service`.

*Alternative:* `unsafe` in `bin/service` itself. That breaks rule 3 for the process that runs as SYSTEM, which is the one that most needs the audit boundary.

**Safe API:**

| Function | Wraps |
|---|---|
| `active_console_session() -> Option<SessionId>` | `WTSGetActiveConsoleSessionId` (`0xFFFFFFFF` → `None`) |
| `user_token(SessionId) -> Result<Token, NoUser \| Error>` | `WTSQueryUserToken`; `ERROR_NO_TOKEN` → `NoUser`. Needs SYSTEM. |
| `Token::user_sid() -> Sid`, `own_sid() -> Sid` | `GetTokenInformation(TokenUser)` |
| `launch_as(&Token, exe, args, dir) -> Process` | `CreateEnvironmentBlock` + `CreateProcessAsUserW`, desktop `winsta0\default`, `CREATE_UNICODE_ENVIRONMENT \| CREATE_NO_WINDOW`, no inherited handles |
| `Process::{id, wait (blocking), terminate}` | process handle, kept open for its lifetime so the ID cannot be reused |
| `create_agent_pipe(name, allowed: &Sid) -> NamedPipeServer` | SDDL `D:P(A;;GA;;;SY)(A;;GRGW;;;<sid>)` through `ConvertStringSecurityDescriptorToSecurityDescriptorW`, then tokio `ServerOptions` with `first_pipe_instance(true)`, `reject_remote_clients(true)` and `max_instances(1)`, via `create_with_security_attributes_raw` |
| `client_process_id(&NamedPipeServer) -> u32` | `GetNamedPipeClientProcessId` |
| `create_admin_readable(path)` | `CreateFileW` with `CREATE_NEW` and the log's descriptor; an existing file is left alone (D12) |
| `secure_folder(path) -> Result<Prepared, Error>` | `GetNamedSecurityInfoW` (owner) and `SetNamedSecurityInfoW` (owner SYSTEM, protected DACL `D:P(A;OICI;FA;;;SY)`), `CreateDirectoryW` with that descriptor; see D7 |
| `sas_policy_allows_services() -> bool`, `send_sas()` | `RegGetValueW` on `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System\SoftwareSASGeneration` (1 or 3), and `SendSAS(FALSE)` from `sas.dll`, declared with `#[link]` if the `windows` crate lacks it |

**Pipe rights:** `GRGW` is what tokio's `ClientOptions` asks for. Generic write also includes the right to create a pipe instance. That is harmless here: the first instance was created with a maximum of one, so no second instance of that name can exist while ours does, and the name is never reused.

**SCM entry:** `windows-service`'s `define_windows_service!` expands to an `unsafe` block.
- If `forbid(unsafe_code)` in `bin/service` rejects that expansion, `win32-session` hosts the entry: `scm::run_dispatcher(name, fn(Vec<OsString>))`, with the handler stored in a `OnceLock`.
- Either way, the control handler and status reporting use `windows-service`'s safe API from `bin/service`.

**`archcheck`:**
- `UNSAFE_CRATES` gains `win32-session`.
- A new rule says only `deskhop-service` depends on it.
- The existing rule already keeps `win32-input` out of the service.

### D2. Shape of `bin/service`

| Module | Holds |
|---|---|
| `main.rs` | Arguments: none (SCM), or `--console [--data <dir>] [--port <n>]`. Builds a multi-thread tokio runtime. |
| `host.rs` | The engine loop (D3). Windows-free apart from the SAS call it is handed. |
| `supervisor.rs` | Session tracking and the agent lifecycle (D5), with a pure `Plan` state machine and an async driver. |
| `launch.rs` | The one launch function (D6). |
| `store.rs` | Data folder, file formats, atomic writes, quarantine (D7). |
| `console.rs` | Command parsing and printing (D11). |
| `scm.rs` | `windows-service` glue: status, control handler (D9). |
| `log.rs` | A small logger (D12). |

**Test seam:** `Host::run` takes its inputs as channels:
- agent links, as `AgentLink { handle, events, ended: oneshot::Sender<()> }`;
- commands, as `HostCommand`.

It also takes a `transport::Config` built by the caller. Tests feed links made with `ipc::link::AgentEnd` over `tokio::io::duplex`, and loopback node configs as the transport tests do. So the whole path from agent bytes through engine and node to the other machine's agent runs in one test process.

### D3. The host loop

One task owns the `Engine`, the `NodeHandle`, the current `AgentLink` (if any), a mirror of the `Desk` (for names), the set of up peers, and the store handle. It `select!`s over node events, the current link's events, new links and commands.

**Clock:**
- `clock()` is milliseconds since an `Instant` taken at start.
- It is passed to `ServiceEnd::accept`, so agent input arrives already on it, and the same function stamps `Undo`.

**Into the engine:**

| Source | Engine event |
|---|---|
| `FromAgent::Input { at, input, origin }` | `Local { at, input, origin }` |
| `FromAgent::Screen(s)` | `ScreenChanged(s)` |
| link ends (receiver `None`) | `LocalInputLost` (D4), then drop the link, which fires `ended` |
| `PeerUp(p)` | `PeerConnected(p)` |
| `PeerDown(p, _)` | `PeerLost(p)` |
| `Message { peer, msg }` | `FromPeer { peer, msg }` |
| stored layouts at start | `LayoutsLoaded(book)`, before the first link is accepted |
| `undo` / `forget-layout` | `Undo { at: clock() }` / `ForgetLayout` |

**Out of the engine:**

| Decision | Action |
|---|---|
| `SetCapture`, `Inject`, `WarpCursor` | `AgentHandle::send`; dropped when no link. A send error means the link is closing; its end arrives as `None`. |
| `Send { peer, msg }` | `NodeHandle::send` |
| `RequestSecureAttention` | D10 |
| `FocusChanged` | log, and console line |
| `LayoutsChanged(book)` | store |
| `OfferUndo { peer, side }` | log, and console line with the time left |

**Other node events:**
- `TrustStore(bytes)` and `Addresses(map)` go to the store. The trust store also refreshes the `Desk` mirror (`Desk::from_file`).
- `Paired`, `JoinFailed` and `PairingClosed` are logged and printed.

**New link:** the host first sends `SetCapture(mode_for(engine.focus()))`, using `Local → PassAll`, `Forwarding → WithholdAll` and `Controlled → WithholdMouse`. Only one link exists at a time; the supervisor guarantees it.

**Never logged:** `Input` contents and `Inject` actions. The log takes `&'static str` reasons and structured fields, never `Debug` of an input value.

### D4. `engine::Event::LocalInputLost`

**Behavior:**
- **`Forwarding { to, .. }`:** clear `forwarded` (the peer releases on withdrawal), send `FocusWithdrawn` to `to`, and set focus to `Local`. That emits `SetCapture(PassAll)` and `FocusChanged`; the warp toward the exit point is harmless with no agent.
- **`Controlled { by, .. }`:** emit releases for `injected`, which are dropped when no agent is linked. Send `TakenBack` to `by`. Set focus to `Local`.
- **`Local`:** nothing to peers.
- **Every state:** clear `pressed`, since a release from a dead agent never comes, and reset the takeover burst state.

`TakenBack` and `FocusWithdrawn` already exist, and their receivers already do what the spec scenarios need. So no wire message changes, and `PROTO_VERSION` and `IPC_VERSION` stay put.

*Alternative:* keep focus while the agent restarts, since a crash restart takes about 0.5 s. That leaves a peer controlled with nobody able to type into it whenever the agent does not come back (logoff, switch). The user pushes the edge again after a crash, which is cheap.

### D5. Supervisor

**`Plan`:** a pure state machine.
- Inputs: `SessionChanged(current console session, has_user)`, `Launched(pid)`, `LinkUp`, `LinkEnded`, `Exited`, `Tick(now)`, `Stop`.
- Outputs: `Launch(session)`, `EndLink`, `Terminate`, `WaitUntil(t)`.
- It holds the target session, the current agent's state (`Starting { deadline }`, `Linked { since }`, `Ending { deadline }`), and a `transport::node::Backoff`.
- Unit tests drive it with fake time.

**Driver:**
- Gets session events from the SCM handler (service mode) or one fixed session (console).
- Re-reads `active_console_session()` and `user_token()` on every `ConsoleConnect`, `ConsoleDisconnect`, `SessionLogon` and `SessionLogoff`, and at start.
- If the target changes, it ends the current agent first (D5 below) and then launches.

**Launch sequence:**
1. Pick a name, `\\.\pipe\deskhop-agent-<32 hex>`, from 16 `OsRng` bytes. If `create_agent_pipe` fails because the name exists, try a new name, up to 3 times.
2. `launch(target, pipe_name)` (D6) returns the process.
3. Wait for `connect()` and `ServiceEnd::accept` within 10 s in total. On timeout, terminate and count a failure.
4. On connect, compare `client_process_id` with the launched ID before accepting.
   - On a mismatch, drop the pipe unread, terminate the agent, log, and count a failure.
   - The open process handle prevents the ID being reused in the meantime.
5. Hand the `AgentLink` to the host.
   - When `ended` fires or the process exits, count a failure if the link lasted under 30 s; otherwise reset the backoff.
   - Then relaunch after `Backoff::wait()` if the session still has a user.

**Ending:**
- Drop the link: the host lets go of its handle when the supervisor sends `HostCommand::DropLink`.
- Wait up to 2 s for the process to exit, then `terminate`.

*Alternative:* job objects with kill-on-close. The agent already exits when its pipe closes, and termination covers a hung agent. A job object adds no protection.

### D6. The one launch function

`launch::agent(target: Target, pipe: &str) -> Result<Launched, LaunchError>` is the only place a process is started. The agent path is `current_exe().parent()/deskhop-agent.exe`.

| `Target` | How |
|---|---|
| `Session(SessionId)` | `user_token(session)`, then `win32_session::launch_as(token, agent, ["--session", "--pipe", name], dir)` |
| `Console` | `tokio::process::Command` with the same arguments, inheriting the terminal so the agent's stderr shows there; same user, same session |

`Launched` hides the difference behind `id()`, `wait()` (async: `spawn_blocking` on `Process::wait`, or `Child::wait`) and `terminate()`.

**ACL user:**
- `Session`: the token's user SID.
- `Console`: `own_sid()`.

### D7. Data folder and files

**Paths:**
- Service mode: `%ProgramData%\DeskHop`, falling back to `C:\ProgramData\DeskHop`.
- Console mode: `%LOCALAPPDATA%\DeskHop\console`, or `--data`. `secure_folder` is not applied there; the profile's ACL protects it.

**`secure_folder` (service mode, before anything is read):**

| Folder state | Action |
|---|---|
| absent | create it with owner SYSTEM and DACL `D:P(A;OICI;FA;;;SY)` |
| owned by SYSTEM or Administrators (`BA`, what an elevated installer creates) | set owner SYSTEM and re-apply the protected DACL, which repairs inherited ACEs on existing children |
| any other owner | rename to `DeskHop.untrusted-<unix seconds>` (SYSTEM can, through its rights on `ProgramData`), log it, and create a fresh folder |

A junction planted by a user is owned by that user, so the junction itself is renamed and its target is never followed.

**Files** (all at most 1 MiB to read; anything larger is unreadable):

| File | Format |
|---|---|
| `identity.key` | 37 bytes, `Identity::to_file` |
| `trust-store.bin` | `DHTS`, `Desk::to_file`, via `Machine::trust_store` / `load_trust_store` |
| `addresses.json` | `{"version":1,"peers":{"<64 hex>":"<ip>:<port>"}}` |
| `layouts.json` | `{"version":1,"sets":[{"monitors":["<id>",…],"sides":[{"monitor":"<id>","side":"left\|right\|top\|bottom","peer":"<64 hex>"}]}]}` |
| `service.log`, `service.log.old` | D12 |

The JSON goes through mirror structs in `store.rs` with `serde`/`serde_json`, so `model` gains no `serde`. Unknown `version` values make the file unreadable.

**Writes:**
- Write `<name>.tmp`, `sync_all`, then `std::fs::rename` over the target, which replaces atomically on NTFS.
- A store task owns all writes: the host sends the latest contents per file, and the task writes whatever is newest, so a burst of changes costs one write.
- On stop, the host waits for the store task to drain.

**Unreadable file:** rename it to `<name>.bad-<unix seconds>`, log the reason, and start as if it were absent.
- For `identity.key`, this means a new identity, and peers must pair again. That is logged loudly. It is still better than refusing to start or trusting a damaged key.

*Alternative:* one combined state file. Separate files keep a damaged layouts file from costing the desk, which is the "Damaged layouts file" scenario.

### D8. Identity and machine name

- **Start:** load `identity.key`, or generate it with `OsRng` and store it before `transport::node::start`.
- **Machine name:** `COMPUTERNAME`, which SCM services have, cut at a char boundary to 63 bytes. If it is unset or empty, use `"Windows PC"`.

### D9. Service control, power and stop

**Accepted controls:** `STOP | SHUTDOWN | SESSION_CHANGE | POWER_EVENT`. The handler runs on the SCM's thread and only sends commands.

| Control | What the service does |
|---|---|
| `Stop` / `Shutdown` | report `StopPending` (wait hint 5 s), then `HostCommand::Stop` |
| `SessionChange` | forwarded to the supervisor |
| `PowerEvent(Suspend)` | `HostCommand::Suspend(reply)`: the host awaits `node.goodbye()` and replies. The handler waits on a `std::sync::mpsc` reply for at most 1 s before returning `NO_ERROR`. |
| `PowerEvent(ResumeAutomatic \| ResumeSuspend)` | `HostCommand::Resume`, then `node.resume()`; idempotent |

**Stop order:**
1. `node.goodbye()`, bounded to 1 s.
2. The supervisor ends the agent (D5).
3. The store drains.
4. `node.shutdown()`.
5. Report `Stopped`.

**Console mode:** Ctrl+C (`tokio::signal::ctrl_c`) and `quit` map to `Stop`. Console mode gets no power events.

### D10. Secure attention

On `RequestSecureAttention`, the host runs this in `spawn_blocking`:
- **Service mode:** if `sas_policy_allows_services()`, call `send_sas()`; otherwise log `SoftwareSASGeneration` with what it found.
- **Console mode:** log that it needs the registered service.

`SendSAS` returns nothing, so the policy check is the only failure the service can see.

### D11. Console mode

**Commands** are read line by line from `tokio::io::stdin`:

| Command | Action |
|---|---|
| `pair` | `open_pairing()`, then print the code, or "no IPv4 network" |
| `close` | `close_pairing()` |
| `join <code>` | `Code::parse` (errors printed as `CodeError`'s text), then `join` |
| `peers` | members from the `Desk` mirror, with up or down and an 8-hex key prefix |
| `remove <peer>` | resolve, then `remove` |
| `undo`, `forget-layout`, `quit` | as named |

**Resolving `<peer>`:** a case-insensitive match on the full machine name, or else a hex key prefix of at least 2 characters. Zero or several matches print an error. Parsing and resolving are pure functions with unit tests.

**Prints:** the console output listed in the spec, as plain lines such as `paired with LAPTOP-7 (ab12cd34)`.

**Output:** code and reports go to stdout; logs go to stderr.

### D12. Logging

- `log.rs` holds a process-wide `Mutex<Sink>`.
  - Service mode: a file, `service.log`, rotated to `service.log.old` at 1 MiB.
  - Console mode: stderr.
- In service mode each new log file, the first and every one after a rotation, is created through `win32_session::create_admin_readable`. Its own protected DACL is `D:P(A;;FA;;;SY)(A;;FR;;;BA)`. Windows lets anyone traverse a folder on the way to a file (`SeChangeNotifyPrivilege`), so an elevated administrator can read the log by its path, while the folder and every other file stay SYSTEM-only. The log holds no codes, keys or input, so this exposes nothing. Added during apply, when the dev script's `log` command could not read the log.
- Lines are `2026-10-04T12:00:00.123Z LEVEL message`. The UTC conversion is a small days-to-civil function, so no new crate is needed.
- Levels: `info`, `warn` and `error`.
- No `log` or `tracing` dependency, so quinn's `log` output stays unwired, as today.

### D13. Dev script

`tools/dev/service.ps1 install|uninstall|start|stop|status|log`. The user runs it from an elevated PowerShell, and the script refuses to run otherwise.

- **`install`:**
  - `cargo build`.
  - Copy `deskhop-service.exe` and `deskhop-agent.exe` to `%ProgramFiles%\DeskHop Dev\`. A SYSTEM service must never run from a user-writable folder such as the repo.
  - `sc.exe create DeskHop binPath= … start= demand`.
  - Add inbound firewall rules for that executable: UDP 47391 and UDP 5353.
  - Set `SoftwareSASGeneration` to `1` if it is unset, and print each change it makes.
- **`uninstall`:**
  - Stop and delete the service.
  - Remove the firewall rules and the copied folder.
  - Leave `%ProgramData%\DeskHop`, and print how to remove it.
  - Leave the policy value, and print it.

### D14. Tests

- **`engine` replay tests** for the three `LocalInputLost` scenarios, plus "held physical key forgotten": a release after relink is dropped. They run on Linux.
- **`win32-session`** (normal user, no admin):
  - a pipe created for `own_sid()` opens, and `client_process_id` equals `std::process::id()`;
  - a pipe created for a different SID (`S-1-5-32-546`, Guests) refuses our open with access denied;
  - a second `create_agent_pipe` with the same name fails;
  - SDDL strings are built correctly.
  - `secure_folder`, `user_token` and `launch_as` need SYSTEM, so they are checked by hand through the dev script.
- **`bin/service` unit tests:**
  - store formats round-trip; atomic replace; unreadable and oversized files are quarantined; an unknown version is rejected;
  - console parsing and resolving: exact name, prefix, ambiguous, too short;
  - `Plan` transitions: logon, logoff, switch, exit backoff `0.5 → 30 s`, reset after 30 s linked, 10 s start deadline, 2 s end deadline;
  - log rotation and timestamp format.
- **`bin/service` host tests** (two hosts, loopback, discovery off, fake agents over duplex):
  - pairing through commands;
  - the cursor crosses: learn by pushing, then B's agent gets `Warp` then `Inject`;
  - restart keeps desk and layout;
  - a new link gets the capture mode first;
  - agent link lost while forwarding: B regains focus;
  - stop while forwarding: B sees goodbye;
  - suspend and resume;
  - a captured log contains no key usages after typing while forwarding.
- **PID check:** a test drives the supervisor's accept step with a real agent pipe and an expected ID that is not ours. The connection is closed unread.
- **Desktop tests** (`#[ignore]`, kill-on-drop guards as in `bin/agent/tests/link.rs`):
  - console mode with `--data <tmp> --port 0` links its agent;
  - `quit` makes both processes exit 0;
  - killing the agent gets a new one linked within 2 s.
- **Manual, run by the user:**
  - Dev script install, then check:
    - the agent runs as the user at medium integrity;
    - restart after the agent is killed;
    - folder owner and ACL (`icacls`);
    - pipe ACL;
    - sleep and resume;
    - Ctrl+Alt+End.
  - Two PCs in console mode: pair, cross, cross back, restart, cross again.

## Risks / Trade-offs

- **`SendSAS` is silent.** The policy check catches the usual cause, but not every refusal. → The manual check covers it, and the log says when it was requested.
- **Injected keys held when the agent dies.** The engine's releases are dropped with no agent, so the OS may show a key as down until it is next pressed. → Rare (an agent crash while controlled with a key held). A pressed-key resync on relink can come later.
- **Console mode on the registered service's port.** Both want UDP 47391. → `--port`, and the "port taken" error names it.
- **Moving a damaged identity aside costs the desk.** → Logged as an error. The alternative, refusing to start, leaves the machine unusable without admin repair.
- **`windows-service` macro and `forbid(unsafe_code)`.** → D1: the dispatcher entry moves into `win32-session` if needed. Decided during apply by compiling, with no behavior difference.
- **ProgramData race at first start.** A user can create the folder between checking and creating it. → `CreateDirectoryW` with the descriptor fails if it exists; the service then re-checks the owner, and the move-aside branch handles it.
- **No RDP or second-session support.** → A non-goal, logged when the console session has no user but others do.

## Migration Plan

Nothing is deployed yet. Before this change the dev loop is `--trace` in the agent; afterwards it is console mode on two PCs. To take the registered service out again, run `tools/dev/service.ps1 uninstall`.

## Open Questions

- Whether Windows 10 22H2 needs `SE_INCREASE_QUOTA_NAME` enabled explicitly in the SYSTEM token for `CreateProcessAsUserW`. SYSTEM normally holds it, enabled. The manual check confirms it, and enabling it is a local change in `launch_as` if needed.
