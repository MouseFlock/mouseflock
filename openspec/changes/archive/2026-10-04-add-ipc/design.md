# Design

## Context

- **`ipc`:** holds only `IPC_VERSION = 1`. `bin/agent` and `bin/service` already depend on it. `engine` must never reach it, and `cargo archcheck` enforces that.
- **`win32-input`** (from `add-input-capture`) gives the agent:
  - `start()`, a `Capture` with `set_mode`, `inject` and `warp`;
  - a `std::sync::mpsc::Receiver<Captured>`;
  - `now()`, the performance-counter clock in milliseconds.
- **`proto`** set the house pattern for wire formats: sans-I/O codecs, fixed payloads, strict decoding, and no-panic lints, tested by a never-panic test and fuzz targets.
- **ADR 0002:** agents are less trusted than the SYSTEM service, and the service validates everything it receives. Agents never touch the network.
- **Split agreed with the user:** `add-ipc` defines the link and makes the agent speak it. `add-service` creates the pipe (random name, ACL via a new `win32-session` crate, ADR 0006), launches the agent, and runs the engine. So this change needs no `unsafe`.

## Goals / Non-Goals

**Goals:**
- Every requirement in `specs/ipc-protocol` and `specs/agent-link`, tested in-process over `tokio::io::duplex` and end to end against the real agent binary over a test-created named pipe.
- A service end that `add-service` can drop behind its pipe unchanged.
- The agent can never leave input withheld when the link is gone.

**Non-Goals:**
- Creating pipes with ACLs, launching agents, and verifying the client process ID: `add-service` (`win32-session`).
- UI messages (`add-onboarding`) and the winlogon agent.
- Clipboard messages (`add-text-clipboard`).

## Decisions

### D1. Frames and `Hello`

- **Frame:** `len: u32le | type: u16le | payload`, with `len` ≤ 65,536. This is the same shape as `proto`'s frames, so it reads the same, but it is its own registry and version: the two protocols evolve separately.
- **`Hello` (`0x0001`):** `min: u16, max: u16, role: u8`, with extra bytes ignored.
  - Roles: `1` session agent, `2` winlogon agent, `3` UI, `4` service.
  - Both ends send `Hello` at once.
  - The service end of an agent link accepts only role `1` (`2` arrives with the winlogon agent), and the agent end accepts only role `4`.
- **No close codes.** Pipes carry none, so an end that finds an error simply closes, and the service logs why. A version mismatch looks the same to the other end; the service log records which side is older.

*Alternative:* reusing `proto`'s frame code. That would make `ipc` depend on `proto` and tie two independent versions together. The frame logic is about 60 lines.

### D2. Agent-link messages (IPC version 1)

All integers are little-endian. The codes are:
- origin: 0 physical, 1 injected;
- down: 0 released, 1 pressed;
- button: 0 left, 1 right, 2 middle, 3 X1, 4 X2;
- capture mode: 0 pass all, 1 withhold all, 2 withhold mouse.

Times are `u64` milliseconds on the sender's monotonic clock.

| Type | Name | Direction | Payload | Bytes |
|---|---|---|---|---|
| `0x0010` | `KeyEvent` | agent → service | at u64, origin u8, usage u16, down u8 | 12 |
| `0x0011` | `ButtonEvent` | agent → service | at u64, origin u8, button u8, down u8 | 11 |
| `0x0012` | `WheelEvent` | agent → service | at u64, origin u8, dx i32, dy i32 | 17 |
| `0x0013` | `MotionEvent` | agent → service | at u64, origin u8, dx i32, dy i32, x i32, y i32 | 25 |
| `0x0014` | `Screen` | agent → service | n u8 (≤ 32), then n × (id_len u16 1..=256, id UTF-8, x i32, y i32, w i32, h i32, dpi u32) | 1 + Σ(22 + id_len) |
| `0x0015` | `Pong` | agent → service | service_time u64, agent_time u64 | 16 |
| `0x0020` | `SetCapture` | service → agent | mode u8 | 1 |
| `0x0021` | `InjectKey` | service → agent | usage u16, down u8 | 3 |
| `0x0022` | `InjectButton` | service → agent | button u8, down u8 | 2 |
| `0x0023` | `InjectWheel` | service → agent | dx i32, dy i32 | 8 |
| `0x0024` | `InjectMotion` | service → agent | dx i32, dy i32 | 8 |
| `0x0025` | `Warp` | service → agent | x i32, y i32 | 8 |
| `0x0026` | `Ping` | service → agent | service_time u64 | 8 |

**Screen limits:** w and h ≥ 1, dpi 48–960, and every monitor's right and bottom edges must fit in `i32`. A screen that breaks them is a protocol error.

The registry records each type's direction (`ToService`, `ToAgent`, or both for `Hello`). Decoding takes the receiving end's side, so a type travelling the wrong way is a protocol error.

`ipc::message` maps these to and from `model` types (`InputEvent`, `InputAction`, `CaptureMode`, `Screen`, `Point`). `ipc` depends on `model`, which is a leaf crate; `model` gains nothing.

*Alternative:* serde with a self-describing format. `serde_json` is in the workspace, but a SYSTEM service decoding untrusted JSON is a larger surface than fixed layouts. Fixed layouts also keep the no-panic lints and fuzzing exactly as in `proto`.

### D3. Async ends

`ipc::link` provides both ends, generic over `S: AsyncRead + AsyncWrite + Unpin + Send + 'static`:

```rust
ServiceEnd::accept(stream, clock: impl Fn() -> Millis) -> Result<(AgentHandle, Receiver<FromAgent>), LinkError>
AgentEnd::connect(stream, clock: impl Fn() -> Millis) -> Result<(ServiceHandle, Receiver<FromService>), LinkError>
```

- **Handshake:** both ends write `Hello` at once, then read the other's, within 5 s. A timeout, version mismatch or wrong role is a `LinkError`.
- **Tasks:** after the handshake, each end runs a reader task and a writer task.
  - The reader decodes one frame at a time into a buffer capped at header plus frame limit, and sends each message on an unbounded channel.
  - The writer drains an outgoing queue.
- **`AgentHandle::send(cmd)`:** pushes onto the queue and never waits. If more than 4,096 frames are queued, the agent is stuck, and the link is closed.
- **Closing:** dropping the handle, or any protocol error, ends both tasks and closes the stream. The receiver then yields `None`, which is how each side learns the link ended.
- **The agent end answers `Ping` inside the reader task,** using the clock it was given (`win32_input::now` in the real agent). The agent's main loop never has to.

### D4. Clock alignment in the service end

- **Ping:** the service end sends `Ping(t0)` right after the handshake and every 10 s after.
- **Estimate:** on `Pong(t0, a)` received at `t1`, it computes `rtt = t1 − t0` and `offset = a − (t0 + t1) / 2` in `i64`. It keeps the estimate with the smallest `rtt` seen.
- **Conversion:** agent event time `at` becomes `at − offset`, clamped at 0 into `Millis`.
- **Before the first `Pong`:** decoded events wait in an order-preserving buffer of up to 10,000 events. Beyond that, the link closes as stuck. The first `Pong` normally arrives within a millisecond.
- **Test seam:** `offset` and the conversion live in a pure `ClockSync` with unit tests. The service's clock function is a parameter, so tests drive time.
- **Effect:** this ties the agent's performance counter to whatever monotonic clock the service passes. In `add-service` that is `Instant`-based milliseconds since start, so `Undo` and other service-stamped events share one timeline with input.

### D5. The session agent

The command line is `deskhop-agent.exe --session --pipe <name>`. Both flags are required, and `<name>` is the pipe's full path, such as `\\.\pipe\deskhop-agent-<hex>`.

1. **Startup:** a Tokio current-thread runtime starts, and the agent opens the pipe with `ClientOptions::open`. It retries `ERROR_PIPE_BUSY` for up to 1 s, and exits with code 3 on any other failure.
2. **Handshake:** `AgentEnd::connect` runs with the clock `win32_input::now`. Failure exits with code 4.
3. **Capture starts:** `win32_input::start()` is called only now, so nothing is captured before the link is up. A failure to start exits with code 5.
4. **Event bridge:** a blocking task moves each `Captured` from the capture's `std::sync::mpsc` receiver to the link.
5. **Commands:** the main loop applies `FromService` commands with `Capture::set_mode`, `inject` and `warp`. Errors, such as `InjectError::Blocked`, are logged to stderr and ignored.
6. **Link ends:** when the receiver yields `None`, the agent drops `Capture` first, which sets `PassAll` and unhooks. It then exits with code 0.

If the process dies outright, Windows removes its hooks anyway, so the 100 ms requirement holds in every case. A broken pipe is seen at once, because the read returns an error as soon as the server's handle closes.

`--trace` stays as it was, and `--winlogon` keeps its stub.

### D6. Tests

- **`ipc` codec (unit):**
  - frame bytes, the oversized header, and need-more input;
  - every message round-trips;
  - bad lengths, codes and directions are rejected;
  - screen limits (33 monitors, an empty id, a 257-byte id, non-UTF-8, w = 0, dpi 47 and 961, right edge overflowing);
  - `Hello` with trailing bytes;
  - `ClockSync` (the spec scenario, a smaller rtt replacing the estimate, a larger one ignored).
- **`ipc` never-panic test and fuzz target `decode_ipc`:** arbitrary and mutated bytes through both ends' decoders.
- **`ipc` ends over `tokio::io::duplex` (paused tokio time where timing matters):**
  - handshake, wrong role, versions that don't overlap, and silence for 5 s;
  - events converted with the clock, and events held until the first `Pong`;
  - commands arrive in order;
  - a protocol error closes the link;
  - `stalled_agent_does_not_block`: a 64-byte duplex with an agent that never reads; 1,000 sends return at once, and the link closes once the queue passes 4,096.
- **Agent binary (`bin/agent/tests/link.rs`):** every test kills its agent process when it ends, pass or fail. Added during apply, after a broken agent from a mutation run outlived its test while withholding all input.
  - `no_pipe_exits_nonzero` runs anywhere.
  - Desktop tests are `#[ignore]` and use a test-created `ServerOptions` pipe (default ACL, same user):
    - `first_report_is_the_screen`;
    - `inject_reaches_the_service_as_injected` (F24 comes back as an injected `KeyEvent`);
    - `service_gone_releases_capture` (set withhold all, drop the server, and the agent exits within 1 s, then an injected F24 is checked through a fresh capture).

## Risks / Trade-offs

- **A process impersonating the agent.** Until `add-service`, nothing checks who connected. Anything that reaches the pipe can claim physical input, which the service would forward to peers. → `add-service` creates each pipe under a random name with an ACL for SYSTEM and that session's user, and accepts only the process ID it launched (`GetNamedPipeClientProcessId`). The threat model records this as an open item until then.
- **Unbounded inbound channel.** A fast agent can outrun a stalled service consumer. → The service drains it in its main loop. The memory risk is the service's own stall, not the agent's doing. The 4,096-frame cap applies to the outbound queue, which the agent controls by not reading.
- **The 10 s ping cadence.** It is enough for drift between two clocks on the same machine. → A constant (`PING_EVERY`).
- **`ERROR_PIPE_BUSY` retry.** The service creates one instance per agent, so busy should not happen. The retry only covers a race at creation.
