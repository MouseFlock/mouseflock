# Design

## Context

- **`ipc`** (from `add-ipc`):
  - frames, `Hello` with roles (`3` is UI), and a registry whose `Direction` is `ToService`, `ToAgent` or `Both`;
  - `check(ty, receiving, version)` rejects a type travelling the wrong way;
  - sans-I/O codecs under no-panic lints, a never-panic test and the `decode_ipc` fuzz target;
  - the agent link's async ends in `link.rs`, with a handshake under a 5 s timeout, reader and writer tasks, and an outbound cap.
- **`bin/service`** (from `add-service`):
  - the host loop owns the engine, the node, a `Desk` mirror, the set of up peers, and the agent link;
  - it reports `Report`s to console mode, and takes `HostCommand`s (`OpenPairing`, `Join(Code)`, `Remove`, `Undo`, `ForgetLayout`, …);
  - the supervisor tells the host nothing about sessions.
- **`win32-session`:** `create_agent_pipe` (one instance, SYSTEM plus one user), `client_process_id`, `active_console_session`, `own_sid`.
- **The `ipc-protocol` spec** uses `0x0042` as its example of an unknown type, so the UI types avoid `0x0040`–`0x004F`.
- **Settled with the user:**
  - any user at the console may use every UI command;
  - this change is the headless link, and `add-onboarding` is the Tauri app;
  - the Undo notice is a tray popup, so `Learned` must carry the time left.

## Goals / Non-Goals

**Goals:**
- Every requirement in `specs/ui-link`, tested:
  - in-process, with `UiEnd` over `tokio::io::duplex` against the real host;
  - end to end, with a Rust client over the real pipe in console mode.
- A UI client end (`ipc::ui::UiEnd`) that `add-onboarding`'s `src-tauri` can use unchanged.

**Non-Goals:**
- The Tauri app, tray, popups and autostart (`add-onboarding`).
- The UI's check that the pipe server is the real service (`add-onboarding`: it lives in the UI process, which may not link `win32-session`).
- Machine rename, the arrangement view (ADR 0005, post-MVP), settings and diagnostics.
- Policy restrictions on who may pair (managed configuration, later).

## Decisions

### D1. Registry directions per link

`Direction` gains `UiToService` and `ServiceToUi`, and the agent link's directions are renamed for symmetry (`AgentToService`, `ServiceToAgent`). Each decoder passes the one direction it receives:
- the service's agent end passes `AgentToService`;
- the agent passes `ServiceToAgent`;
- the service's UI end passes `UiToService`;
- the UI passes `ServiceToUi`.

`check` already compares exactly, so a UI type on an agent link, or the reverse, is a protocol error with no new code path.

*Alternative:* a separate registry per link. That duplicates `Hello` and the checks, and the spec already speaks of one registry with directions.

### D2. UI messages (IPC version 1)

All integers are little-endian.
- **`key`:** a member's 32-byte Ed25519 public key.
- **`name`:** `name_len: u8` (1–63), then UTF-8.
- **`code`:** `code_len: u8` (1–32), then printable ASCII `0x21`–`0x7E`.

| Type | Name | Direction | Payload |
|---|---|---|---|
| `0x0030` | `OpenPairing` | UI → service | empty |
| `0x0031` | `ClosePairing` | UI → service | empty |
| `0x0032` | `Join` | UI → service | code |
| `0x0033` | `RemovePeer` | UI → service | key |
| `0x0034` | `Undo` | UI → service | empty |
| `0x0035` | `ForgetLayout` | UI → service | empty |
| `0x0038` | `Status` | service → UI | see below |
| `0x0039` | `Paired` | service → UI | key, name |
| `0x003A` | `JoinFailed` | service → UI | reason u8 |
| `0x003B` | `PairingClosed` | service → UI | reason u8 |
| `0x003C` | `PairingNotOpened` | service → UI | reason u8 |
| `0x003D` | `Learned` | service → UI | key, side u8, undo_ms u32 |

**`Status`:**
```
key, name                                   this machine
agent: u8                                   0 none, 1 linked
pairing: u8                                 0 closed, 1 open
  [code, left_ms: u32]                      only when open
focus: u8                                   0 local, 1 on a member, 2 controlled by one
  [key]                                     only when not local
n: u16 (≤ 256), then n × (key, name, up: u8)
```
The largest `Status` is about 25 KB, inside the 65,536-byte frame.

**Codes:**
- `JoinFailed` reason:
  - `0` wrong code, `1` not ready, `2` protocol error, `3` desk full, `4` connection lost, `5` unreachable: the same order as `pairing::JoinFailure`;
  - `6` mistyped code (`CodeError::CheckDigit`);
  - `7` malformed code (any other `CodeError`).
- `PairingClosed` reason: `0` a machine joined, `1` time ran out, `2` closed by the user, `3` too many wrong codes.
- `PairingNotOpened` reason: `0` no IPv4 network.
- Side: `0` left, `1` right, `2` top, `3` bottom, as in `proto`.
- Booleans, `up` included: `0` or `1`.

Any other value is a protocol error.

**`Join` carries the code as typed,** so the service parses it with `pairing::Code::parse` and answers a typo with reason `6`. The UI never needs `pairing`.

*Alternative:* the UI parses the code. That puts `pairing` (and its crypto dependencies) in the UI, or duplicates the check-digit rules in TypeScript.

### D3. `ipc::ui`: types and async ends

- `ipc::ui::{UiCommand, UiEvent, Status, Member, Pairing, FocusOf, JoinFailedReason, …}` hold the messages above in plain types, with `[u8; 32]` keys and `String` names. `ipc` still depends only on `model`; `model::Side` is reused.
- **`ServiceUiEnd::accept(stream) -> (UiHandle, Receiver<UiCommand>)`:**
  - the handshake accepts role `3` only;
  - after it, the writer task sends whatever `UiHandle::send(UiEvent)` queues, and never waits;
  - more than 256 queued frames close the link.
- **`UiEnd::connect(stream) -> (ServiceUiHandle, Receiver<UiEvent>)`:** the client end, for tests and for `add-onboarding`.
- The reader, frame cap, and close-on-error are shared with the agent link by moving the generic parts of `link.rs` (handshake, `write_frame`, a bounded frame reader) into `link/common.rs`. Clock alignment stays agent-only.

### D4. Status coalescing in the host

- The host keeps a `UiHub`: the connected UIs (at most 8), each with its `UiHandle`.
- **`Status` is computed from host state**, never stored: the `Desk` mirror, the up set, `engine.focus()`, whether an agent is linked, and pairing.
  - Pairing is new host state: `Some((code, opened_at))` after `open_pairing` succeeds, and cleared on `PairingClosed`.
  - The time left is `WINDOW_MS − (now − opened_at)`, clamped at 0.
- **Dirty flag:** any event that can change a field sets it. A 100 ms tick (`tokio::time::interval`) sends the current `Status` to every UI when the flag is set, then clears it. That gives at most one `Status` per 100 ms, always the latest. A new UI gets its first `Status` at once, outside the tick.
- **Notices** go out at once to every UI, mapped from the same places that produce `Report`s today (`Paired`, `JoinFailed`, `PairingClosed`, `OfferUndo`), plus a mistyped or malformed `Join` and a failed `OpenPairing`.
- **UI commands** become `HostCommand`s inside the host:
  - `Join` parses its code first;
  - `RemovePeer` is ignored unless the key is a member;
  - a UI's `OpenPairing` answers through `Status` and `PairingNotOpened`, not a reply channel.

### D5. The UI pipe server

A task in `bin/service` (`ui.rs`):

1. **Create the next instance** with `win32_session::create_ui_pipe(name, first, admit)`:
   - the first instance at start, `first_pipe_instance(true)`;
   - later ones with it false;
   - `reject_remote_clients(true)`, `max_instances(8)`.
   - If the first creation fails because the name exists, log it and retry every 5 s.
2. **`connect().await`.** With 8 connected, the next instance is not created until one leaves, so a ninth client sees `ERROR_PIPE_BUSY`.
3. **Session check.** `client_session_id(&pipe)` (`GetNamedPipeClientSessionId`) must equal `active_console_session()`. Console mode compares with its own session instead. A mismatch closes the pipe before `Hello`.
4. **`ServiceUiEnd::accept`,** then hand the link to the host as `HostInput::Ui(link)`.

**ACL (`admit`):**
- Registered service: `D:P(A;;GA;;;SY)(A;;GRGW;;;IU)`. `IU` is the interactive logon group, which includes RDP users; the session check narrows it to the console.
- Console mode: `D:P(A;;GA;;;SY)(A;;GRGW;;;<own SID>)`.

**Pipe rights:** generic write includes creating instances (as noted for agent pipes). Here the name is long-lived, so `max_instances(8)` is what stops a client adding its own instance. Windows enforces the first creator's maximum on later creators of the same name.

*Alternative:* a per-session pipe name. The UI could not find it without asking the service, and the console check covers the same need.

### D6. Pipe squatting

A process that creates `\\.\pipe\deskhop-ui` before the service could pose as the service to UIs.
- **Mitigation here:** the service is a boot-time service, so it normally creates the name before any user logs on. If the name is taken, the service logs it loudly and retries.
- **Mitigation in `add-onboarding`:** the UI checks `GetNamedPipeServerSessionId == 0` (only services run in session 0) before trusting what it reads.

Both go in the threat model.

### D7. Console mode

- **The UI pipe:** console mode serves it with the own-user ACL when it can create the first instance. If the registered service already holds the name, console mode logs "UI pipe in use (the registered service is running?)" and runs without it.
- **Printing:** console printing is unchanged. The host keeps `Report` for the console and adds UI events beside it.

### D8. Tests

- **`ipc` unit tests:**
  - every UI message round-trips;
  - each layout is checked one byte short and one byte long;
  - codes and limits: a 64-byte name, an empty name, a 33-byte code, a code with a space, non-UTF-8, 257 members, reason `8`, side `4`, `up` = 2;
  - a UI type on an agent link and an agent type on a UI link are rejected;
  - the largest `Status` fits in a frame.
- **`ipc` duplex tests:** `ServiceUiEnd` rejects role `1`; events arrive in order; a UI that stops reading is cut off after 256 frames while sends never wait.
- **Never-panic and fuzz:** `never_panic.rs` and `decode_ipc` include the UI directions.
- **Host tests** (two hosts over loopback, as in `add-service`), with UIs as `UiEnd` over duplex:
  - `status_on_connect`;
  - `status_after_peer_down` (within 200 ms);
  - `burst_is_coalesced`;
  - `open_pairing_shows_code`;
  - `join_from_ui`;
  - `mistyped_code_is_reported`;
  - `remove_unknown_key_changes_nothing`;
  - `learned_reaches_every_ui`;
  - `stalled_ui_is_dropped` (other UIs and the agent unaffected);
  - `malformed_command_closes_only_that_ui`.
- **`win32-session` tests:**
  - a UI pipe made for this user opens;
  - `client_session_id` equals this session;
  - a ninth client gets busy when `max_instances` is 8;
  - creating the first instance twice fails.
- **Desktop test** (`#[ignore]`, console mode): a Rust `UiEnd` client over the real pipe gets `Status` and opens pairing.
- **Pipe-level session check:** a pipe created for this session with an expected session that differs closes the client unread. The comparison is factored into a function, so the mismatch can be tested without a second session.

## Risks / Trade-offs

- **Any console user can pair and remove machines.** → Chosen for zero configuration. The risk sits in the threat model; managed configuration can narrow it later.
- **`IU` includes RDP users.** → The session check closes them. A remote user who becomes the console session (session switch) is then the console user, which is the intended rule.
- **The pairing code is visible to every UI at the console.** → It is shown on screen anyway, and it is only good for 3 guesses during a 10-minute window.
- **A UI flooding commands.** → Each command is cheap, and pairing attempts are limited by pairing mode. A per-UI rate limit is not needed now.
- **Squatting before the service starts.** → D6, completed in `add-onboarding`.

## Migration Plan

Nothing is deployed. `IPC_VERSION` stays 1 (unreleased), and `docs/protocol.md` records the UI types under version 1.
